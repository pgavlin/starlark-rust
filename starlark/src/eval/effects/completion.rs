/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use crate::eval::effects::predicate::PredicateId;

/// A noncontinuing result. Normal completion is represented separately by `Flow::normal`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AbruptCompletion<V, E, S> {
    Return { value: V, state: S },
    Failure { error: E, state: S },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuardedCompletion<V, E, S> {
    pub(crate) predicate: PredicateId,
    pub(crate) completion: AbruptCompletion<V, E, S>,
}
