/*
 * Copyright 2026 The Starlark in Rust Authors.
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Source-to-structured-IR harness for the experimental analyzer.

use std::collections::HashMap;
use std::fmt::Write;

use dupe::Dupe;
use starlark_syntax::syntax::ast::StmtP;
use starlark_syntax::syntax::module::AstModuleFields;
use starlark_syntax::syntax::top_level_stmts::top_level_stmts;

use crate::environment::Globals;
use crate::environment::Module;
use crate::eval::Evaluator;
use crate::eval::compiler::Compiler;
use crate::eval::compiler::def::DefCompiled;
use crate::eval::compiler::expr::ExprCompiled;
use crate::eval::compiler::scope::ModuleScopes;
use crate::eval::compiler::scope::ScopeId;
use crate::eval::compiler::scope::scope_resolver_globals::ScopeResolverGlobals;
use crate::eval::compiler::stmt::StmtCompiled;
use crate::eval::compiler::stmt::StmtsCompiled;
use crate::eval::effects::analyze::Analyzer;
use crate::eval::effects::analyze::CallKey;
use crate::eval::effects::analyze::CallSummary;
use crate::eval::effects::registry::SummaryRegistry;
use crate::syntax::AstModule;
use crate::values::Value;
use crate::values::any::StarlarkAny;

#[derive(Debug, thiserror::Error)]
enum SourceAnalysisError {
    #[error("load statements are not supported by the experimental effects CLI")]
    Load,
}

/// Parse-independent source harness used by the CLI and source-level tests.
pub(crate) fn analyze_source(
    ast: AstModule,
    globals: &Globals,
    registry: &SummaryRegistry,
) -> crate::Result<String> {
    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        compile_and_analyze(&mut eval, ast, globals, registry)
    })
}

fn compile_and_analyze(
    eval: &mut Evaluator<'_, '_, '_>,
    ast: AstModule,
    globals: &Globals,
    registry: &SummaryRegistry,
) -> crate::Result<String> {
    let (codemap, statement, dialect, typecheck) = ast.into_parts();
    let module_env = eval.module_env;
    module_env.heaps().frozen_heap(|fh, edge, seal_edge| {
        let configured: Vec<_> = registry.functions().collect();
        let extra_globals: HashMap<String, Value<'_>> = configured
            .iter()
            .map(|(name, _)| {
                let marker = format!("__effects_host_function__:{name}");
                ((*name).to_owned(), fh.alloc_str(&marker).to_value())
            })
            .collect();
        let call_summaries: Vec<_> = configured
            .into_iter()
            .map(|(name, summary)| {
                let value = extra_globals
                    .get(name)
                    .expect("global allocated for every configured summary");
                (CallKey::Constant(value.to_repr()), summary)
            })
            .collect();
        let codemap = fh.alloc_simple_typed(StarlarkAny::new(codemap.dupe()));
        let ModuleScopes {
            cst,
            module_slot_count,
            scope_data,
            top_level_stmt_count,
        } = globals.data().by_ref_with_reconstructor(|globals, r| {
            ModuleScopes::check_module_err(
                module_env.mutable_names(),
                fh,
                edge,
                &HashMap::new(),
                statement,
                ScopeResolverGlobals {
                    globals: Some((globals, r.frozen_edge(fh))),
                    extra_globals: Some(&extra_globals),
                },
                codemap,
                &dialect,
            )
        })?;

        if top_level_stmts(&cst)
            .iter()
            .any(|statement| matches!(statement.node, StmtP::Load(_)))
        {
            return Err(crate::Error::new_other(SourceAnalysisError::Load));
        }

        module_env.slots().ensure_slots(module_slot_count);
        let globals = fh.alloc_simple_typed(StarlarkAny::new(globals.dupe()));
        let mut compiler = Compiler {
            scope_data,
            locals: Vec::new(),
            globals,
            codemap,
            eval,
            fh,
            edge,
            seal_edge,
            check_types: dialect.enable_types == crate::syntax::DialectTypes::Enable,
            top_level_stmt_count,
            typecheck,
            local_as_values: Vec::new(),
        };
        compiler.enter_scope(ScopeId::module());
        let statements = compiler
            .stmt(&cst, false)
            .map_err(|error| error.into_eval_exception().into_error())?;
        compiler.exit_scope();

        Ok(report_program(&statements, &call_summaries))
    })
}

fn analyzer(summaries: &[(CallKey, CallSummary)]) -> Analyzer {
    let mut analyzer = Analyzer::default();
    for (key, summary) in summaries {
        analyzer.add_summary(key.clone(), summary.clone());
    }
    analyzer
}

fn report_program(statements: &StmtsCompiled<'_>, summaries: &[(CallKey, CallSummary)]) -> String {
    let mut output = String::new();
    output.push_str("== module ==\n");
    output.push_str(&analyzer(summaries).analyze(statements, 0).report());
    report_functions(statements, "", summaries, &mut output);
    output
}

fn report_functions(
    statements: &StmtsCompiled<'_>,
    parent: &str,
    summaries: &[(CallKey, CallSummary)],
    output: &mut String,
) {
    for statement in statements.stmts() {
        match &statement.node {
            StmtCompiled::Assign(_, _, expression) => {
                if let ExprCompiled::Def(def) = &expression.node {
                    report_function(def, parent, summaries, output);
                }
            }
            StmtCompiled::If(branches) => {
                let (_, then_branch, else_branch) = &**branches;
                report_functions(then_branch, parent, summaries, output);
                report_functions(else_branch, parent, summaries, output);
            }
            _ => {}
        }
    }
}

fn report_function(
    def: &DefCompiled<'_>,
    parent: &str,
    summaries: &[(CallKey, CallSummary)],
    output: &mut String,
) {
    let name = if parent.is_empty() {
        def.name().to_owned()
    } else {
        format!("{parent}.{}", def.name())
    };
    writeln!(output, "== function {name} ==").unwrap();
    output.push_str(
        &analyzer(summaries)
            .analyze(def.body(), def.parameter_count())
            .report(),
    );
    report_functions(def.body(), &name, summaries, output);
}

#[cfg(test)]
mod tests {
    use crate::environment::Globals;
    use crate::eval::effects::registry::SummaryRegistry;
    use crate::eval::effects::source::analyze_source;
    use crate::syntax::AstModule;
    use crate::syntax::Dialect;

    fn analyze(source: &str) -> String {
        let ast = AstModule::parse("effects.star", source.to_owned(), &Dialect::Standard).unwrap();
        analyze_source(ast, &Globals::standard(), &SummaryRegistry::default()).unwrap()
    }

    fn analyze_with_registry(source: &str, registry: &str) -> String {
        let ast = AstModule::parse("effects.star", source.to_owned(), &Dialect::Standard).unwrap();
        let registry = SummaryRegistry::parse(registry).unwrap();
        analyze_source(ast, &Globals::standard(), &registry).unwrap()
    }

    #[test]
    fn source_module_writes_propagate_regions_and_read_failure() {
        let report = analyze("xs = []\nalias = xs\n");

        assert!(report.contains(
            "WriteModule { slot: 0, value: SymbolicValue { expression: Apply(\"construct\", []), \
             regions: Objects({Allocation(0)}) } } when true"
        ));
        assert!(report.contains(
            "WriteModule { slot: 1, value: SymbolicValue { expression: Module(0, 1), regions: \
             Objects({Allocation(0)}) } } when normal(module read#0)"
        ));
        assert!(report.contains("failure Unknown(\"module read\") when fails(module read#0)"));
    }

    #[test]
    fn source_function_reports_local_assignment_and_return() {
        let report = analyze("def identity(x):\n  y = x\n  return y\n");
        let function = report.split("== function identity ==\n").nth(1).unwrap();

        assert!(function.contains("effects:\ncompletions:\n"));
        assert!(function.contains("return Local(1, 1) when true"));
        assert!(!function.contains("  normal "));
        assert!(!function.contains("WriteLocal"));
    }

    #[test]
    fn source_nested_function_reports_captured_read() {
        let report = analyze("def outer(x):\n  def inner():\n    return x\n  return inner\n");
        let inner = report.split("== function outer.inner ==\n").nth(1).unwrap();

        assert!(inner.contains("return Captured(0, 0) when true"));
    }

    #[test]
    fn registry_declares_placeholder_global_with_host_effect() {
        let report = analyze_with_registry(
            "emit(\"hello\")\n",
            r#"{
                "functions": {
                    "emit": {
                        "effects": [{
                            "guard": "entered",
                            "host_effect": {"name": "emit", "arguments": [0]}
                        }],
                        "may_return": true,
                        "may_fail": false
                    }
                }
            }"#,
        );

        assert!(report.contains("HostEffect { name: \"emit\""));
        assert!(report.contains("Constant(\"\\\"hello\\\"\")"));
        assert!(!report.contains("UnknownEffect"));
        assert!(!report.contains("failure "));
    }

    #[test]
    fn registry_mutation_substitutes_function_arguments() {
        let report = analyze_with_registry(
            "def append_host(xs, value):\n  append_host_impl(xs, value)\n",
            r#"{
                "functions": {
                    "append_host_impl": {
                        "effects": [{
                            "guard": "normal",
                            "mutate_argument": {
                                "target": 0,
                                "mutation": {"append_argument": 1}
                            }
                        }],
                        "may_return": true,
                        "may_fail": true
                    }
                }
            }"#,
        );

        assert!(report.contains("region: Objects({InputRoot(0)})"));
        assert!(report.contains("Append(SymbolicValue { expression: Input(1)"));
        assert!(report.contains("when normal(op0)"));
        assert!(report.contains("failure Unknown(\"error(op0)\") when fails(op0)"));
    }
}
