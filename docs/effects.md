# Predicated effects

This document proposes a prototype for a path-sensitive effect analysis for
Starlark. It is a design sketch, not a commitment to a public API.

## Motivation

An effect is an operation that changes the world visible at some point in a
program. Which world is relevant depends on the level of the analysis:

* at function boundaries, summaries include mutation of caller-visible state,
  mutation of module state, and host effects, plus failure and cancellation
  outcomes;
* inside a function, assignment to locals and captured locals is also relevant;
* at the bytecode level, stack and iterator-stack operations are relevant;
* at an individual operation, calling another function and operation-specific
  behavior are relevant.

Every effect has a predicate describing when it occurs. For example:

```python
if enabled:
    emit()
```

contributes the effects in `emit`'s summary under the guard:

```text
Effects(emit) when truth(enabled)
```

The call itself is not an effect. A pure function contributes no effects.

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
Effects(a)    when entry
FailureAt(a)  when entry AND fails(a)
Effects(b)    when entry AND normal(a)
FailureAt(b)  when entry AND normal(a) AND fails(b)
```

`Effects(a)` and `Effects(b)` mean the effects contributed by the respective
callee summaries. Either set may be empty.

The analysis therefore propagates a normal-continuation predicate through the
operations in a block. Given an input predicate `p`, an operation summary
produces:

* zero or more guarded effects;
* a normal outcome and its predicate;
* zero or more non-local outcomes and their predicates;
* a symbolic result and symbolic state for each continuing outcome.

Failure and cancellation are first-class outcomes and observable function
exits, but are not represented as ordinary `Effect` values. A broader report
may project them into `MayFail` or `MayCancel` properties. `return`, `break`,
and `continue` are non-local outcomes used to build control flow.

### Effects and outcomes are different

A call is an operation that contributes the effects and outcomes in its
callee's summary; it is not itself an effect. Calling a pure function therefore
contributes no effects. Failure is an outcome of evaluating the call. Keeping
these concepts separate allows a native operation to have a description such
as:

```text
operation read_file(path):
    effect ReadFile(path)
    normal(bytes) when readable(path)
    failure(error) when NOT readable(path)
```

A pure but partial function has no effects but may have a failure outcome. A
pure and total function has no effects and only a normal outcome. The prototype
does not require predicates to be this precise. A conservative summary may say
that multiple outcomes are possible and that unknown effects may occur.

## Relation to programming-languages literature

The proposed outcome model is well established, although the literature uses
several names: *outcome*, *completion*, *behavior*, *result*, and *exit*.
"Completion" is probably the most recognizable name for the internal sum:

```text
Completion(T) =
    Normal(T)
  | Return(T)
  | Break(loop)
  | Continue(loop)
  | Failure(error)
  | Cancel(reason)
```

The variants are not all visible at every boundary. A loop consumes its own
`Break` and `Continue`; a function consumes `Return`; `Failure` and `Cancel`
propagate out of the function unless some future language construct handles
them.

### Operational semantics and abrupt completion

Big-step operational semantics commonly gives statement evaluation a judgment
of the form:

```text
<statement, state> => <completion, state'>
```

Sequencing continues only from normal completion:

```text
s1 => Normal(state1)    s2(state1) => completion
-------------------------------------------------
s1; s2 => completion

s1 => Abrupt(reason)
--------------------
s1; s2 => Abrupt(reason)
```

The [Java Language Specification,
§14.1](https://docs.oracle.com/javase/specs/jls/se21/html/jls-14.html#jls-14.1)
uses exactly the distinction between *normal completion* and *abrupt
completion*. Its abrupt reasons include `break`, `continue`, `return`, and
`throw`.

CompCert's Clight semantics is an especially close precedent. It assigns
statements `Normal`, `Break`, `Continue`, and `Return` outcomes and separately
records observable traces. See Blazy and Leroy, [*Mechanized Semantics for the
Clight Subset of the C
Language*](https://xavierleroy.org/publi/Clight.pdf), JAR 2009
([DOI](https://doi.org/10.1007/s10817-009-9148-3)), especially §3.3. The
separation between an outcome and a trace is analogous to the proposed
separation between completions and effects.

Charguéraud's [*Pretty-Big-Step
Semantics*](https://www.chargueraud.org/research/2012/pretty/pretty.pdf), ESOP
2013 ([DOI](https://doi.org/10.1007/978-3-642-37036-6_3)), generalizes an
evaluation judgment so that it produces a *behavior*: either a regular result
or an exception. Intermediate forms and generic propagation rules avoid
replicating exception propagation throughout the semantics. This supports the
same implementation principle proposed here: represent abrupt completion once
and propagate it uniformly.

### Monadic and continuation accounts

In denotational semantics, failure is commonly represented by a sum:

```text
Computation(T) = T + Error
```

Sequential composition continues only through the `T` alternative. This is the
exception monad, situated in the more general account of computational effects
in Moggi, [*Notions of Computation and
Monads*](https://doi.org/10.1016/0890-5401(91)90052-4), Information and
Computation 1991.

In continuation-passing style, the same distinction appears as separate normal
and exceptional continuations:

```text
evaluate(normal_continuation, failure_continuation)
```

This is another presentation of explicit successor edges. `Return`, `Break`,
and `Continue` can likewise be represented by distinguished continuations.

Weakest-precondition and related predicate-transformer presentations make the
same split in the reverse direction: a potentially failing computation is
reasoned about using separate normal and exceptional postconditions. Our
forward propagation of guarded completions is the corresponding forward view.

### Why some literature calls failure an effect

Type-and-effect systems use *effect* more broadly than this proposal. A typical
judgment may look like:

```text
Gamma |- expression : T ! {State, IO, Throw(E)}
```

Here `Throw(E)` is an effect because the effect row summarizes what evaluation
may do. Important references include Lucassen and Gifford, [*Polymorphic Effect
Systems*](https://doi.org/10.1145/73560.73564), POPL 1988, and Leroy and
Pessaux, [*Type-Based Analysis of Uncaught
Exceptions*](https://doi.org/10.1145/349214.349230), TOPLAS 2000.

Algebraic-effect literature also classifies exceptions as effects. Raising an
exception invokes a handler and, unlike a resumable operation, does not provide
a normal resumption at the raise point. See Plotkin and Power, [*Algebraic
Operations and Generic
Effects*](https://doi.org/10.1023/A:1023064908962), 2003, and Plotkin and
Pretnar, [*Handlers of Algebraic
Effects*](https://doi.org/10.1007/978-3-642-00590-9_7), ESOP 2009.

There is no contradiction between these uses:

1. an **effect annotation** says statically that a computation may throw;
2. a **completion** says dynamically or operationally how this execution
   finished;
3. an **exceptional CFG edge** says where control goes after that completion.

This proposal reserves `Effect` for guarded observable actions such as mutation
or host interaction, and uses `Completion` for control exits. `MayFail` can be
derived from the presence of a reachable `Failure` completion when an
embedding wants a conventional effect-system view.

### Compiler representations

Compiler IRs commonly make the same control-flow distinction. LLVM's
[`invoke`](https://llvm.org/docs/LangRef.html#invoke-instruction) has separate
normal and exceptional destinations. Rust MIR terminators expose normal targets
and unwind behavior on operations such as [`Call`, `Assert`, and
`Drop`](https://doc.rust-lang.org/nightly/nightly-rustc/rustc_middle/mir/enum.TerminatorKind.html).
JVM bytecode leaves many exceptions implicit, but analyses generally recover
exceptional CFG edges.

The prototype should follow this compiler practice semantically without
necessarily allocating a physical basic block after every potentially failing
operation. An operation transfer function can return guarded normal and abrupt
completions, and the analysis can materialize CFG nodes only when useful.

## Effect vocabulary

The first implementation should use a deliberately small and extensible
vocabulary.

### Observable effects

```text
WriteModule(slot, value)
Mutate(region, operation)
Load(module)
HostEffect(name, arguments)
UnknownEffect(origin)
```

The global environment includes more than module bindings. It includes mutable
objects visible to callers or reachable from module values, loaded-module
state, and state owned by native code.

### Mutation, places, and regions

`Mutate(region, operation)` is an abstract description of a heap transition.
The region identifies the storage that may have changed; the operation is a
semantic description of the change. It is not a call event. A call contributes
a mutation only when its callee summary contains one.

A region is an abstract identity for a set of runtime objects, not a concrete
address. The analysis should distinguish three related concepts:

1. An **abstract place** or **access path** is a symbolic way to locate a value,
   such as `Parameter(0)["items"]` or `Module(config).field`.
2. A **region** is the abstract object or set of objects stored at a place,
   such as `InputObject(0)` or `Allocation(L7)`.
3. **Reachability** is the conservative set of regions obtainable by traversing
   an object graph, such as `ReachableFrom(InputObject(0))`.

The distinction matters because different places may contain references to the
same object. Treating an access path as an object identity would miss that
aliasing.

A possible representation is:

```rust
pub(crate) enum PlaceRoot {
    Parameter(u32),
    Local(LocalSlotId),
    Captured(LocalCapturedSlotId),
    Module(ModuleSlotId),
}

pub(crate) enum Projection {
    Attribute(String),
    Index(SymbolicValueId),
    Unknown,
}

pub(crate) struct AbstractPlace {
    pub root: PlaceRoot,
    pub projections: Box<[Projection]>,
}

pub(crate) enum AbstractObject {
    InputRoot(u32),
    CapturedRoot(LocalCapturedSlotId),
    ModuleRoot(ModuleSlotId),
    Allocation(AllocationSite),
    Host(HostRegionId),
    InputReachableSummary(ExternalRoot),
}

pub(crate) enum RegionSet {
    Empty,
    Objects(SmallSet<AbstractObject>),
    AllObjects,
}

/// A lazily evaluated expression denoting a RegionSet.
pub(crate) enum RegionExpr {
    Concrete(RegionSetId),
    ReachableFrom(RegionExprId),
    Union(Box<[RegionExprId]>),
    AllObjects,
}
```

For:

```python
def add(config):
    config["items"].append(1)
```

the target expression first denotes the place:

```text
Parameter(0)["items"]
```

Loading that place produces a symbolic value whose points-to set may identify a
particular region. If it cannot be resolved precisely, the mutation is:

```text
Mutate(ReachableFrom(InputObject(0)), Append(1))
```

`ReachableFrom` is not a logical address. It denotes any object transitively
reachable from its root. `AbstractPlace` is the logical-address-like concept.
If even the root is unknown, the safe fallback is `AllObjects`.

#### Mutation descriptors

The operation component should describe the semantic mutation rather than the
method used to implement it:

```rust
pub(crate) enum Mutation {
    SetIndex {
        key: SymbolicValueId,
        value: SymbolicValueId,
    },
    DeleteIndex {
        key: SymbolicValueId,
    },
    SetAttribute {
        name: String,
        value: SymbolicValueId,
    },
    Append {
        value: SymbolicValueId,
    },
    Extend {
        values: SymbolicValueId,
    },
    Clear,
    Opaque {
        name: String,
        arguments: Box<[SymbolicValueId]>,
    },
}
```

Thus `xs.append(x)` and `list.append(xs, x)` should contribute the same
semantic effect:

```text
Mutate(region(xs), Append(x))
```

The initial analysis records mutation events, not merely the net difference
between initial and final heaps. `xs.append(1); xs.pop()` contains two
mutations even if it restores the original value. A future analysis may reason
about cancellation, commutativity, or final heap differences.

#### Propagating and joining regions

Symbolic reference values carry a may-points-to set:

```rust
pub(crate) struct SymbolicValue {
    pub expression: SymbolicExpression,
    pub regions: RegionSet,
}
```

Assignments propagate this set. In:

```python
def f(xs):
    ys = xs
    ys.append(1)
```

`ys` points to `InputObject(0)`, so the effect is:

```text
Mutate(InputObject(0), Append(1))
```

Control-flow joins union points-to sets, while predicates preserve useful
precision:

```python
def f(cond, xs, ys):
    z = xs if cond else ys
    z.append(1)
```

can produce:

```text
Mutate(InputObject(1), Append(1)) when truth(cond)
Mutate(InputObject(2), Append(1)) when NOT truth(cond)
```

If the path distinction is widened away, the target becomes
`Union(InputObject(1), InputObject(2))`. This is a may-alias abstraction: a
region may denote several concrete objects, and two region expressions may
alias.

Rebinding a variable is distinct from mutating the referenced object:

```python
def f(xs):
    xs = []
```

writes a local slot but does not mutate `InputObject(0)`. Similarly,
`WriteModule(slot, value)` changes a module binding, whereas
`Mutate(ModuleObject(slot), ...)` changes an object obtained from that binding.

#### Call-site substitution

Regions make effect summaries compositional. Given:

```python
def add(xs, value):
    xs.append(value)
```

the relative summary is:

```text
Mutate(InputObject(0), Append(Input(1))) when normal(append)
```

At `add(my_list, compute())`, parameter and region substitution produces:

```text
Mutate(region(my_list), Append(result(compute)))
  when normal(compute) AND normal(append)
```

The call itself contributes no effect. It evaluates the callee and arguments,
instantiates the callee summary, adds the resulting guarded effects, and
propagates the callee's completions.

#### Fresh regions and escape

Fresh allocations begin in allocation-site regions:

```python
def make():
    xs = []
    xs.append(1)
    return xs
```

Internally this is approximately:

```text
Allocate Allocation(L1)
Mutate(Allocation(L1), Append(1))
Return Allocation(L1)
```

Constructing a fresh result is ordinarily pure from the caller's perspective.
The caller receives the completed list and cannot observe its earlier empty
state. The mutation can be folded into the symbolic returned value rather than
reported as an externally visible function effect.

The analyzer therefore needs a small, flow-sensitive escape abstraction:

* parameters, captured values, module values, and host regions begin externally
  reachable;
* fresh allocations begin local;
* returning a fresh value exposes it only at function completion;
* storing a fresh value into externally reachable state exposes it at that
  program point;
* passing it to an unknown operation may expose it or cause it to be retained;
* mutations before first exposure are construction of the value and can be
  folded into its symbolic state;
* mutations after exposure are externally observable and remain effects.

Thus a value may eventually escape while its earlier construction mutations
remain unobservable. The implementation can retain all fresh-region mutations
while analyzing a function and perform this projection only when constructing
the summary. If exposure timing cannot be established safely, retaining the
mutation is conservative.

This projection composes across calls. A callee may report mutation of its
first parameter; if the caller substitutes a nonescaping fresh region for that
parameter, the caller may project the mutation out of its own summary.

#### Mutation and failure

A mutation is not necessarily confined to normal completion. An operation may
fail before mutating, mutate and succeed, or partially mutate and then fail.
Guarded effects must therefore be independent of guarded completions:

```rust
pub(crate) struct OperationSummary {
    pub effects: Vec<GuardedEffect>,
    pub completions: Vec<GuardedCompletion>,
}
```

A built-in append may report mutation only on its normal path after rejecting a
frozen receiver. An unknown native operation must conservatively allow an
opaque mutation on both normal and failure paths if it may modify state before
failing.

#### Native and host regions

Native code may mutate its receiver, an argument, a value reachable from an
argument, or state with no Starlark address. Named host regions represent the
last case:

```text
Mutate(Host("build-cache"), SetIndex(key, value))
```

A native summary should be able to refer to receiver and argument regions
relatively, so they can be substituted at each call site. It should also be
able to use `AllObjects` when no meaningful footprint is available.

### Proposed full region model

The full region lattice should not be a large enum in which access paths,
object identities, reachability, and escape are all variants of the same type.
Its mathematical core is the powerset of a finite collection of abstract object
identities.

Let `A` be the abstract objects created for one analysis. Then:

```text
Region = P(A)

ordering:  R1 <= R2 when R1 is a subset of R2
bottom:    {}
join:      R1 union R2
meet:      R1 intersection R2
top:       AllObjects
```

This is a complete lattice. An abstract object is indivisible to the analysis,
but need not denote exactly one concrete object. An allocation-site object in a
loop may summarize every concrete object allocated at that site.

A more complete set of object atoms is:

```rust
pub(crate) enum AbstractObject {
    InputRoot {
        parameter: u32,
    },
    CapturedRoot {
        slot: LocalCapturedSlotId,
    },
    ModuleRoot {
        slot: ModuleSlotId,
    },
    Allocation {
        function: FunctionId,
        site: AllocationSite,
        context: AllocationContext,
    },
    Host {
        id: HostRegionId,
    },
    InputReachableSummary {
        root: ExternalRoot,
        selector: ReachabilitySelector,
    },
}

pub(crate) enum RegionSet {
    Empty,
    Objects(SmallSet<AbstractObject>),
    AllObjects,
}
```

`Union` is the lattice join, not an object identity. A fully unknown target is
`AllObjects`; an `InputReachableSummary` is more precise when an external root
is still known.

#### Abstract heap

Points-to information is represented by a finite abstract heap graph:

```rust
pub(crate) struct AbstractHeap {
    pub edges: Map<(AbstractObject, ProjectionKey), RegionSetId>,
}
```

For example:

```text
InputRoot(0) --Index("items")--> {Allocation(L7)}
InputRoot(0) --Index("backup")--> {Allocation(L7)}
```

records that two different places may refer to the same object. Loading an
abstract place follows these edges and returns a region set. Unknown indexing
joins all matching edges. Missing or unmodellable information widens to
`AllObjects`.

Projection keys form a small lattice of their own:

```text
Index("items") <= AnyIndex <= AnyProjection
Attribute("name") <= AnyAttribute <= AnyProjection
TupleIndex(0) <= AnyTupleIndex <= AnyProjection
```

Small joins may retain sets of exact keys. Once a size limit is exceeded they
widen to `AnyIndex`, `AnyAttribute`, or `AnyProjection`. Access paths are
similarly truncated after a configurable depth.

#### Reachability

Reachability is a monotone closure operation over the abstract heap:

```text
reachable(heap, roots) -> RegionSet
```

It is not a distinct object identity. Semantically it computes every abstract
object reachable from the root set. The implementation may retain a lazy
`ReachableFrom(roots)` expression to avoid eagerly expanding the closure.
Selectors can restrict traversal:

```rust
pub(crate) enum ReachabilitySelector {
    Any,
    Elements,
    Attributes,
    IndexKeys,
    IndexValues,
}
```

For example, `ReachableFrom(InputRoot(0), Elements)` can describe objects held
inside a container without including unrelated host regions.

#### Guarded region information

Path sensitivity should preserve guarded alternatives when affordable:

```rust
pub(crate) struct GuardedRegion {
    pub predicate: PredicateId,
    pub region: RegionSetId,
}
```

For:

```python
z = xs if cond else ys
```

this retains:

```text
{InputRoot(1)} when truth(cond)
{InputRoot(2)} when NOT truth(cond)
```

rather than immediately joining the targets. When the state budget is reached,
the alternatives join to `{InputRoot(1), InputRoot(2)}`.

#### Interprocedural identity

Function summaries refer to boundary-relative objects such as `InputRoot(0)`,
`CapturedRoot(0)`, and `ModuleRoot(config)`. Call-site substitution maps these
objects to region sets in the caller.

Fresh objects produced by a callee must be instantiated per call context:

```text
Allocation(callee, L7)
    -> Allocation(caller, call_site, callee, L7)
```

Otherwise two calls to a constructor would incorrectly appear to return the
same object. Context sensitivity is bounded; after the budget is exhausted,
allocation identities widen back to a context-insensitive function and
allocation site. Recursive calls use the same widening.

#### Cardinality and strong updates

A singleton region set does not imply one concrete object. Track cardinality
separately:

```rust
pub(crate) enum Cardinality {
    Zero,
    One,
    Many,
    Unknown,
}
```

A singleton region containing a unique `One` allocation may permit a strong
symbolic-heap update. A region with multiple targets, `Many` cardinality, or
`AllObjects` requires a weak update. This distinction affects subsequent
symbolic state; either update still produces a mutation effect.

#### Escape and exposure

Escape is a separate dataflow domain, not part of object identity. A region may
begin private and become exposed later. The domain can record exposure
channels:

```rust
bitflags! {
    pub(crate) struct Exposure {
        const RETURNED     = 1 << 0;
        const MODULE       = 1 << 1;
        const CAPTURED     = 1 << 2;
        const HOST         = 1 << 3;
        const UNKNOWN_CALL = 1 << 4;
    }
}
```

The lattice is set inclusion: bottom is no exposure, join is union, and top is
unknown exposure through any channel. Flow-sensitive first-exposure information
determines whether a mutation constructs private state or changes already
observable state.

#### Mutability

Mutability and frozen state are also separate from region identity:

```rust
pub(crate) enum Mutability {
    Mutable,
    Frozen,
    MaybeMutable,
}
```

Type information can establish that primitive values and tuples are immutable,
that lists and dictionaries may be mutable, and that host values require a
supplied summary. Mutation of a frozen object produces a failure completion
rather than a mutation effect. `MaybeMutable` retains both possibilities under
appropriate predicates.

#### Full abstract state

The eventual analysis state is a reduced product of these domains:

```rust
pub(crate) struct AbstractState {
    /// Symbolic values in locals, captures, module slots, and temporaries.
    pub values: Environment<SymbolicValue>,

    /// May-points-to information for symbolic reference values.
    pub points_to: PointsToGraph,

    /// Symbolic object contents and object-to-object edges.
    pub heap: AbstractHeap,

    /// Exposure channels and first-exposure information.
    pub exposure: ExposureState,

    /// Whether atoms denote zero, one, or many concrete objects.
    pub cardinality: CardinalityState,

    /// Mutable, frozen, or unknown status.
    pub mutability: MutabilityState,

    /// Conditions under which this state is reachable.
    pub predicate: PredicateId,
}
```

The components refine one another: types inform mutability, points-to sets
identify mutation targets, heap edges resolve places, cardinality controls
strong updates, exposure controls effect projection, and predicates preserve
path-sensitive alternatives.

#### Widening

The analysis must widen rather than grow these domains without bound:

```text
too many object atoms       -> broader reachable summary or AllObjects
too many guarded choices    -> join their region sets
too much access-path depth  -> ReachableFrom(prefix)
too many exact index keys   -> AnyIndex
too much call context       -> context-insensitive allocation site
loop allocation             -> one Many allocation-site object
unknown native behavior     -> AllObjects
```

Every widening loses precision but must retain every possible concrete target.

### Region implementation phases

#### Phase 1: roots, allocations, and direct aliases

Implement:

```rust
pub(crate) enum AbstractObject {
    InputRoot(u32),
    CapturedRoot(LocalCapturedSlotId),
    ModuleRoot(ModuleSlotId),
    Allocation(AllocationSite),
    Host(HostRegionId),
}

pub(crate) enum RegionSet {
    Empty,
    Objects(SmallSet<AbstractObject>),
    AllObjects,
}
```

Propagate regions through direct assignments, preserve guarded alternatives at
joins, substitute input roots at calls, track basic exposure, and widen
unsupported projections to `AllObjects`. Retain fresh mutations internally and
project private construction from function summaries.

#### Phase 2: abstract heap and aliasing through loads

Add:

* abstract heap edges;
* exact attribute, constant-index, and tuple-index projections;
* alias discovery through loads;
* projection widening;
* allocation cardinality;
* strong and weak symbolic-heap updates.

#### Phase 3: reachability and context

Add:

* transitive reachability with selectors;
* lazy `ReachableFrom` expressions;
* access-path depth widening;
* bounded call-context-sensitive allocation identities;
* loop allocation summarization;
* flow-sensitive first-exposure tracking.

#### Phase 4: host and precision extensions

Add:

* host-provided region identities and containment edges;
* richer native mutation footprints;
* type-directed mutability refinement;
* more precise may-alias relationships;
* configurable region and predicate budgets;
* diagnostics explaining where widening lost precision.

The implementation phases grow toward the complete powerset lattice without
requiring the full heap abstraction before the basic effect and completion
model can be validated.

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
* call with a known or unknown summary;
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

* apply supplied summaries for known native functions;
* retain call operations as provenance in debug output, but not as effects;
* treat unresolved calls conservatively;
* do not recursively analyze Starlark callees yet.

A conservative unknown-call summary is approximately:

```text
UnknownEffect(call_site)
Mutate(AllObjects, Opaque("unknown mutation"))
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

Assume `a` contributes effect `A` and `b` contributes effect `B`. At event
granularity:

```text
A(iteration=i) when i in range(5) AND i < 2
B(iteration=i) when i in range(5) AND NOT (i < 2)
```

At function granularity, the iteration variable is existentially quantified:

```text
MayEffect(A) when EXISTS i: i in range(5) AND i < 2
MayEffect(B) when EXISTS i: i in range(5) AND NOT (i < 2)
```

Both predicates simplify to `true`. If multiplicity is retained, `a` is called
twice and `b` three times, assuming every preceding operation completes
normally.

That final qualification matters. If calls may fail, reaching later iterations
also requires earlier iterations to complete normally. After unrolling:

```text
A@0 when true
A@1 when normal(a@0)
B@2 when normal(a@0) AND normal(a@1)
B@3 when normal(a@0) AND normal(a@1) AND normal(b@2)
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

Assuming `a` contributes effect `A`:

```text
x[0] = input(x)
x[k + 1] = result(step(x[k]))

A@k when
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

pub(crate) struct FunctionSummary {
    pub occurrences: Vec<EffectOccurrence>,
    pub completions: Vec<GuardedCompletion>,
}
```

A deterministic textual form is important for golden tests. For example:

Assuming `emit` contributes an `Emit` effect and may fail:

```text
function example(enabled):
  effects:
    Emit @ example.star:3:5
      when truth(input(enabled))
  outcomes:
    fail unknown @ example.star:3:5
      when truth(input(enabled)) AND fails(emit)
    return None
      when NOT truth(input(enabled)) OR
           (truth(input(enabled)) AND normal(emit))
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
* composition of calls with known or conservative unknown summaries;
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

Assume `compute` and `finish` have no effects but may fail, while `xs.append`
contributes mutation of `xs`. The expected analysis should show:

```text
effects:
  Mutate(InputObject(1), Append(result(compute)))
    when truth(enabled) AND normal(compute) AND mutates(xs.append)

outcomes:
  Fail(compute)
    when truth(enabled) AND fails(compute)
  Fail(xs.append)
    when truth(enabled) AND normal(compute) AND fails(xs.append)
  Fail(finish)
    when reaches(finish) AND fails(finish)
  Normal
    when reaches(finish) AND normal(finish)

where:
  reaches(finish) =
    NOT truth(enabled)
    OR (truth(enabled) AND normal(compute) AND normal(xs.append))
```

The calls may be retained as operation provenance in a detailed report, but
are not effects. The important property is the control dependency: `finish()`
is not unconditionally evaluated because the apparently straight-line
preceding branch contains operations that can fail.
