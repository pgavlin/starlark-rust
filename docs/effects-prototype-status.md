# Effects prototype status

This document describes the effects prototype currently implemented in
`starlark/src/eval/effects`. It is a snapshot of the implementation, not a
stable API or a claim of sound coverage for arbitrary Starlark programs. The
intended design and prototype scope remain documented in
[`effects.md`](effects.md) and [`effects-prototype.md`](effects-prototype.md).

The implementation is crate-private. It analyzes optimized structured compiler
IR (`StmtCompiled` and `ExprCompiled`) and does not participate in evaluation or
bytecode generation.

## Predicates

Predicates are hash-consed in a per-analysis DAG and referred to by
`PredicateId`. The implemented nodes are:

```text
True
False
Atom
Not(predicate)
And(predicates...)
Or(predicates...)
```

The implemented atom vocabulary is:

```text
Truth(symbolic value)
Normal(operation)
Fails(operation)
Opaque(name)
```

`Truth` guards conditional branches and short-circuit operands. `Normal` and
`Fails` correlate effects and continuations with the outcome of a potentially
failing operation. `Opaque` is available to tests and future classifiers.

The interner performs the following normalization:

* interns structurally equal nodes;
* folds `NOT true`, `NOT false`, and double negation;
* flattens nested `And` and `Or` nodes;
* removes identity operands and duplicate operands;
* applies absorbing operands (`false` for `And`, `true` for `Or`);
* recognizes direct `p AND NOT p` and `p OR NOT p`;
* sorts operands by interned predicate ID.

The prototype does not perform general Boolean reasoning. In particular, it
has no SAT/SMT integration, implication checking, distributive normalization,
or predicates with bound variables. It also does not yet encode the
mutual-exclusion and exhaustiveness rules for operation outcomes as an
`OutcomeGroup`; `Normal(op)` and `Fails(op)` are currently opaque atoms to the
Boolean simplifier.

## Completions and flow

A structured analysis result is a `Flow` with three collections:

```text
effects: guarded effect occurrences
normal: guarded state/value continuations
abrupt: guarded noncontinuing completions
```

Normal completion is represented by an entry in `Flow::normal`. The abrupt
completion variants currently implemented are:

```text
Return(value, state)
Failure(error, state)
```

Sequential composition preserves effects and abrupt completions from the first
operation, but invokes the next operation only for entries in `normal`. This is
how failure and return prevent later statements from being analyzed on those
paths.

Branching evaluates successors under complementary predicates. Identical
continuing states and values are joined by disjoining their reachability
predicates. When states differ, the implementation currently retains guarded
alternatives instead of constructing a full merged state with phi nodes.
Equivalent effect payloads are merged at the end of analysis by OR-ing their
guards.

The following completion forms are not implemented:

* `Break`;
* `Continue`;
* `Cancel`.

Compiler IR containing loops, `break`, or `continue` is sent through the
visible conservative fallback rather than given loop semantics.

## Effects

The implemented externally visible effect vocabulary is:

```text
ObserveHost(name, arguments)
WriteModule(slot, value)
Mutate(region, mutation)
HostEffect(name, arguments)
UnknownEffect(origin)
```

The implementation also records these internal diagnostic effects:

```text
WriteLocal(slot, value)
WriteCaptured(slot, value)
```

`AnalysisResult::external_effects` projects local and captured writes out of
the externally visible view. Module writes remain visible.

The represented mutation vocabulary is:

```text
SetIndex(key, value)
SetAttribute(name, value)
Append(value)
Opaque(name)
```

At present, supplied call summaries can instantiate `Append` and `Opaque`
mutations. The structured IR adapter does not yet emit precise `SetIndex` or
`SetAttribute` mutations; unsupported assignment targets use the conservative
fallback.

A call is not itself an effect. A supplied pure summary therefore contributes
no effects. An unresolved call contributes both:

```text
UnknownEffect(call site/key)
Mutate(AllObjects, Opaque("unknown mutation"))
```

and may complete normally or fail. Other unsupported operations use the same
shape of conservative fallback, with an origin describing the unsupported
operation.

## Call summaries

The prototype has an analyzer-owned, crate-private summary registry. It can
identify callees by:

```text
Module(slot)
Local(slot)
Captured(slot)
Method(name)
Constant(repr)
```

This is an analyzer-owned mechanism, not a native registration API. Tests can
register summaries directly, and the exploratory CLI can load summaries from a
JSON registry. Summaries state whether a call may return and/or fail, plus
guarded effects. A summary
effect can be guarded by:

```text
Entered
Normal
Failure
```

Supported summary effect templates are:

```text
ObserveHost(name, selected arguments)
MutateArgument(target argument, mutation)
MutateAll(name)
HostEffect(name, selected arguments)
Unknown(name)
```

`MutateArgument` substitutes the selected argument's current region at the call
site. Its mutation can append another argument or use an opaque mutation name.
Method summaries receive the receiver as argument zero.

Arguments are evaluated in compiler-IR order before applying the summary.
Effects guarded by `Normal` use the same `Normal(op)` atom as the normal
continuation, while effects guarded by `Entered` occur independently of the
operation's eventual outcome.

Arbitrary summary predicates, named-argument substitution, exported summary
interfaces, and interprocedural Starlark summary computation are not
implemented.

### JSON summary registry

Pass a registry to the source CLI with `--effects-summaries`:

```console
cargo run -p starlark_bin -- \
  --effects \
  --effects-summaries effects.json \
  example.star
```

A registry can declare names that do not have runtime implementations. These
names are installed as compile-only placeholder globals and are never executed.
Configured names override ordinary globals for the analysis, allowing an
existing host function such as `print` to receive a custom summary as well.

```json
{
  "functions": {
    "emit": {
      "effects": [
        {
          "guard": "entered",
          "host_effect": {
            "name": "emit",
            "arguments": [0]
          }
        }
      ],
      "may_return": true,
      "may_fail": false
    },
    "read_clock": {
      "effects": [
        {
          "observe_host": {
            "name": "clock",
            "arguments": []
          }
        }
      ],
      "may_return": true,
      "may_fail": true
    }
  }
}
```

`guard` is one of `entered`, `normal`, or `failure` and defaults to `entered`.
`may_return` defaults to `true`; `may_fail` defaults to `false`. Available
effect forms are:

```json
{"host_effect": {"name": "emit", "arguments": [0]}}
{"observe_host": {"name": "clock", "arguments": []}}
{"mutate_argument": {"target": 0, "mutation": {"append_argument": 1}}}
{"mutate_argument": {"target": 0, "mutation": {"opaque": "change"}}}
{"mutate_all": {"name": "unknown mutation"}}
{"unknown": {"origin": "embedding-defined behavior"}}
```

Argument numbers are zero-based. The registry currently applies only to direct
calls to configured global names. Aliases, methods, namespaced functions, and
command-line definitions of arbitrary predicate guards are not supported.

## Regions

The current abstract-object vocabulary is:

```text
InputRoot(parameter)
CapturedRoot(slot)
ModuleRoot(slot)
Allocation(site)
Host(id)
```

A region is one of:

```text
Empty
Objects(finite set of abstract objects)
AllObjects
```

Region join unions finite object sets and treats `AllObjects` as top. The
prototype currently propagates regions through direct local, captured, and
module assignment. Parameters initially point to their corresponding
`InputRoot`; uncached captured and module reads receive their corresponding
root. Constructed list, tuple, and dictionary values receive an
analysis-local `Allocation` site. Unknown values and unsupported projections
widen to `AllObjects`.

The structured analyzer preserves differing branch states as guarded
alternatives. `RegionSet::join` exists for places where guarded alternatives
must be collapsed, but there is not yet a general state-join implementation
that unions every differing slot.

The following parts of the full region design are not implemented:

* abstract heap edges and projection-sensitive loads;
* transitive reachability;
* alias discovery through attributes or indices;
* cardinality and strong updates;
* mutability/frozen-state tracking;
* call-context-sensitive allocation identities;
* exposure timing and escape analysis;
* projection of construction mutations on fresh returned values;
* host-region registration.

`Host` objects and precise index/attribute mutation descriptors are present in
the vocabulary for supplied summaries and later transfers, but are not yet
produced by ordinary structured-IR analysis.

## Structured IR coverage

The analyzer currently has direct handling for:

* statement sequences;
* expression statements;
* `if` statements and conditional expressions;
* `and` and `or` short-circuit evaluation;
* `return`;
* direct local, captured, and module assignment;
* local, captured, and module reads;
* constants;
* tuple, list, and dictionary construction;
* expression sequencing;
* selected known unary operations (`not` and compiler type tests);
* calls with supplied summaries;
* unresolved calls through conservative fallback.

Module reads and reads of uninitialized locals produce normal and failure
alternatives. Captured reads currently assume an input captured root when no
value has been assigned in the analyzed state.

The following are explicitly unsupported and widened visibly:

* loops, comprehensions, `break`, and `continue`;
* augmented assignment;
* destructuring assignment;
* precise attribute and index assignment;
* most unary and binary operations;
* slicing and multi-index operations;
* arbitrary dynamic call resolution.

The fallback emits `UnknownEffect`, an opaque mutation of `AllObjects`, and
both normal and failure completions. It does not silently classify unsupported
behavior as pure and total.

## Reporting and testing

`AnalysisResult::report` produces deterministic, debug-oriented text beginning
with `experimental effect analysis`. It prints externally visible effects and
normal, return, and failure completions with rendered guards. Reports do not
yet include source spans, function identities, effect-site identities, or loop
occurrence identities.

A source-level exploratory CLI is available through the repository's existing
`starlark` binary. It compiles source to structured IR without executing it and
prints reports for the module and each named function, including nested
functions:

```console
cargo run -p starlark_bin -- --effects example.star
cargo run -p starlark_bin -- --effects -e 'def f(x): return x'
```

The CLI accepts `--dialect standard` or `--dialect extended`. Loads are rejected
because source analysis does not execute a loader. JSON call summaries can be
supplied with `--effects-summaries` as described above. The analyzer's types and
implementation remain crate-private; an opt-in, doc-hidden `effects-cli` feature
exposes only the string-report bridge needed by `starlark_bin`.

The tests currently cover:

* predicate interning and cheap normalization;
* branch joins using disjunction;
* abrupt completion stopping sequencing;
* pure total and pure partial summaries;
* mutation of an input region;
* correlation of mutation with normal completion;
* mutation that occurs before either outcome;
* compiler-IR short-circuit guarding;
* compiler-IR failure preventing a later effect;
* compiler-IR branch continuation simplifying to an OR join;
* visible conservative widening and deterministic report output for unsupported IR;
* source compilation and reporting for modules, functions, and nested functions;
* module-region propagation, local return flow, and captured reads and writes;
* JSON summary validation, placeholder host globals, host effects, and argument mutation.

## Deferred work

Consistent with the prototype plan, the implementation does not include loops,
effect families, interprocedural Starlark analysis, a stable native-summary API,
the full region model, cancellation, or bytecode validation. Useful next steps for
the source harness include richer summary predicates, load modeling, source
spans in reports, and compiler-level golden files for larger programs.
