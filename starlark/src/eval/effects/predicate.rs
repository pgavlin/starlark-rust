/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use std::collections::HashMap;
use std::fmt;
use std::fmt::Write;

/// An interned predicate. IDs are meaningful only in the interner that created them.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct PredicateId(u32);

/// Opaque facts supplied by the structured analyzer.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum PredicateAtom {
    Truth(String),
    Normal(String),
    Fails(String),
    Opaque(String),
}

impl fmt::Display for PredicateAtom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truth(value) => write!(f, "truth({value})"),
            Self::Normal(operation) => write!(f, "normal({operation})"),
            Self::Fails(operation) => write!(f, "fails({operation})"),
            Self::Opaque(atom) => f.write_str(atom),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum Predicate {
    True,
    False,
    Atom(PredicateAtom),
    Not(PredicateId),
    And(Box<[PredicateId]>),
    Or(Box<[PredicateId]>),
}

/// Hash-conses predicates and performs deliberately cheap Boolean normalization.
pub(crate) struct PredicateInterner {
    nodes: Vec<Predicate>,
    ids: HashMap<Predicate, PredicateId>,
}

impl Default for PredicateInterner {
    fn default() -> Self {
        let mut result = Self {
            nodes: Vec::new(),
            ids: HashMap::new(),
        };
        assert_eq!(PredicateId(0), result.intern(Predicate::True));
        assert_eq!(PredicateId(1), result.intern(Predicate::False));
        result
    }
}

impl PredicateInterner {
    pub(crate) fn true_(&self) -> PredicateId {
        PredicateId(0)
    }

    pub(crate) fn false_(&self) -> PredicateId {
        PredicateId(1)
    }

    pub(crate) fn get(&self, id: PredicateId) -> &Predicate {
        &self.nodes[id.0 as usize]
    }

    fn intern(&mut self, predicate: Predicate) -> PredicateId {
        if let Some(id) = self.ids.get(&predicate) {
            return *id;
        }
        let id = PredicateId(self.nodes.len().try_into().expect("predicate ID overflow"));
        self.nodes.push(predicate.clone());
        self.ids.insert(predicate, id);
        id
    }

    pub(crate) fn atom(&mut self, atom: PredicateAtom) -> PredicateId {
        self.intern(Predicate::Atom(atom))
    }

    pub(crate) fn not(&mut self, predicate: PredicateId) -> PredicateId {
        match self.get(predicate) {
            Predicate::True => self.false_(),
            Predicate::False => self.true_(),
            Predicate::Not(inner) => *inner,
            _ => self.intern(Predicate::Not(predicate)),
        }
    }

    pub(crate) fn and<I>(&mut self, predicates: I) -> PredicateId
    where
        I: IntoIterator<Item = PredicateId>,
    {
        self.associative(predicates, true)
    }

    pub(crate) fn or<I>(&mut self, predicates: I) -> PredicateId
    where
        I: IntoIterator<Item = PredicateId>,
    {
        self.associative(predicates, false)
    }

    fn associative<I>(&mut self, predicates: I, is_and: bool) -> PredicateId
    where
        I: IntoIterator<Item = PredicateId>,
    {
        let identity = if is_and { self.true_() } else { self.false_() };
        let absorbing = if is_and { self.false_() } else { self.true_() };
        let mut operands = Vec::new();
        for predicate in predicates {
            if predicate == absorbing {
                return absorbing;
            }
            if predicate == identity {
                continue;
            }
            match (is_and, self.get(predicate)) {
                (true, Predicate::And(nested)) | (false, Predicate::Or(nested)) => {
                    operands.extend_from_slice(nested);
                }
                _ => operands.push(predicate),
            }
        }
        operands.sort_unstable();
        operands.dedup();
        for &operand in &operands {
            let complement = match self.get(operand) {
                Predicate::Not(inner) => *inner,
                _ => self.not(operand),
            };
            if operands.binary_search(&complement).is_ok() {
                return absorbing;
            }
        }
        match operands.as_slice() {
            [] => identity,
            [only] => *only,
            _ if is_and => self.intern(Predicate::And(operands.into_boxed_slice())),
            _ => self.intern(Predicate::Or(operands.into_boxed_slice())),
        }
    }

    pub(crate) fn render(&self, predicate: PredicateId) -> String {
        fn go(interner: &PredicateInterner, id: PredicateId, parent: u8, out: &mut String) {
            let (precedence, separator) = match interner.get(id) {
                Predicate::Or(_) => (1, " OR "),
                Predicate::And(_) => (2, " AND "),
                Predicate::Not(_) => (3, ""),
                _ => (4, ""),
            };
            let parens = precedence < parent;
            if parens {
                out.push('(');
            }
            match interner.get(id) {
                Predicate::True => out.push_str("true"),
                Predicate::False => out.push_str("false"),
                Predicate::Atom(atom) => write!(out, "{atom}").unwrap(),
                Predicate::Not(inner) => {
                    out.push_str("NOT ");
                    go(interner, *inner, precedence, out);
                }
                Predicate::And(operands) | Predicate::Or(operands) => {
                    for (index, operand) in operands.iter().enumerate() {
                        if index != 0 {
                            out.push_str(separator);
                        }
                        go(interner, *operand, precedence, out);
                    }
                }
            }
            if parens {
                out.push(')');
            }
        }

        let mut result = String::new();
        go(self, predicate, 0, &mut result);
        result
    }
}
