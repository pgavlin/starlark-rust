/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

use crate::eval::compiler::args::ArgsCompiledValue;
use crate::eval::compiler::call::CallCompiled;
use crate::eval::compiler::expr::Builtin1;
use crate::eval::compiler::expr::ExprCompiled;
use crate::eval::compiler::expr::ExprLogicalBinOp;
use crate::eval::compiler::span::IrSpanned;
use crate::eval::compiler::stmt::StmtCompiled;
use crate::eval::compiler::stmt::StmtsCompiled;
use crate::eval::effects::analyze::AnalysisState;
use crate::eval::effects::analyze::Analyzer;
use crate::eval::effects::analyze::CallKey;
use crate::eval::effects::analyze::CallSummary;
use crate::eval::effects::analyze::GuardedSummaryEffect;
use crate::eval::effects::analyze::SummaryEffect;
use crate::eval::effects::analyze::SummaryGuard;
use crate::eval::effects::analyze::SummaryMutation;
use crate::eval::effects::completion::AbruptCompletion;
use crate::eval::effects::completion::GuardedCompletion;
use crate::eval::effects::effect::AbstractObject;
use crate::eval::effects::effect::Effect;
use crate::eval::effects::effect::RegionSet;
use crate::eval::effects::effect::SymbolicExpression;
use crate::eval::effects::effect::SymbolicValue;
use crate::eval::effects::flow::Flow;
use crate::eval::effects::flow::GuardedEffect;
use crate::eval::effects::flow::GuardedState;
use crate::eval::effects::predicate::Predicate;
use crate::eval::effects::predicate::PredicateAtom;
use crate::eval::effects::predicate::PredicateInterner;
use crate::eval::runtime::frame_span::FrameSpan;
use crate::eval::runtime::slots::LocalSlotId;
use crate::values::Value;

#[test]
fn predicate_interner_normalizes_boolean_structure() {
    let mut predicates = PredicateInterner::default();
    let a = predicates.atom(PredicateAtom::Opaque("a".to_owned()));
    let b = predicates.atom(PredicateAtom::Opaque("b".to_owned()));
    let not_a = predicates.not(a);

    assert_eq!(a, predicates.atom(PredicateAtom::Opaque("a".to_owned())));
    let nested = predicates.and([b, predicates.true_()]);
    assert_eq!(predicates.and([a, nested, b]), predicates.and([b, a]));
    assert_eq!(predicates.false_(), predicates.and([a, not_a]));
    assert_eq!(predicates.true_(), predicates.or([a, not_a]));
    assert_eq!(a, predicates.not(not_a));
    assert!(matches!(predicates.get(nested), Predicate::Atom(_)));
}

#[test]
fn branch_join_disjoins_reachability() {
    let mut predicates = PredicateInterner::default();
    let condition = predicates.atom(PredicateAtom::Opaque("condition".to_owned()));
    let not_condition = predicates.not(condition);
    let mut flow: Flow<u8, u8, (), ()> = Flow::default();
    flow.normal.push(GuardedState {
        predicate: condition,
        state: 0,
        value: 0,
    });
    flow.normal.push(GuardedState {
        predicate: not_condition,
        state: 0,
        value: 0,
    });

    flow.join_normal(&mut predicates);

    assert_eq!(1, flow.normal.len());
    assert_eq!(predicates.true_(), flow.normal[0].predicate);
}

#[test]
fn abrupt_completion_does_not_feed_sequence() {
    let mut predicates = PredicateInterner::default();
    let normal = predicates.atom(PredicateAtom::Normal("first".to_owned()));
    let fails = predicates.atom(PredicateAtom::Fails("first".to_owned()));
    let first: Flow<u8, u8, &'static str, &'static str> = Flow {
        effects: Vec::new(),
        normal: vec![GuardedState {
            predicate: normal,
            state: 0,
            value: 0,
        }],
        abrupt: vec![GuardedCompletion {
            predicate: fails,
            completion: AbruptCompletion::Failure {
                error: "error",
                state: 0,
            },
        }],
    };

    let result = first.sequence(|continuing| Flow {
        effects: vec![GuardedEffect {
            predicate: continuing.predicate,
            effect: "second",
        }],
        normal: vec![continuing],
        abrupt: Vec::new(),
    });

    assert_eq!(
        vec![normal],
        result
            .effects
            .iter()
            .map(|x| x.predicate)
            .collect::<Vec<_>>()
    );
    assert_eq!(1, result.abrupt.len());
    assert_eq!(fails, result.abrupt[0].predicate);
}

fn entry(analyzer: &mut Analyzer) -> GuardedState<AnalysisState, SymbolicValue> {
    GuardedState {
        predicate: analyzer.predicates().true_(),
        state: AnalysisState::for_parameters(1),
        value: SymbolicValue::none(),
    }
}

#[test]
fn pure_total_and_partial_summaries_have_no_effects() {
    let mut total_analyzer = Analyzer::default();
    let total_entry = entry(&mut total_analyzer);
    let total = total_analyzer.apply_summary(total_entry, &CallSummary::pure_total(), &[]);
    assert!(total.effects.is_empty());
    assert_eq!(1, total.normal.len());
    assert!(total.abrupt.is_empty());

    let mut partial_analyzer = Analyzer::default();
    let partial_entry = entry(&mut partial_analyzer);
    let partial = partial_analyzer.apply_summary(partial_entry, &CallSummary::pure_partial(), &[]);
    assert!(partial.effects.is_empty());
    assert_eq!(1, partial.normal.len());
    assert_eq!(1, partial.abrupt.len());
}

#[test]
fn summary_mutation_targets_input_and_preserves_outcome_correlation() {
    let mut analyzer = Analyzer::default();
    let input = SymbolicValue {
        expression: SymbolicExpression::Input(0),
        regions: RegionSet::one(AbstractObject::InputRoot(0)),
    };
    let value = SymbolicValue {
        expression: SymbolicExpression::Constant("1".to_owned()),
        regions: RegionSet::Empty,
    };
    let summary = CallSummary {
        effects: vec![GuardedSummaryEffect {
            guard: SummaryGuard::Normal,
            effect: SummaryEffect::MutateArgument {
                target: 0,
                mutation: SummaryMutation::AppendArgument(1),
            },
        }],
        may_return: true,
        may_fail: true,
    };
    let call_entry = entry(&mut analyzer);

    let result = analyzer.apply_summary(call_entry, &summary, &[input, value]);

    assert_eq!(1, result.effects.len());
    assert!(matches!(
        &result.effects[0].effect,
        Effect::Mutate {
            region: RegionSet::Objects(objects),
            ..
        } if objects.contains(&AbstractObject::InputRoot(0))
    ));
    assert!(
        analyzer
            .predicates()
            .render(result.effects[0].predicate)
            .contains("normal(op0)")
    );
    assert!(
        analyzer
            .predicates()
            .render(result.abrupt[0].predicate)
            .contains("fails(op0)")
    );
}

fn call(value: Value<'static>) -> IrSpanned<'static, ExprCompiled<'static>> {
    let span = FrameSpan::default();
    IrSpanned {
        span,
        node: ExprCompiled::Call(Box::new(IrSpanned {
            span,
            node: CallCompiled {
                fun: IrSpanned {
                    span,
                    node: ExprCompiled::Value(value),
                },
                args: ArgsCompiledValue::default(),
            },
        })),
    }
}

fn expression_statement(
    expression: IrSpanned<'static, ExprCompiled<'static>>,
) -> StmtsCompiled<'static> {
    StmtsCompiled::one(IrSpanned {
        span: expression.span,
        node: StmtCompiled::Expr(expression),
    })
}

fn host_summary(name: &str) -> CallSummary {
    CallSummary {
        effects: vec![GuardedSummaryEffect {
            guard: SummaryGuard::Entered,
            effect: SummaryEffect::HostEffect {
                name: name.to_owned(),
                arguments: Box::new([]),
            },
        }],
        may_return: true,
        may_fail: false,
    }
}

#[test]
fn compiler_ir_short_circuit_guards_right_operand() {
    for (operator, expected_guard) in [
        (ExprLogicalBinOp::And, "truth(Input(0))"),
        (ExprLogicalBinOp::Or, "NOT truth(Input(0))"),
    ] {
        let span = FrameSpan::default();
        let right = call(Value::new_none());
        let expression = IrSpanned {
            span,
            node: ExprCompiled::LogicalBinOp(
                operator,
                Box::new((
                    IrSpanned {
                        span,
                        node: ExprCompiled::Local(LocalSlotId(0)),
                    },
                    right,
                )),
            ),
        };
        let statements = expression_statement(expression);
        let mut analyzer = Analyzer::default();
        analyzer.add_summary(CallKey::Constant("None".to_owned()), host_summary("right"));

        let result = analyzer.analyze(&statements, 1);

        assert_eq!(1, result.external_effects().len());
        assert_eq!(
            expected_guard,
            result
                .predicates
                .render(result.external_effects()[0].predicate)
        );
    }
}

#[test]
fn compiler_ir_branch_continuation_uses_or() {
    let span = FrameSpan::default();
    let then_branch = expression_statement(call(Value::new_none()));
    let else_branch = expression_statement(call(Value::new_bool(false)));
    let mut statements = StmtsCompiled::one(IrSpanned {
        span,
        node: StmtCompiled::If(Box::new((
            IrSpanned {
                span,
                node: ExprCompiled::Local(LocalSlotId(0)),
            },
            then_branch,
            else_branch,
        ))),
    });
    statements.extend(expression_statement(call(Value::new_bool(true))));
    let mut analyzer = Analyzer::default();
    analyzer.add_summary(CallKey::Constant("None".to_owned()), host_summary("then"));
    analyzer.add_summary(CallKey::Constant("False".to_owned()), host_summary("else"));
    analyzer.add_summary(CallKey::Constant("True".to_owned()), host_summary("finish"));

    let result = analyzer.analyze(&statements, 1);
    let finish = result
        .external_effects()
        .into_iter()
        .find(
            |effect| matches!(&effect.effect, Effect::HostEffect { name, .. } if name == "finish"),
        )
        .unwrap();

    assert_eq!("true", result.predicates.render(finish.predicate));
}

#[test]
fn compiler_ir_failure_prevents_later_effect() {
    let mut statements = expression_statement(call(Value::new_none()));
    statements.extend(expression_statement(call(Value::new_bool(true))));
    let mut analyzer = Analyzer::default();
    analyzer.add_summary(
        CallKey::Constant("None".to_owned()),
        CallSummary::pure_partial(),
    );
    analyzer.add_summary(CallKey::Constant("True".to_owned()), host_summary("later"));

    let result = analyzer.analyze(&statements, 0);

    assert_eq!(1, result.external_effects().len());
    let guard = result
        .predicates
        .render(result.external_effects()[0].predicate);
    assert!(guard.contains("normal(op0)"), "guard was {guard}");
    assert!(result.flow.abrupt.iter().any(|completion| {
        result
            .predicates
            .render(completion.predicate)
            .contains("fails(op0)")
    }));
}

#[test]
fn mutation_before_failure_is_guarded_by_entry_not_normal() {
    let mut analyzer = Analyzer::default();
    let summary = CallSummary {
        effects: vec![GuardedSummaryEffect {
            guard: SummaryGuard::Entered,
            effect: SummaryEffect::MutateAll("before outcome".to_owned()),
        }],
        may_return: true,
        may_fail: true,
    };
    let call_entry = entry(&mut analyzer);
    let result = analyzer.apply_summary(call_entry, &summary, &[]);

    assert_eq!(analyzer.predicates().true_(), result.effects[0].predicate);
}

#[test]
fn unsupported_compiler_ir_widens_visibly_in_report() {
    let span = FrameSpan::default();
    let statements = expression_statement(IrSpanned {
        span,
        node: ExprCompiled::Builtin1(
            Builtin1::Minus,
            Box::new(IrSpanned {
                span,
                node: ExprCompiled::Local(LocalSlotId(0)),
            }),
        ),
    });

    let result = Analyzer::default().analyze(&statements, 1);

    assert_eq!(
        concat!(
            "experimental effect analysis\n",
            "effects:\n",
            "  UnknownEffect(\"unsupported unary operation\") when true\n",
            "  Mutate { region: AllObjects, mutation: Opaque(\"unknown mutation\") } when true\n",
            "completions:\n",
            "  normal None when normal(op0)\n",
            "  failure Unknown(\"error(op0)\") when fails(op0)\n",
        ),
        result.report(),
    );
}
