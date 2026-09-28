/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use std::fmt::Write;

use crate::eval::effects::analyze::AnalysisResult;
use crate::eval::effects::completion::AbruptCompletion;

impl AnalysisResult {
    /// Stable, intentionally debug-oriented output for prototype golden tests.
    pub(crate) fn report(&self) -> String {
        let mut output = String::from("experimental effect analysis\neffects:\n");
        for effect in self.external_effects() {
            writeln!(
                output,
                "  {:?} when {}",
                effect.effect,
                self.predicates.render(effect.predicate)
            )
            .unwrap();
        }
        output.push_str("completions:\n");
        for normal in &self.flow.normal {
            writeln!(
                output,
                "  normal {:?} when {}",
                normal.value.expression,
                self.predicates.render(normal.predicate)
            )
            .unwrap();
        }
        for abrupt in &self.flow.abrupt {
            let completion = match &abrupt.completion {
                AbruptCompletion::Return { value, .. } => {
                    format!("return {:?}", value.expression)
                }
                AbruptCompletion::Failure { error, .. } => {
                    format!("failure {:?}", error.expression)
                }
            };
            writeln!(
                output,
                "  {completion} when {}",
                self.predicates.render(abrupt.predicate)
            )
            .unwrap();
        }
        output
    }
}
