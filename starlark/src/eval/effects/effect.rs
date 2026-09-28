/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use std::collections::BTreeSet;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum AbstractObject {
    InputRoot(u32),
    CapturedRoot(u32),
    ModuleRoot(u32),
    Allocation(u32),
    Host(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RegionSet {
    Empty,
    Objects(BTreeSet<AbstractObject>),
    AllObjects,
}

impl RegionSet {
    pub(crate) fn one(object: AbstractObject) -> Self {
        Self::Objects(BTreeSet::from([object]))
    }

    pub(crate) fn join(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::AllObjects, _) | (_, Self::AllObjects) => Self::AllObjects,
            (Self::Empty, other) => other.clone(),
            (this, Self::Empty) => this.clone(),
            (Self::Objects(left), Self::Objects(right)) => {
                Self::Objects(left.union(right).cloned().collect())
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SymbolicExpression {
    None,
    Constant(String),
    Input(u32),
    Local(u32, u32),
    Captured(u32, u32),
    Module(u32, u32),
    Apply(String, Box<[SymbolicExpression]>),
    Arguments(Box<[SymbolicValue]>),
    Phi(Box<[SymbolicExpression]>),
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SymbolicValue {
    pub(crate) expression: SymbolicExpression,
    pub(crate) regions: RegionSet,
}

impl SymbolicValue {
    pub(crate) fn none() -> Self {
        Self {
            expression: SymbolicExpression::None,
            regions: RegionSet::Empty,
        }
    }

    pub(crate) fn unknown(origin: impl Into<String>) -> Self {
        Self {
            expression: SymbolicExpression::Unknown(origin.into()),
            regions: RegionSet::AllObjects,
        }
    }

    pub(crate) fn label(&self) -> String {
        format!("{:?}", self.expression)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Mutation {
    SetIndex {
        key: SymbolicValue,
        value: SymbolicValue,
    },
    SetAttribute {
        name: String,
        value: SymbolicValue,
    },
    Append(SymbolicValue),
    Opaque(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Effect {
    ObserveHost {
        name: String,
        arguments: Box<[SymbolicValue]>,
    },
    WriteModule {
        slot: u32,
        value: SymbolicValue,
    },
    Mutate {
        region: RegionSet,
        mutation: Mutation,
    },
    HostEffect {
        name: String,
        arguments: Box<[SymbolicValue]>,
    },
    UnknownEffect(String),
    /// Internal diagnostic event, projected out by `AnalysisResult::external_effects`.
    WriteLocal {
        slot: u32,
        value: SymbolicValue,
    },
    /// Internal diagnostic event, projected out by `AnalysisResult::external_effects`.
    WriteCaptured {
        slot: u32,
        value: SymbolicValue,
    },
}

impl Effect {
    pub(crate) fn externally_visible(&self) -> bool {
        !matches!(self, Self::WriteLocal { .. } | Self::WriteCaptured { .. })
    }
}
