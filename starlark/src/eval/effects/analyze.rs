/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use std::collections::BTreeMap;
use std::collections::HashMap;

use crate::eval::compiler::expr::Builtin1;
use crate::eval::compiler::expr::ExprCompiled;
use crate::eval::compiler::expr::ExprLogicalBinOp;
use crate::eval::compiler::span::IrSpanned;
use crate::eval::compiler::stmt::AssignCompiledValue;
use crate::eval::compiler::stmt::StmtCompiled;
use crate::eval::compiler::stmt::StmtsCompiled;
use crate::eval::effects::completion::AbruptCompletion;
use crate::eval::effects::completion::GuardedCompletion;
use crate::eval::effects::effect::AbstractObject;
use crate::eval::effects::effect::Effect;
use crate::eval::effects::effect::Mutation;
use crate::eval::effects::effect::RegionSet;
use crate::eval::effects::effect::SymbolicExpression;
use crate::eval::effects::effect::SymbolicValue;
use crate::eval::effects::flow::Flow;
use crate::eval::effects::flow::GuardedEffect;
use crate::eval::effects::flow::GuardedState;
use crate::eval::effects::predicate::PredicateAtom;
use crate::eval::effects::predicate::PredicateId;
use crate::eval::effects::predicate::PredicateInterner;

pub(crate) type AnalysisFlow = Flow<AnalysisState, SymbolicValue, Effect, SymbolicValue>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CallKey {
    Module(u32),
    Local(u32),
    Captured(u32),
    Method(String),
    Constant(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SummaryGuard {
    Entered,
    Normal,
    Failure,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SummaryMutation {
    AppendArgument(usize),
    Opaque(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum SummaryEffect {
    ObserveHost {
        name: String,
        arguments: Box<[usize]>,
    },
    MutateArgument {
        target: usize,
        mutation: SummaryMutation,
    },
    MutateAll(String),
    HostEffect {
        name: String,
        arguments: Box<[usize]>,
    },
    Unknown(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GuardedSummaryEffect {
    pub(crate) guard: SummaryGuard,
    pub(crate) effect: SummaryEffect,
}

/// A deliberately small, analyzer-owned call summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CallSummary {
    pub(crate) effects: Vec<GuardedSummaryEffect>,
    pub(crate) may_return: bool,
    pub(crate) may_fail: bool,
}

impl CallSummary {
    pub(crate) fn pure_total() -> Self {
        Self {
            effects: Vec::new(),
            may_return: true,
            may_fail: false,
        }
    }

    pub(crate) fn pure_partial() -> Self {
        Self {
            effects: Vec::new(),
            may_return: true,
            may_fail: true,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AnalysisState {
    locals: BTreeMap<u32, SymbolicValue>,
    captured: BTreeMap<u32, SymbolicValue>,
    modules: BTreeMap<u32, SymbolicValue>,
    versions: BTreeMap<(u8, u32), u32>,
}

impl AnalysisState {
    pub(crate) fn for_parameters(parameter_count: u32) -> Self {
        let mut result = Self::default();
        for parameter in 0..parameter_count {
            result.locals.insert(
                parameter,
                SymbolicValue {
                    expression: SymbolicExpression::Input(parameter),
                    regions: RegionSet::one(AbstractObject::InputRoot(parameter)),
                },
            );
        }
        result
    }

    fn next_version(&mut self, kind: u8, slot: u32) -> u32 {
        let version = self.versions.entry((kind, slot)).or_default();
        *version += 1;
        *version
    }
}

pub(crate) struct AnalysisResult {
    pub(crate) flow: AnalysisFlow,
    pub(crate) predicates: PredicateInterner,
}

impl AnalysisResult {
    /// Project internal slot writes while retaining occurrence order.
    pub(crate) fn external_effects(&self) -> Vec<&GuardedEffect<Effect>> {
        self.flow
            .effects
            .iter()
            .filter(|effect| effect.effect.externally_visible())
            .collect()
    }
}

/// Intraprocedural analysis of optimized structured compiler IR.
#[derive(Default)]
pub(crate) struct Analyzer {
    predicates: PredicateInterner,
    summaries: HashMap<CallKey, CallSummary>,
    next_operation: u32,
    next_allocation: u32,
}

impl Analyzer {
    pub(crate) fn add_summary(&mut self, key: CallKey, summary: CallSummary) {
        self.summaries.insert(key, summary);
    }

    pub(crate) fn predicates(&self) -> &PredicateInterner {
        &self.predicates
    }

    pub(crate) fn predicates_mut(&mut self) -> &mut PredicateInterner {
        &mut self.predicates
    }

    pub(crate) fn analyze(
        mut self,
        statements: &StmtsCompiled<'_>,
        parameter_count: u32,
    ) -> AnalysisResult {
        let entry = self.predicates.true_();
        let flow = AnalysisFlow::normal(
            entry,
            AnalysisState::for_parameters(parameter_count),
            SymbolicValue::none(),
        );
        let mut flow = self.analyze_statements(flow, statements);
        self.merge_effects(&mut flow);
        AnalysisResult {
            flow,
            predicates: self.predicates,
        }
    }

    fn bind(
        &mut self,
        flow: AnalysisFlow,
        mut next: impl FnMut(&mut Self, GuardedState<AnalysisState, SymbolicValue>) -> AnalysisFlow,
    ) -> AnalysisFlow {
        let mut result = AnalysisFlow {
            effects: flow.effects,
            normal: Vec::new(),
            abrupt: flow.abrupt,
        };
        for normal in flow.normal {
            result.append(next(self, normal));
        }
        result
    }

    fn analyze_statements(
        &mut self,
        mut flow: AnalysisFlow,
        statements: &StmtsCompiled<'_>,
    ) -> AnalysisFlow {
        for statement in statements.stmts() {
            flow = self.bind(flow, |this, input| {
                this.analyze_statement(input, &statement.node)
            });
        }
        flow
    }

    fn analyze_statement(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        statement: &StmtCompiled<'_>,
    ) -> AnalysisFlow {
        match statement {
            StmtCompiled::PossibleGc => {
                AnalysisFlow::normal(input.predicate, input.state, SymbolicValue::none())
            }
            StmtCompiled::Expr(expression) => {
                let mut flow = self.eval_expr(input, expression);
                for normal in &mut flow.normal {
                    normal.value = SymbolicValue::none();
                }
                flow
            }
            StmtCompiled::Return(expression) => {
                let flow = self.eval_expr(input, expression);
                let mut result = AnalysisFlow {
                    effects: flow.effects,
                    normal: Vec::new(),
                    abrupt: flow.abrupt,
                };
                result
                    .abrupt
                    .extend(flow.normal.into_iter().map(|normal| GuardedCompletion {
                        predicate: normal.predicate,
                        completion: AbruptCompletion::Return {
                            value: normal.value,
                            state: normal.state,
                        },
                    }));
                result
            }
            StmtCompiled::Assign(target, _ty, value) => {
                let value_flow = self.eval_expr(input, value);
                self.bind(value_flow, |this, normal| this.assign(normal, &target.node))
            }
            StmtCompiled::If(cond_then_else) => {
                let (condition, then_branch, else_branch) = &**cond_then_else;
                let condition_flow = self.eval_expr(input, condition);
                let constant = condition.is_pure_infallible_to_bool();
                self.bind(condition_flow, |this, normal| {
                    let truth = if let Some(constant) = constant {
                        if constant {
                            this.predicates.true_()
                        } else {
                            this.predicates.false_()
                        }
                    } else {
                        this.predicates
                            .atom(PredicateAtom::Truth(normal.value.label()))
                    };
                    let falsehood = this.predicates.not(truth);
                    let mut branches = AnalysisFlow::default();
                    if truth != this.predicates.false_() {
                        let guard = this.predicates.and([normal.predicate, truth]);
                        branches.append(this.analyze_statements(
                            AnalysisFlow::normal(
                                guard,
                                normal.state.clone(),
                                SymbolicValue::none(),
                            ),
                            then_branch,
                        ));
                    }
                    if falsehood != this.predicates.false_() {
                        let guard = this.predicates.and([normal.predicate, falsehood]);
                        branches.append(this.analyze_statements(
                            AnalysisFlow::normal(guard, normal.state, SymbolicValue::none()),
                            else_branch,
                        ));
                    }
                    branches.join_normal(&mut this.predicates);
                    branches
                })
            }
            // Loops and loop completions are intentionally outside this vertical slice. They are
            // represented visibly rather than being mistaken for pure operations.
            StmtCompiled::For(_) | StmtCompiled::Break | StmtCompiled::Continue => {
                self.unknown_operation(input, "unsupported control flow")
            }
            StmtCompiled::AssignModify(_, _, rhs) => {
                let rhs = self.eval_expr(input, rhs);
                self.bind(rhs, |this, normal| {
                    this.unknown_operation(normal, "unsupported augmented assignment")
                })
            }
        }
    }

    fn assign(
        &mut self,
        normal: GuardedState<AnalysisState, SymbolicValue>,
        target: &AssignCompiledValue<'_>,
    ) -> AnalysisFlow {
        let GuardedState {
            predicate,
            mut state,
            value,
        } = normal;
        let effect = match target {
            AssignCompiledValue::Local(slot) => {
                let version = state.next_version(0, slot.0);
                let mut stored = value.clone();
                stored.expression = SymbolicExpression::Local(slot.0, version);
                stored.regions = value.regions.clone();
                state.locals.insert(slot.0, stored);
                Effect::WriteLocal {
                    slot: slot.0,
                    value,
                }
            }
            AssignCompiledValue::LocalCaptured(slot) => {
                let version = state.next_version(1, slot.0);
                let mut stored = value.clone();
                stored.expression = SymbolicExpression::Captured(slot.0, version);
                state.captured.insert(slot.0, stored);
                Effect::WriteCaptured {
                    slot: slot.0,
                    value,
                }
            }
            AssignCompiledValue::Module(slot, _) => {
                let version = state.next_version(2, slot.0);
                let mut stored = value.clone();
                stored.expression = SymbolicExpression::Module(slot.0, version);
                state.modules.insert(slot.0, stored);
                Effect::WriteModule {
                    slot: slot.0,
                    value,
                }
            }
            AssignCompiledValue::Dot(_, _)
            | AssignCompiledValue::Index(_, _)
            | AssignCompiledValue::Tuple(_) => {
                return self.unknown_operation(
                    GuardedState {
                        predicate,
                        state,
                        value,
                    },
                    "unsupported assignment target",
                );
            }
        };
        AnalysisFlow {
            effects: vec![GuardedEffect { predicate, effect }],
            normal: vec![GuardedState {
                predicate,
                state,
                value: SymbolicValue::none(),
            }],
            abrupt: Vec::new(),
        }
    }

    fn eval_expr(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        expression: &IrSpanned<'_, ExprCompiled<'_>>,
    ) -> AnalysisFlow {
        match &expression.node {
            ExprCompiled::Value(value) => AnalysisFlow::normal(
                input.predicate,
                input.state,
                SymbolicValue {
                    expression: SymbolicExpression::Constant(value.to_repr()),
                    regions: RegionSet::Empty,
                },
            ),
            ExprCompiled::Local(slot) => match input.state.locals.get(&slot.0).cloned() {
                Some(value) => AnalysisFlow::normal(input.predicate, input.state, value),
                None => self.failing_read(input, format!("local[{}]", slot.0)),
            },
            ExprCompiled::LocalCaptured(slot) => {
                let value = input
                    .state
                    .captured
                    .get(&slot.0)
                    .cloned()
                    .unwrap_or_else(|| SymbolicValue {
                        expression: SymbolicExpression::Captured(slot.0, 0),
                        regions: RegionSet::one(AbstractObject::CapturedRoot(slot.0)),
                    });
                AnalysisFlow::normal(input.predicate, input.state, value)
            }
            ExprCompiled::Module(slot) => {
                let value = input
                    .state
                    .modules
                    .get(&slot.0)
                    .cloned()
                    .unwrap_or_else(|| SymbolicValue {
                        expression: SymbolicExpression::Module(slot.0, 0),
                        regions: RegionSet::one(AbstractObject::ModuleRoot(slot.0)),
                    });
                // Module reads can fail when the slot is unbound.
                self.partial_value(input.predicate, input.state, value, "module read")
            }
            ExprCompiled::Tuple(items) | ExprCompiled::List(items) => {
                self.eval_sequence(input, items, "construct", true)
            }
            ExprCompiled::Dict(items) => {
                let expressions: Vec<_> =
                    items.iter().flat_map(|(key, value)| [key, value]).collect();
                self.eval_sequence_refs(input, &expressions, "dict", true)
            }
            ExprCompiled::Seq(pair) => {
                let (left, right) = &**pair;
                let left = self.eval_expr(input, left);
                self.bind(left, |this, normal| this.eval_expr(normal, right))
            }
            ExprCompiled::If(cond_then_else) => {
                let (condition, then_expr, else_expr) = &**cond_then_else;
                let condition_flow = self.eval_expr(input, condition);
                let constant = condition.is_pure_infallible_to_bool();
                self.bind(condition_flow, |this, normal| {
                    let truth = match constant {
                        Some(true) => this.predicates.true_(),
                        Some(false) => this.predicates.false_(),
                        None => this
                            .predicates
                            .atom(PredicateAtom::Truth(normal.value.label())),
                    };
                    let falsehood = this.predicates.not(truth);
                    let mut result = AnalysisFlow::default();
                    if truth != this.predicates.false_() {
                        let guard = this.predicates.and([normal.predicate, truth]);
                        result.append(this.eval_expr(
                            GuardedState {
                                predicate: guard,
                                state: normal.state.clone(),
                                value: SymbolicValue::none(),
                            },
                            then_expr,
                        ));
                    }
                    if falsehood != this.predicates.false_() {
                        let guard = this.predicates.and([normal.predicate, falsehood]);
                        result.append(this.eval_expr(
                            GuardedState {
                                predicate: guard,
                                state: normal.state,
                                value: SymbolicValue::none(),
                            },
                            else_expr,
                        ));
                    }
                    result.join_normal(&mut this.predicates);
                    result
                })
            }
            ExprCompiled::LogicalBinOp(operator, pair) => {
                let (left, right) = &**pair;
                let left_flow = self.eval_expr(input, left);
                self.bind(left_flow, |this, normal| {
                    let truth = this
                        .predicates
                        .atom(PredicateAtom::Truth(normal.value.label()));
                    let evaluate_right = match operator {
                        ExprLogicalBinOp::And => truth,
                        ExprLogicalBinOp::Or => this.predicates.not(truth),
                    };
                    let short_circuit = this.predicates.not(evaluate_right);
                    let mut result = AnalysisFlow::default();
                    let right_guard = this.predicates.and([normal.predicate, evaluate_right]);
                    if right_guard != this.predicates.false_() {
                        result.append(this.eval_expr(
                            GuardedState {
                                predicate: right_guard,
                                state: normal.state.clone(),
                                value: SymbolicValue::none(),
                            },
                            right,
                        ));
                    }
                    let short_guard = this.predicates.and([normal.predicate, short_circuit]);
                    if short_guard != this.predicates.false_() {
                        result.normal.push(GuardedState {
                            predicate: short_guard,
                            state: normal.state,
                            value: normal.value,
                        });
                    }
                    result.join_normal(&mut this.predicates);
                    result
                })
            }
            ExprCompiled::Builtin1(Builtin1::Not | Builtin1::TypeIs(_), operand) => {
                let operand = self.eval_expr(input, operand);
                self.bind(operand, |_, normal| {
                    AnalysisFlow::normal(
                        normal.predicate,
                        normal.state,
                        SymbolicValue {
                            expression: SymbolicExpression::Apply(
                                "unary".to_owned(),
                                vec![normal.value.expression].into_boxed_slice(),
                            ),
                            regions: RegionSet::Empty,
                        },
                    )
                })
            }
            ExprCompiled::Call(call) => self.eval_call(input, &call.node),
            ExprCompiled::Def(_) => AnalysisFlow::normal(
                input.predicate,
                input.state,
                SymbolicValue {
                    expression: SymbolicExpression::Unknown("function".to_owned()),
                    regions: RegionSet::Empty,
                },
            ),
            ExprCompiled::Builtin1(_, operand) => {
                let operand = self.eval_expr(input, operand);
                self.bind(operand, |this, normal| {
                    this.unknown_operation(normal, "unsupported unary operation")
                })
            }
            ExprCompiled::Builtin2(_, pair) => {
                let (left, right) = &**pair;
                let left = self.eval_expr(input, left);
                let both = self.bind(left, |this, normal| this.eval_expr(normal, right));
                self.bind(both, |this, normal| {
                    this.unknown_operation(normal, "unsupported binary operation")
                })
            }
            ExprCompiled::Slice(parts) => {
                let (value, start, stop, step) = &**parts;
                let mut expressions = vec![value];
                expressions.extend(start.iter());
                expressions.extend(stop.iter());
                expressions.extend(step.iter());
                let values = self.eval_sequence_refs(input, &expressions, "slice arguments", false);
                self.bind(values, |this, normal| {
                    this.unknown_operation(normal, "unsupported slice")
                })
            }
            ExprCompiled::Index2(parts) => {
                let (value, first, second) = &**parts;
                let values = self.eval_sequence_refs(
                    input,
                    &[value, first, second],
                    "index arguments",
                    false,
                );
                self.bind(values, |this, normal| {
                    this.unknown_operation(normal, "unsupported index")
                })
            }
            ExprCompiled::Compr(_) => self.unknown_operation(input, "unsupported comprehension"),
        }
    }

    fn eval_sequence(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        expressions: &[IrSpanned<'_, ExprCompiled<'_>>],
        name: &str,
        allocate: bool,
    ) -> AnalysisFlow {
        let refs: Vec<_> = expressions.iter().collect();
        self.eval_sequence_refs(input, &refs, name, allocate)
    }

    fn eval_sequence_refs(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        expressions: &[&IrSpanned<'_, ExprCompiled<'_>>],
        name: &str,
        allocate: bool,
    ) -> AnalysisFlow {
        let mut flow = AnalysisFlow::normal(
            input.predicate,
            input.state,
            SymbolicValue {
                expression: SymbolicExpression::Apply(name.to_owned(), Box::new([])),
                regions: RegionSet::Empty,
            },
        );
        for expression in expressions {
            flow = self.bind(flow, |this, normal| this.eval_expr(normal, expression));
        }
        if allocate {
            let allocation = self.next_allocation;
            self.next_allocation += 1;
            for normal in &mut flow.normal {
                normal.value = SymbolicValue {
                    expression: SymbolicExpression::Apply(name.to_owned(), Box::new([])),
                    regions: RegionSet::one(AbstractObject::Allocation(allocation)),
                };
            }
        }
        flow
    }

    fn eval_call(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        call: &crate::eval::compiler::call::CallCompiled<'_>,
    ) -> AnalysisFlow {
        let (key, mut flow, receiver) = if let Some((receiver, name, _)) = call.method() {
            (
                CallKey::Method(name.as_str().to_owned()),
                self.eval_expr(input, receiver),
                true,
            )
        } else {
            let key = match &call.fun.node {
                ExprCompiled::Module(slot) => CallKey::Module(slot.0),
                ExprCompiled::Local(slot) => CallKey::Local(slot.0),
                ExprCompiled::LocalCaptured(slot) => CallKey::Captured(slot.0),
                ExprCompiled::Value(value) => CallKey::Constant(value.to_repr()),
                _ => CallKey::Constant("<dynamic>".to_owned()),
            };
            (key, self.eval_expr(input, &call.fun), false)
        };

        // The accumulating value is an internal argument pack. A method receiver is argument zero.
        for normal in &mut flow.normal {
            let arguments = if receiver {
                vec![normal.value.clone()]
            } else {
                Vec::new()
            };
            normal.value = SymbolicValue {
                expression: SymbolicExpression::Arguments(arguments.into_boxed_slice()),
                regions: RegionSet::Empty,
            };
        }
        for argument in call.args.arg_exprs() {
            flow = self.bind(flow, |this, normal| {
                let previous = match normal.value.expression {
                    SymbolicExpression::Arguments(arguments) => arguments.into_vec(),
                    _ => Vec::new(),
                };
                let argument_flow = this.eval_expr(
                    GuardedState {
                        predicate: normal.predicate,
                        state: normal.state,
                        value: SymbolicValue::none(),
                    },
                    argument,
                );
                let mut argument_flow = argument_flow;
                for result in &mut argument_flow.normal {
                    let mut arguments = previous.clone();
                    arguments.push(result.value.clone());
                    result.value = SymbolicValue {
                        expression: SymbolicExpression::Arguments(arguments.into_boxed_slice()),
                        regions: RegionSet::Empty,
                    };
                }
                argument_flow
            });
        }

        let summary = self
            .summaries
            .get(&key)
            .cloned()
            .unwrap_or_else(|| CallSummary {
                effects: vec![
                    GuardedSummaryEffect {
                        guard: SummaryGuard::Entered,
                        effect: SummaryEffect::Unknown(format!("call {key:?}")),
                    },
                    GuardedSummaryEffect {
                        guard: SummaryGuard::Entered,
                        effect: SummaryEffect::MutateAll("unknown mutation".to_owned()),
                    },
                ],
                may_return: true,
                may_fail: true,
            });
        self.bind(flow, |this, mut normal| {
            let arguments =
                match std::mem::replace(&mut normal.value.expression, SymbolicExpression::None) {
                    SymbolicExpression::Arguments(arguments) => arguments.into_vec(),
                    _ => Vec::new(),
                };
            this.apply_summary(normal, &summary, &arguments)
        })
    }

    pub(crate) fn apply_summary(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        summary: &CallSummary,
        arguments: &[SymbolicValue],
    ) -> AnalysisFlow {
        let operation = format!("op{}", self.next_operation);
        self.next_operation += 1;
        let normal_atom = summary.may_fail.then(|| {
            self.predicates
                .atom(PredicateAtom::Normal(operation.clone()))
        });
        let failure_atom = summary.may_fail.then(|| {
            self.predicates
                .atom(PredicateAtom::Fails(operation.clone()))
        });
        let normal_guard = if summary.may_return {
            normal_atom
                .map(|outcome| self.predicates.and([input.predicate, outcome]))
                .unwrap_or(input.predicate)
        } else {
            self.predicates.false_()
        };
        let failure_guard =
            failure_atom.map(|outcome| self.predicates.and([input.predicate, outcome]));

        let mut flow = AnalysisFlow::default();
        for effect in &summary.effects {
            let predicate = match effect.guard {
                SummaryGuard::Entered => input.predicate,
                SummaryGuard::Normal => normal_guard,
                SummaryGuard::Failure => failure_guard.unwrap_or(self.predicates.false_()),
            };
            if predicate != self.predicates.false_() {
                flow.effects.push(GuardedEffect {
                    predicate,
                    effect: self.instantiate_effect(&effect.effect, arguments),
                });
            }
        }
        if summary.may_return {
            flow.normal.push(GuardedState {
                predicate: normal_guard,
                state: input.state.clone(),
                value: SymbolicValue::unknown(format!("result({operation})")),
            });
        }
        if let Some(predicate) = failure_guard {
            flow.abrupt.push(GuardedCompletion {
                predicate,
                completion: AbruptCompletion::Failure {
                    error: SymbolicValue::unknown(format!("error({operation})")),
                    state: input.state,
                },
            });
        }
        flow
    }

    fn instantiate_effect(&self, effect: &SummaryEffect, arguments: &[SymbolicValue]) -> Effect {
        let argument = |index: usize| {
            arguments
                .get(index)
                .cloned()
                .unwrap_or_else(|| SymbolicValue::unknown(format!("argument {index}")))
        };
        match effect {
            SummaryEffect::ObserveHost { name, arguments } => Effect::ObserveHost {
                name: name.clone(),
                arguments: arguments.iter().map(|index| argument(*index)).collect(),
            },
            SummaryEffect::MutateArgument { target, mutation } => Effect::Mutate {
                region: argument(*target).regions,
                mutation: match mutation {
                    SummaryMutation::AppendArgument(index) => Mutation::Append(argument(*index)),
                    SummaryMutation::Opaque(name) => Mutation::Opaque(name.clone()),
                },
            },
            SummaryEffect::MutateAll(name) => Effect::Mutate {
                region: RegionSet::AllObjects,
                mutation: Mutation::Opaque(name.clone()),
            },
            SummaryEffect::HostEffect { name, arguments } => Effect::HostEffect {
                name: name.clone(),
                arguments: arguments.iter().map(|index| argument(*index)).collect(),
            },
            SummaryEffect::Unknown(origin) => Effect::UnknownEffect(origin.clone()),
        }
    }

    fn unknown_operation(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        origin: &str,
    ) -> AnalysisFlow {
        let summary = CallSummary {
            effects: vec![
                GuardedSummaryEffect {
                    guard: SummaryGuard::Entered,
                    effect: SummaryEffect::Unknown(origin.to_owned()),
                },
                GuardedSummaryEffect {
                    guard: SummaryGuard::Entered,
                    effect: SummaryEffect::MutateAll("unknown mutation".to_owned()),
                },
            ],
            may_return: true,
            may_fail: true,
        };
        self.apply_summary(input, &summary, &[])
    }

    fn failing_read(
        &mut self,
        input: GuardedState<AnalysisState, SymbolicValue>,
        origin: String,
    ) -> AnalysisFlow {
        self.partial_value(
            input.predicate,
            input.state,
            SymbolicValue::unknown(origin.clone()),
            &origin,
        )
    }

    fn partial_value(
        &mut self,
        predicate: PredicateId,
        state: AnalysisState,
        value: SymbolicValue,
        origin: &str,
    ) -> AnalysisFlow {
        let operation = format!("{}#{}", origin, self.next_operation);
        self.next_operation += 1;
        let normal = self
            .predicates
            .atom(PredicateAtom::Normal(operation.clone()));
        let fails = self.predicates.atom(PredicateAtom::Fails(operation));
        AnalysisFlow {
            effects: Vec::new(),
            normal: vec![GuardedState {
                predicate: self.predicates.and([predicate, normal]),
                state: state.clone(),
                value,
            }],
            abrupt: vec![GuardedCompletion {
                predicate: self.predicates.and([predicate, fails]),
                completion: AbruptCompletion::Failure {
                    error: SymbolicValue::unknown(origin),
                    state,
                },
            }],
        }
    }

    fn merge_effects(&mut self, flow: &mut AnalysisFlow) {
        let mut merged: Vec<GuardedEffect<Effect>> = Vec::new();
        for incoming in flow.effects.drain(..) {
            if let Some(existing) = merged.iter_mut().find(|x| x.effect == incoming.effect) {
                existing.predicate = self.predicates.or([existing.predicate, incoming.predicate]);
            } else {
                merged.push(incoming);
            }
        }
        flow.effects = merged;
    }
}
