# Predicated effects

This document proposes a prototype for a path-sensitive effect analysis for
Starlark. It is a design sketch, not a commitment to a public API.

## Motivation

An effect is an operation that changes the world visible at some point in a
program. Which world is relevant depends on the level of the analysis:

* at function boundaries, effects include mutation of caller-visible state,
  mutation of module state, host effects, failure, and cancellation;
* inside a function, assignment to locals and captured locals is also relevant;
* at the bytecode level, stack and iterator-stack operations are relevant;
* at an individual operation, calling another function and operation-specific
  behavior are relevant.

Every effect has a predicate describing when it occurs. For example:

```python
if enabled:
    emit()
```

produces the effect:

```text
Call(emit) when truth(enabled)
```

The intended result is a conservative description of which effects may occur
and under which conditions. It is not initially intended to prove arbitrary
predicates or reproduce complete execution traces.

This resembles symbolic execution, guarded effect systems, and
weakest-precondition analysis. The prototype should reuse ideas from all three
without initially attempting to be a complete implementation of any of them.

## Terminology

* **Conjunction** is logical AND, written `p AND q` or `p ∧ q`.
* **Disjunction** is logical OR, written `p OR q` or `p ∨ q`.
* **Negation** is logical NOT, written `NOT p` or `¬p`.
* A **path predicate** is a condition under which control reaches a program
  point.
* An **effect predicate** is a condition under which an effect occurs.
* An **outcome** determines where control goes after an operation. Normal
  completion, failure, cancellation, return, break, and continue are outcomes.

Multiple conditions along one path are conjoined. Alternative paths into a
block are disjoined.

## Core model

Let `P(B)` be the predicate under which a basic block `B` is entered. The entry
block has predicate `true`. Other blocks have:

```text
P(B) = OR(P(E) for E in incoming(B))
```

For a conditional terminator in `B`:

```text
P(true_edge)  = P_out(B) AND truth(condition)
P(false_edge) = P_out(B) AND NOT truth(condition)
```

`P_out(B)` is not necessarily equal to `P(B)`. Operations in the block may
fail or otherwise terminate control before the terminator is reached.

If the same effect is found under predicates `p` and `q`, its may-effect summary
can merge them as:

```text
Effect when p OR q
```

Alternatively, the analysis may preserve the two occurrences when order,
multiplicity, or source locations matter.

### Operations have outcomes

Failure introduces implicit control flow. Conceptually, every potentially
failing operation has at least two outgoing edges:

```text
             +-- normal(value) --> next operation
operation ---+
             +-- failure(error) -> function failure
```

For straight-line source such as:

```python
x = a()
b(x)
```

the relevant predicates are:

```text
Call(a)       when entry
FailureAt(a)  when entry AND fails(a)
Call(b)       when entry AND normal(a)
FailureAt(b)  when entry AND normal(a) AND fails(b)
```

The analysis therefore propagates a normal-continuation predicate through the
operations in a block. Given an input predicate `p`, an operation summary
produces:

* zero or more guarded effects;
* a normal outcome and its predicate;
* zero or more non-local outcomes and their predicates;
* a symbolic result and symbolic state for each continuing outcome.

Failure and cancellation are first-class outcomes. They are also observable
function effects. `return`, `break`, and `continue` are non-local outcomes used
to build control flow, but are not necessarily externally observable effects.

### Effects and outcomes are different

Calling a function is an effect occurrence even when the function returns
normally. Failure is an outcome of the call. Keeping these concepts separate
allows a native operation to have a description such as:

```text
operation read_file(path):
    effect ReadFile(path)
    normal(bytes) when readable(path)
    failure(error) when NOT readable(path)
```

The prototype does not require predicates to be this precise. A conservative
summary may say that both outcomes are possible.

## Effect vocabulary

The first implementation should use a deliberately small and extensible
vocabulary.

### Observable effects

```text
WriteModule(slot, value)
Mutate(region, operation)
Load(module)
Call(callee, arguments)
HostEffect(name, arguments)
Fail(error)
Cancel(reason)
```

The global environment includes more than module bindings. It includes mutable
objects visible to callers or reachable from module values, loaded-module
state, and state owned by native code.

Regions provide a conservative description of mutated storage:

```text
Argument(index)
Captured(slot)
Module(slot)
Fresh(allocation_site)
Host(name)
UnknownReachable
```

For example, `xs.append(1)` may produce `Mutate(Argument(0), Append(1))` when
`xs` is known to be the first argument. A mutation of a fresh value that does
not escape can be omitted from a function-level observable summary. Escape and
alias analysis can be added after the initial prototype; until then,
`UnknownReachable` is the safe fallback.

### Internal effects

```text
WriteLocal(slot, value)
WriteCaptured(slot, value)
PushValue(value)
PopValue
PushIterator(value)
PopIterator
```

Internal effects are useful for validating the analysis and explaining
bytecode, but should normally be projected out of a function's public summary.
The source-level prototype will initially record local and captured writes but
will not model bytecode stack operations.

### Per-operation behavior

Arithmetic, comparison, indexing, attribute access, truth conversion,
iteration, mutation, argument binding, and type checks may fail or invoke
behavior implemented by a Starlark value. They must not all be assumed pure and
infallible merely because they are not syntactic calls.

The initial operation table should classify each compiler IR operation as one
of:

* known pure and infallible;
* known pure but potentially failing;
* known mutation;
* known call;
* unknown, conservatively potentially failing and effectful.

## Predicates and symbolic values

The prototype should keep predicates symbolic. A possible internal form is:

```rust
pub(crate) enum Predicate {
    True,
    False,
    Atom(PredicateAtom),
    Not(PredicateId),
    And(Box<[PredicateId]>),
    Or(Box<[PredicateId]>),
    Exists(Symbol, PredicateId),
}
```

Predicate atoms may initially be opaque:

```text
Truth(value)
Normal(operation)
Fails(operation)
Member(iteration_value, iterable)
Equals(left, right)
LessThan(left, right)
```

Symbolic values need only be expressive enough to preserve useful identity:

```text
Input(parameter)
Constant(value)
Local(slot, version)
Captured(slot, version)
Module(slot, version)
Apply(operation, arguments)
Phi(incoming values)
IterationValue(loop, iteration)
Unknown(origin)
```

Predicates and symbolic values should be interned. Constructors should perform
cheap normalization:

* flatten nested `And` and `Or` nodes;
* remove duplicate operands;
* eliminate `true` and `false` identities;
* recognize `p AND NOT p` and `p OR NOT p`;
* use stable operand ordering.

A SAT or SMT solver is explicitly not required for the first prototype. The
analysis can expose unsimplified predicates and allow a later component to
translate them to a solver.

State-changing operations require versioned values. In an acyclic graph, this
is ordinary SSA-like state with `Phi` values at joins. Loops require recurrence
or approximation, discussed below.

## Calls and summaries

A function summary consists of guarded effects and guarded outcomes expressed
in terms of its inputs and incoming regions. At a call site, the callee's
formal values are replaced with the caller's symbolic argument values, and all
of its predicates are conjoined with the predicate under which the call occurs.

For a callee effect `E` guarded by `q` and a call guarded by `p`:

```text
E[arguments / parameters] when p AND q[arguments / parameters]
```

The first prototype should be intraprocedural:

* record calls as effects;
* use supplied summaries for known native functions;
* treat unresolved calls conservatively;
* do not recursively analyze Starlark callees yet.

A conservative unknown-call summary is approximately:

```text
Call(Unknown, arguments)
Mutate(UnknownReachable, UnknownMutation)
normal(UnknownValue) OR failure(UnknownError)
```

Interprocedural Starlark summaries can later be computed over the call graph.
Recursion requires a fixed point and widening, just as loops do.

### Native functions

Functions implemented outside Starlark need to contribute their own summaries.
The analysis should accept a registry alongside the globals used to compile the
module. The exact public API should be deferred until the internal model has
proved useful. Conceptually:

```rust
trait NativeEffectProvider {
    fn effects(&self, call: &SymbolicCall) -> OperationSummary;
}
```

Registration may ultimately be integrated with `GlobalsBuilder` or the
`#[starlark_module]` machinery. The prototype can instead use an analyzer-owned
map keyed by the resolved global value or a stable native-function identifier.

A summary is allowed to be conservative. For example, a filesystem function
may report `HostEffect("filesystem")` and both normal and failure outcomes
without describing operating-system permissions.

## Loops

Loops are simple when effects are described for a particular dynamic
iteration, but function summaries must account for iteration variables,
loop-carried state, early exit, and failures in prior iterations.

### A simple counted loop

```python
for i in range(5):
    if i < 2:
        a()
    else:
        b()
```

At event granularity:

```text
Call(a, iteration=i) when i in range(5) AND i < 2
Call(b, iteration=i) when i in range(5) AND NOT (i < 2)
```

At function granularity, the iteration variable is existentially quantified:

```text
MayCall(a) when EXISTS i: i in range(5) AND i < 2
MayCall(b) when EXISTS i: i in range(5) AND NOT (i < 2)
```

Both predicates simplify to `true`. If multiplicity is retained, `a` is called
twice and `b` three times, assuming every preceding operation completes
normally.

That final qualification matters. If calls may fail, reaching later iterations
also requires earlier iterations to complete normally. After unrolling:

```text
Call(a@0) when true
Call(a@1) when normal(a@0)
Call(b@2) when normal(a@0) AND normal(a@1)
Call(b@3) when normal(a@0) AND normal(a@1) AND normal(b@2)
```

A coarse may-effect analysis may omit this history when it is enough to know
that there exists an execution in which earlier calls return normally. The
result remains conservative, but its predicates are less informative.

### Early exit

```python
for i in range(n):
    if stop(i):
        break
    write(i)
```

The occurrence `write(k)` is guarded by approximately:

```text
0 <= k < n
AND normal(stop(k))
AND NOT truth(stop(k))
AND FOR ALL j, 0 <= j < k:
    normal(stop(j))
    AND NOT truth(stop(j))
    AND normal(write(j))
```

The function-level may-effect existentially quantifies `k`. `continue`,
`return`, and failure introduce similar history conditions.

### Loop-carried state

```python
def f(n, x):
    for i in range(n):
        x = step(x)
        if x == 0:
            a()
```

The value of `x` differs on each iteration:

```text
x[0] = input(x)
x[k + 1] = result(step(x[k]))

Call(a@k) when
    0 <= k < n
    AND all required earlier operations completed normally
    AND x[k + 1] == 0
```

This cannot be represented precisely by attaching one timeless `x == 0` atom
to the loop body. It requires a recurrence, an invariant, bounded unrolling, or
a conservative abstraction.

### Initial loop strategy

The prototype should use two strategies:

1. Unroll statically known small loops, especially constant `range` loops, up
   to a configurable limit.
2. For all other loops, emit a symbolic loop summary with an iteration variable
   and conservatively approximate loop-carried state.

The symbolic summary should preserve:

* membership in the iterated value;
* body branch predicates;
* the possibility of zero iterations;
* `break`, `continue`, `return`, failure, and cancellation outcomes;
* a `MayRepeat` marker when multiplicity is unknown.

It may initially omit exact prior-iteration history and replace modified
loop-carried values with `Unknown(loop, slot)`. Later versions can add
recurrences, abstract interpretation, and widening.

## Where to implement the prototype

The Rust implementation has two useful compiler representations:

* the resolved and optimized structured IR in
  `starlark/src/eval/compiler`, notably `StmtCompiled`, `ExprCompiled`, and
  `CallCompiled`;
* the bytecode writer and instructions in `starlark/src/eval/bc`.

The initial analysis should operate on the structured compiler IR, immediately
before bytecode generation.

Reasons:

* names have already been resolved to local, captured, and module slots;
* constants and some branches have already been simplified;
* `if`, `for`, `break`, `continue`, and `return` remain explicit;
* call expressions and assignment targets retain more semantic information
  than bytecode instructions;
* source spans are available;
* the bytecode writer emits a patched linear instruction stream rather than
  retaining a convenient CFG.

The analyzer should lower `StmtCompiled` and `ExprCompiled` into a small,
analysis-only control-flow representation. This representation makes implicit
failure edges explicit. It does not need to replace the execution bytecode or
be stored in compiled modules.

A later bytecode-level validation pass can compare the source-level operational
effects against emitted instructions and add stack effects. Bytecode should not
be the first implementation because recovering callee and region information
there would require unnecessary abstract stack interpretation.

### Proposed internal modules

The exact names are tentative:

```text
starlark/src/eval/effects/
    mod.rs          public-in-crate entry points
    cfg.rs          analysis CFG and outcomes
    effect.rs       effect and region vocabulary
    predicate.rs    interned predicate DAG
    symbolic.rs     symbolic values and state
    operation.rs    operation classification and transfer functions
    loop.rs         unrolling and loop summaries
    native.rs       native summary registry
    report.rs       stable text/debug output
```

The analysis should initially be behind a crate-private API and test-only entry
point. A stable public API should wait until the vocabulary and native-summary
mechanism have been exercised.

## Analysis algorithm

For each function or module body:

1. Create symbolic values for parameters, captured values, and module inputs.
2. Start the entry point with predicate `true` and an initial symbolic state.
3. Lower structured statements and expressions in evaluation order.
4. For each operation:
   1. instantiate its operation summary;
   2. record each effect under the current predicate and the effect's guard;
   3. send failure and cancellation outcomes to function exits;
   4. continue with the normal-outcome predicate and updated symbolic state.
5. At a conditional, send state to both successors under the truth and false
   predicates.
6. At a join, disjoin incoming reachability predicates and merge symbolic state
   with `Phi` values.
7. Handle constant bounded loops by unrolling; otherwise apply the symbolic
   loop approximation.
8. Merge equivalent effects by disjoining predicates when producing a
   function-level may-effect summary.
9. Project internal effects and nonescaping fresh-region mutations out of the
   externally visible summary when justified.

Joins must use disjunction. For example:

```python
if x:
    y = 1
else:
    y = 2
use(y)
```

`use(y)` is reached under `truth(x) OR NOT truth(x)`, while its symbolic
argument is `Phi(1 when truth(x), 2 when NOT truth(x))`.

## Proposed report shape

The analyzer should retain occurrences before producing a merged summary:

```rust
pub(crate) struct EffectOccurrence {
    pub effect: Effect,
    pub predicate: PredicateId,
    pub span: FrameSpan,
    pub function: FunctionId,
    pub iteration: Box<[IterationBinding]>,
}

pub(crate) struct FunctionEffects {
    pub occurrences: Vec<EffectOccurrence>,
    pub outcomes: Vec<GuardedOutcome>,
}
```

A deterministic textual form is important for golden tests. For example:

```text
function example(enabled):
  effects:
    call emit() @ example.star:3:5
      when truth(input(enabled))
    fail unknown @ example.star:3:5
      when truth(input(enabled)) AND fails(call emit())
  outcomes:
    return None
      when NOT truth(input(enabled)) OR
           (truth(input(enabled)) AND normal(call emit()))
```

The report should distinguish an occurrence from a merged function summary so
that merging does not accidentally discard source location, order, or
multiplicity information.

## Prototype milestones

### Milestone 1: acyclic, intraprocedural analysis

Support:

* sequential evaluation;
* `if` statements and conditional expressions;
* `and` and `or` short-circuiting;
* local, captured, module, attribute, and index assignment;
* calls as effects;
* normal and failure outcomes;
* `return`;
* deterministic debug output.

Treat unknown operations and all calls conservatively. Do not analyze callees.
Reject or summarize loops as a single opaque `MayRepeat` operation.

This milestone validates the predicate algebra, evaluation order, hidden
failure control flow, and effect vocabulary.

### Milestone 2: finite loops and abrupt control flow

Add:

* unrolling for small constant `range` loops;
* iteration effects;
* `break` and `continue`;
* failures in earlier iterations;
* multiplicity in occurrence reports.

The simple `range(5)` example should be a golden test.

### Milestone 3: native summaries

Add an analyzer-owned native summary registry and summaries for a small set of
builtins. Verify that argument substitution and guarded outcomes compose at
call sites.

### Milestone 4: symbolic loops

Add:

* existential iteration variables;
* symbolic iterable membership;
* conservative loop-carried state;
* recurrence nodes for selected patterns;
* widening and resource limits.

### Milestone 5: interprocedural Starlark summaries

Compute summaries over the call graph, substitute arguments and regions at
call sites, and use fixed points for recursion. Cache summaries by function and
relevant analysis configuration.

### Milestone 6: external API and tooling

After the model is stable, expose summaries for embedders and potentially the
LSP. Possible uses include diagnostics, policy enforcement, optimization,
sandbox review, and documentation of native APIs.

## Soundness and limits

The prototype is a may-analysis: if it omits an effect, the intent is that the
effect cannot occur. False positives are acceptable. This requires unknown
calls and unknown value operations to be modeled conservatively.

Exact predicates are not generally achievable. Even when all Starlark loops
terminate, predicates can depend on arbitrary input values, native behavior,
mutation, aliasing, and properties that are difficult or impossible to decide
statically. Totality removes divergence as an outcome; it does not make exact
path reachability or effect inference decidable.

The analysis must have explicit resource limits for:

* predicate DAG size;
* number of states at a program point;
* loop unrolling;
* call-graph iteration;
* effect occurrences before merging.

When a limit is reached, the analysis should widen to `Unknown` predicates,
values, regions, or effects rather than silently dropping behavior.

A set of guarded effects is also less expressive than a trace. It does not by
itself preserve effect ordering, exact multiplicity, or correlations between
different effects. Occurrence reports retain enough identity to add ordering
or trace summaries later, but those are not goals of the first prototype.

## Initial test cases

The first implementation should include golden tests for:

1. mutually exclusive branches joining with OR;
2. failure preventing a later call;
3. short-circuit `and` and `or`;
4. mutation through an argument versus mutation of a fresh local value;
5. module assignment;
6. a small constant `range` loop;
7. `break` and `continue`;
8. return from inside a branch and inside a loop;
9. unknown native calls;
10. an operation such as indexing that may fail despite not being a syntactic
    function call.

A representative first test is:

```python
def example(enabled, xs):
    if enabled:
        xs.append(compute())
    finish()
```

The expected analysis should show, in evaluation order:

```text
Call(compute) when truth(enabled)
Fail(compute) when truth(enabled) AND fails(compute)
Call(xs.append)
  when truth(enabled) AND normal(compute)
Fail(xs.append)
  when truth(enabled) AND normal(compute) AND fails(xs.append)
Mutate(Argument(1), Append(result(compute)))
  when truth(enabled) AND normal(compute) AND mutates(xs.append)
Call(finish)
  when NOT truth(enabled)
       OR (truth(enabled) AND normal(compute) AND normal(xs.append))
```

The exact spelling is less important than preserving the control dependency:
`finish()` is not unconditionally executed because the apparently straight-line
preceding branch contains operations that can fail.
