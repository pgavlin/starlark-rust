/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! JSON-backed call summaries for the exploratory source analyzer.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::eval::effects::analyze::CallSummary;
use crate::eval::effects::analyze::GuardedSummaryEffect;
use crate::eval::effects::analyze::SummaryEffect;
use crate::eval::effects::analyze::SummaryGuard;
use crate::eval::effects::analyze::SummaryMutation;

#[derive(Debug, thiserror::Error)]
enum RegistryError {
    #[error("effect for function `{0}` must specify exactly one effect kind")]
    EffectKind(String),
    #[error("normal effect for non-returning function `{0}` is unreachable")]
    NormalWithoutReturn(String),
    #[error("failure effect for infallible function `{0}` is unreachable")]
    FailureWithoutFailure(String),
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SummaryRegistry {
    #[serde(default)]
    functions: BTreeMap<String, JsonFunctionSummary>,
}

impl SummaryRegistry {
    pub(crate) fn parse(json: &str) -> crate::Result<Self> {
        let registry: Self = serde_json::from_str(json).map_err(crate::Error::new_other)?;
        registry.validate()?;
        Ok(registry)
    }

    pub(crate) fn functions(&self) -> impl Iterator<Item = (&str, CallSummary)> {
        self.functions
            .iter()
            .map(|(name, summary)| (name.as_str(), summary.to_summary()))
    }

    fn validate(&self) -> crate::Result<()> {
        for (name, summary) in &self.functions {
            for effect in &summary.effects {
                if effect.kind_count() != 1 {
                    return Err(crate::Error::new_other(RegistryError::EffectKind(
                        name.clone(),
                    )));
                }
                match effect.guard {
                    JsonGuard::Normal if !summary.may_return => {
                        return Err(crate::Error::new_other(RegistryError::NormalWithoutReturn(
                            name.clone(),
                        )));
                    }
                    JsonGuard::Failure if !summary.may_fail => {
                        return Err(crate::Error::new_other(
                            RegistryError::FailureWithoutFailure(name.clone()),
                        ));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonFunctionSummary {
    #[serde(default)]
    effects: Vec<JsonGuardedEffect>,
    #[serde(default = "default_true")]
    may_return: bool,
    #[serde(default)]
    may_fail: bool,
}

impl JsonFunctionSummary {
    fn to_summary(&self) -> CallSummary {
        CallSummary {
            effects: self
                .effects
                .iter()
                .map(JsonGuardedEffect::to_effect)
                .collect(),
            may_return: self.may_return,
            may_fail: self.may_fail,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JsonGuard {
    #[default]
    Entered,
    Normal,
    Failure,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonGuardedEffect {
    #[serde(default)]
    guard: JsonGuard,
    host_effect: Option<JsonNamedArguments>,
    observe_host: Option<JsonNamedArguments>,
    mutate_argument: Option<JsonMutateArgument>,
    mutate_all: Option<JsonNamedMutation>,
    unknown: Option<JsonUnknown>,
}

impl JsonGuardedEffect {
    fn kind_count(&self) -> usize {
        [
            self.host_effect.is_some(),
            self.observe_host.is_some(),
            self.mutate_argument.is_some(),
            self.mutate_all.is_some(),
            self.unknown.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count()
    }

    fn to_effect(&self) -> GuardedSummaryEffect {
        let effect = if let Some(effect) = &self.host_effect {
            SummaryEffect::HostEffect {
                name: effect.name.clone(),
                arguments: effect.arguments.clone().into_boxed_slice(),
            }
        } else if let Some(effect) = &self.observe_host {
            SummaryEffect::ObserveHost {
                name: effect.name.clone(),
                arguments: effect.arguments.clone().into_boxed_slice(),
            }
        } else if let Some(effect) = &self.mutate_argument {
            SummaryEffect::MutateArgument {
                target: effect.target,
                mutation: match &effect.mutation {
                    JsonMutation::AppendArgument(index) => SummaryMutation::AppendArgument(*index),
                    JsonMutation::Opaque(name) => SummaryMutation::Opaque(name.clone()),
                },
            }
        } else if let Some(effect) = &self.mutate_all {
            SummaryEffect::MutateAll(effect.name.clone())
        } else if let Some(effect) = &self.unknown {
            SummaryEffect::Unknown(effect.origin.clone())
        } else {
            unreachable!("validated before conversion")
        };
        GuardedSummaryEffect {
            guard: match self.guard {
                JsonGuard::Entered => SummaryGuard::Entered,
                JsonGuard::Normal => SummaryGuard::Normal,
                JsonGuard::Failure => SummaryGuard::Failure,
            },
            effect,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonNamedArguments {
    name: String,
    #[serde(default)]
    arguments: Vec<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonMutateArgument {
    target: usize,
    mutation: JsonMutation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum JsonMutation {
    AppendArgument(usize),
    Opaque(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonNamedMutation {
    name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonUnknown {
    origin: String,
}

#[cfg(test)]
mod tests {
    use crate::eval::effects::registry::SummaryRegistry;

    #[test]
    fn rejects_multiple_effect_kinds() {
        let error = SummaryRegistry::parse(
            r#"{
                "functions": {
                    "emit": {
                        "effects": [{
                            "host_effect": {"name": "emit"},
                            "observe_host": {"name": "emit"}
                        }]
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("exactly one effect kind"));
    }

    #[test]
    fn rejects_guards_for_disabled_outcomes() {
        let error = SummaryRegistry::parse(
            r#"{
                "functions": {
                    "emit": {
                        "effects": [{
                            "guard": "failure",
                            "host_effect": {"name": "emit"}
                        }],
                        "may_fail": false
                    }
                }
            }"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("infallible function"));
    }
}
