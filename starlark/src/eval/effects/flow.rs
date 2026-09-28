/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use crate::eval::effects::completion::GuardedCompletion;
use crate::eval::effects::predicate::PredicateId;
use crate::eval::effects::predicate::PredicateInterner;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuardedEffect<E> {
    pub(crate) predicate: PredicateId,
    pub(crate) effect: E,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuardedState<S, V> {
    pub(crate) predicate: PredicateId,
    pub(crate) state: S,
    pub(crate) value: V,
}

/// Results of structured analysis. Only entries in `normal` feed the next operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Flow<S, V, E, Error> {
    pub(crate) effects: Vec<GuardedEffect<E>>,
    pub(crate) normal: Vec<GuardedState<S, V>>,
    pub(crate) abrupt: Vec<GuardedCompletion<V, Error, S>>,
}

impl<S, V, E, Error> Default for Flow<S, V, E, Error> {
    fn default() -> Self {
        Self {
            effects: Vec::new(),
            normal: Vec::new(),
            abrupt: Vec::new(),
        }
    }
}

impl<S, V, E, Error> Flow<S, V, E, Error> {
    pub(crate) fn normal(predicate: PredicateId, state: S, value: V) -> Self {
        Self {
            normal: vec![GuardedState {
                predicate,
                state,
                value,
            }],
            ..Self::default()
        }
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        self.effects.append(&mut other.effects);
        self.normal.append(&mut other.normal);
        self.abrupt.append(&mut other.abrupt);
    }

    /// Monadic sequencing for guarded flow. Abrupt paths from the left are preserved and never
    /// passed to `next`.
    pub(crate) fn sequence(self, mut next: impl FnMut(GuardedState<S, V>) -> Self) -> Self {
        let mut result = Self {
            effects: self.effects,
            normal: Vec::new(),
            abrupt: self.abrupt,
        };
        for normal in self.normal {
            result.append(next(normal));
        }
        result
    }

    /// Coalesce identical continuing states/values. Alternative reachability is disjoined.
    pub(crate) fn join_normal(&mut self, predicates: &mut PredicateInterner)
    where
        S: Eq,
        V: Eq,
    {
        let mut joined: Vec<GuardedState<S, V>> = Vec::new();
        for incoming in self.normal.drain(..) {
            if let Some(existing) = joined
                .iter_mut()
                .find(|x| x.state == incoming.state && x.value == incoming.value)
            {
                existing.predicate = predicates.or([existing.predicate, incoming.predicate]);
            } else {
                joined.push(incoming);
            }
        }
        self.normal = joined;
    }
}
