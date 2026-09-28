# Predicated effects prototype plan

This document scopes the first implementation of the design in
[`effects.md`](effects.md). The purpose of the prototype is to test the core
algebra against the Rust compiler IR and discover which parts of the larger
design need refinement.

The prototype is deliberately crate-private, incomplete, and conservative. It
is not initially a public API or a claim that all Starlark behavior has been
modeled precisely.

## Readiness

The design is ready to prototype. Its remaining gaps are mostly questions that
are easier to answer with running code than with more design work. The first
implementation should make unsupported behavior explicit and should favor a
small vertical slice over broad but shallow coverage.

In particular, the prototype should not begin with symbolic loops,
interprocedural Starlark analysis, or the complete region model. It should first
validate:

* predicate construction and simplification;
* normal and abrupt completion propagation;
* hidden failure control flow;
* guarded effects;
* branch joins;
* call-summary composition;
* a minimal region abstraction;
* deterministic diagnostics suitable for golden tests.

## Open questions

These questions are not blockers. The prototype should preserve enough
information to let us answer them incrementally.

### Observable effects versus observations

The motivating definition says that an effect changes the world, but useful
analyses also need to represent dependence on the world. Reading a file or the
clock changes no state but prevents referential transparency and may establish
a dependency.

The eventual vocabulary should probably distinguish:

```text
Read/Observe     depends on externally visible state
Write/Mutate     changes externally visible state
Allocate         creates a fresh identity
HostEffect       embedding-defined behavior
```

For the prototype, both externally observable reads and writes belong in
`Effect`, using distinct variants. Allocation of private Starlark values is an
internal operation, not automatically an externally observable effect.

### Predicate atom semantics

The design currently uses atoms such as:

```text
truth(value)
normal(operation)
fails(operation)
cancels(operation)
```

Their relationships must be explicit. For an operation reached under predicate
`p`, its outcome alternatives should be mutually exclusive and exhaustive:

```text
p -> normal(op) OR fails(op) OR cancels(op)
NOT(normal(op) AND fails(op))
NOT(normal(op) AND cancels(op))
NOT(fails(op) AND cancels(op))
```

The exact alternatives depend on the operation. A known infallible operation
has only a normal alternative. The first implementation may record an
`OutcomeGroup` alongside the predicate DAG rather than teaching the Boolean
simplifier all of these facts immediately.

### Function-summary predicate boundaries

A function summary may initially contain predicates referring to:

* parameters;
* module or host state;
* internal operations;
* iteration binders;
* symbolic values and heap regions;
* completion alternatives.

Not every internal atom can be exported and substituted at a call site. We
will need rules for a summary's symbolic interface. A likely direction is:

* retain predicates over inputs and named host state;
* bind iteration variables in effect families;
* existentially hide irrelevant internal values;
* expose guarded completion alternatives;
* preserve any internal outcome identities needed to correlate effects with
  completions.

The intraprocedural prototype may retain internal atoms. Interprocedural work
must define the boundary explicitly.

### Correlation and ordering

Separate collections of effects and completions must still express whether an
effect occurs on a normal path, a failure path, or both. Initially this
correlation will be represented by shared predicate and outcome atoms:

```text
Mutate(R, M) when normal(op)
Failure(E)   when fails(op)
```

or:

```text
Mutate(R, M) when entered(op)
Failure(E)   when fails(op)
```

for an operation that may mutate before failing.

The prototype records effect occurrences in evaluation order internally, but a
function's may-effect summary does not promise to preserve a complete trace or
partial order. Trace summaries and cross-effect ordering are non-goals for the
first implementation.

### Unknown behavior

Many operations that are not syntactic calls dispatch to behavior implemented
by a Starlark value:

* truth testing;
* comparison and equality;
* indexing;
* attribute access;
* hashing;
* iteration;
* arithmetic;
* mutation and type checks.

A fully conservative unknown transfer is approximately:

```text
UnknownEffect(operation)
Mutate(AllObjects, Opaque("unknown mutation"))
Normal(UnknownValue) OR Failure(UnknownError)
```

Applying that summary everywhere would make results nearly useless. The
prototype should therefore model a deliberately closed subset of operations
and make every unsupported or widened operation visible in the report. It must
never silently treat an unsupported operation as pure and infallible.

Until operation coverage is comprehensive, prototype reports should be labeled
as experimental rather than sound summaries of arbitrary programs.

### Compiler analysis point

There are three possible subjects for the analysis:

1. source-language behavior;
2. behavior of the optimized structured compiler IR;
3. literal emitted-bytecode behavior.

The prototype will analyze optimized `StmtCompiled` and `ExprCompiled`
immediately before bytecode generation. At that point:

* names have been resolved to slots;
* constants and some branches have been simplified;
* structured control flow is still present;
* assignment targets and calls retain semantic information;
* source spans remain available.

The analysis therefore describes the optimized structured program. A later
bytecode validation pass may compare its results with emitted instructions and
add machine effects.

We should revisit this choice if compiler optimization evaluates user-defined
behavior or otherwise introduces compile-time observable actions not represented
by the optimized IR.

### Region lattice precision

The full design uses a powerset lattice over abstract objects. Simple subset
ordering is most natural when object atoms denote disjoint concrete sets.
Summary atoms such as separate transitive reachable regions may overlap because
arguments can alias.

The first prototype avoids this issue by using only:

```text
InputRoot(parameter)
CapturedRoot(slot)
ModuleRoot(slot)
Allocation(site)
Host(id)
AllObjects
```

and explicit finite sets of those atoms. Transitive reachability and
root-relative summary atoms are deferred. The eventual lattice order may need
to be defined through concretization rather than syntactic set inclusion.

### Cancellation

Cancellation is an abrupt completion, but adding a cancellation successor to
every operation would add considerable noise. The prototype omits cancellation
until actual evaluator cancellation or safepoints have been identified. It must
not imply that cancellation is impossible in the real evaluator.

A later model may add cancellation only at safepoints or expose it as a coarse
function-level completion.

### Type information

Types can establish that some operations are built-in, immutable, or
infallible. The prototype should not require type information initially, but
its operation classifier should accept it later. Unknown types must use a
conservative transfer rather than inherit behavior from an optimistic type
assumption.

## First vertical slice

The first implementation should define the following conceptual model.

### Predicates

```text
Predicate:
    True
    False
    Atom
    Not
    And
    Or
```

Predicates should be interned in a DAG. Constructors should initially perform
only cheap, deterministic normalization:

* flatten nested `And` and `Or`;
* remove duplicate operands;
* eliminate Boolean identities;
* recognize direct `p AND NOT p` and `p OR NOT p`;
* impose stable operand ordering.

No SAT or SMT integration is required.

### Completions

```text
Completion:
    Normal(value, state)
    Return(value, state)
    Failure(error, state)
```

`Break`, `Continue`, and `Cancel` are deferred until their corresponding
control-flow constructs are implemented.

Only `Normal` completion feeds the next operation in a sequence. `Return` and
`Failure` propagate outwards.

### Effects

The initial effect vocabulary is:

```text
ObserveHost(name, arguments)
WriteModule(slot, value)
Mutate(region, mutation)
HostEffect(name, arguments)
UnknownEffect(origin)
```

Local and captured writes may be retained as internal events for diagnostics,
but they are projected out of an externally visible function summary.

The initial mutation vocabulary may be limited to:

```text
SetIndex(key, value)
SetAttribute(name, value)
Append(value)
Opaque(name)
```

### Regions

```text
AbstractObject:
    InputRoot(parameter)
    CapturedRoot(slot)
    ModuleRoot(slot)
    Allocation(site)
    Host(id)

RegionSet:
    Empty
    Objects(set of AbstractObject)
    AllObjects
```

The first implementation propagates regions through direct assignments and
joins. Unsupported projections widen to `AllObjects`. Abstract heap edges,
transitive reachability, cardinality, and strong updates are deferred.

### Flow

The structured analyzer should return a flow containing:

```rust
struct Flow {
    effects: Vec<GuardedEffect>,
    normal: Vec<GuardedState>,
    abrupt: Vec<GuardedCompletion>,
}
```

The exact Rust representation may differ, but it must preserve the distinction
between continuing and noncontinuing paths.

Sequential composition is conceptually:

```text
analyze(first; second, input):
    first_flow = analyze(first, input)
    preserve first_flow.effects
    preserve first_flow.abrupt
    analyze second from each state in first_flow.normal
```

Branching analyzes both successors under complementary predicates. A join
disjoins reachability predicates and merges symbolic state.

### Calls

A call is an operation, not an effect. Analysis proceeds in evaluation order:

1. evaluate the callee expression;
2. evaluate arguments;
3. identify or conservatively approximate the callee summary;
4. substitute symbolic arguments and regions;
5. add the callee's guarded effects;
6. propagate each callee completion.

The first implementation should use a test-owned summary registry. It does not
need a stable native registration API.

## Supported structured IR

The first vertical slice should recursively analyze enough of
`StmtCompiled`/`ExprCompiled` to support:

* statement sequences;
* `if` statements and conditional expressions;
* `and` and `or` short-circuiting;
* `return`;
* local reads and writes;
* captured reads and writes;
* module reads and writes;
* constants;
* calls with supplied summaries;
* explicit conservative fallback for unsupported expressions and assignments.

Loops, comprehensions, complex destructuring, interprocedural Starlark calls,
and precise attribute/index behavior are deferred.

The structured IR already represents the relevant control constructs, so the
first implementation does not need a general basic-block CFG. Explicit
completions in `Flow` provide the required hidden control flow. A CFG can be
introduced later if fixed-point analysis or bytecode validation makes it
useful.

## Proposed module layout

```text
starlark/src/eval/effects/
    mod.rs          crate-private entry points and result types
    predicate.rs    interned predicate DAG
    effect.rs       effects, mutations, and initial regions
    completion.rs   guarded completion types and outcome groups
    flow.rs         sequencing, branching, and joins
    analyze.rs      structured compiler-IR traversal
    report.rs       deterministic debug/golden output
    tests.rs        unit and golden tests
```

This layout is provisional. Modules should be combined if the initial
implementation is too small to justify them.

## Initial tests

### Branch joins use disjunction

```python
if cond:
    effect_a()
else:
    effect_b()
finish()
```

Verify that the continuation is reached under the disjunction of the two
normal branch predicates, not their conjunction.

### Failure stops sequencing

```python
potentially_failing()
effect_a()
```

Verify that `effect_a` is guarded by normal completion of the first operation.

### Pure calls contribute no effects

```python
pure()
```

A supplied pure-and-total summary should produce no effects and one normal
completion. A pure-but-partial summary should produce no effects and both
normal and failure completions.

### Mutation targets an input region

```python
def f(xs):
    xs.append(1)
```

With a supplied append summary, verify:

```text
Mutate(InputRoot(0), Append(1))
```

under the append mutation guard.

### Fresh construction is projected out

```python
def f():
    xs = []
    xs.append(1)
    return xs
```

The internal analysis may retain mutation of `Allocation(site)`, but the
externally visible summary should treat it as construction of the returned
value rather than mutation of preexisting state.

This test may be deferred until basic exposure tracking exists.

### Mutation and failure remain correlated

Use one supplied summary that mutates only on normal completion and another
that mutates before either normal or failure completion. Verify that their
mutation predicates differ.

### Short-circuiting

```python
cond and effect_a()
cond or effect_b()
```

Verify that the right operand is guarded by the appropriate truth predicate and
normal completion of the left operand.

### Unsupported operations widen visibly

Analyze an operation outside the modeled subset. Verify that the result
contains `UnknownEffect`, possible failure, and any required unknown mutation
rather than silently treating the operation as pure.

## Implementation order

1. Implement and test the predicate interner and normalization.
2. Implement guarded completions and `Flow` sequencing.
3. Add branch splitting and joins over a small test-only analysis IR.
4. Add effects and test-owned operation summaries.
5. Adapt the analyzer to the supported `StmtCompiled`/`ExprCompiled` subset.
6. Add deterministic report output and compiler-level golden tests.
7. Add minimal roots, allocations, and direct region propagation.
8. Add small constant-loop unrolling and effect families only after the
   acyclic model is stable.

A small test-only analysis IR in step 3 is useful for validating the algebra
without compiler-IR lifetime and allocation concerns. It should be discarded
or kept strictly as a test fixture once the structured compiler adapter works.

## Prototype success criteria

The first prototype is successful if it can explain, deterministically and
correctly within its supported subset:

* why an effect is or is not reachable;
* why failure prevents a later effect;
* why branch joins use OR;
* why a pure call contributes no effects;
* which minimal region a mutation targets;
* where unsupported behavior forced widening.

It does not need to provide precise answers for loops, arbitrary native values,
recursive calls, or the full Starlark language.

After this slice works, the next implementation steps are:

1. small constant-loop unrolling;
2. effect families and multiplicity;
3. a native summary API;
4. abstract heap edges and richer regions;
5. symbolic loops;
6. interprocedural Starlark summaries;
7. bytecode validation and machine effects.
