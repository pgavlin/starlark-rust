/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Experimental, crate-private predicated effect analysis.
//!
//! The analyzer intentionally targets optimized structured compiler IR and does not participate in
//! execution. Unsupported operations are represented conservatively and visibly.

#![allow(dead_code)] // Entry points remain test-only until the prototype vocabulary stabilizes.

pub(crate) mod analyze;
pub(crate) mod completion;
pub(crate) mod effect;
pub(crate) mod flow;
pub(crate) mod predicate;
mod report;

#[cfg(test)]
mod tests;
