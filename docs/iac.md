# Predicated effects for infrastructure as code

This document sketches an infrastructure-as-code toolset built around the
predicated effect analysis described in [effects.md](effects.md). It is an
initial design intended for iteration, not a stable format or API.

## Goals

The system should have the expressive power of Pulumi-style IaC, including the
ability to make resource declarations conditional on outputs that are not known
until another resource has been created or read:

```python
network = aws.vpc(...)

if network.region == "us-east-1":
    aws.instance(...)
```

At the same time, it should provide stronger preview and update guarantees:

* preview reports known and possible infrastructure effects;
* possible effects include predicates explaining when they occur;
* policy can reject effects merely because they are possible;
* known values computed during preview are persisted;
* update consumes those persisted values instead of recomputing them;
* update executes a sealed plan rather than rerunning the source program;
* every update-time cloud or identity-state effect was represented by the
  preview;
* interruption and retry reuse previously observed outputs;
* the plan and its execution decisions are auditable.

The target is not a prediction of all future cloud behavior. Cloud APIs are
external systems and can fail or return different outputs. The target is a
deterministic residual plan whose behavior is explicit as a function of those
external observations.

## Central idea: preview produces a residual program

Preview is a partial-evaluation stage. It evaluates the Starlark program using:

* concrete values for known configuration, files, state, and other frozen
  inputs;
* symbolic values for resource outputs and deferred observations;
* effectful summaries for cloud SDK operations;
* explicit normal, failure, and cancellation completions.

The result is a persisted residual program. It contains guarded effects and the
pure computations needed to resolve their predicates and arguments later.
Update interprets this artifact; it does not reevaluate the Starlark source.

For:

```python
network = aws.vpc(...)

if network.region == "us-east-1":
    aws.instance(...)
```

the Starlark reconciliation libraries may produce a residual fragment like:

```text
R1: FindVpc(logical_vpc)

C1: CreateVpc(...)
    when Output(R1, "found") == false

VPC_REGION =
    if Output(R1, "found")
    then Output(R1, "vpc.region")
    else Output(C1, "region")

R2: FindInstance(logical_instance)
    when VPC_REGION == "us-east-1"

C2: CreateInstance(...)
    when VPC_REGION == "us-east-1"
     AND Output(R2, "found") == false
```

The actual fragment may also contain property updates, identity bindings, or a
delete when the condition becomes false. At update time the runtime executes
only these persisted API and state operations, binds their outputs, and follows
the persisted predicates. The conditional instance is visible during preview
even though preview cannot yet determine whether it should exist.

This is analogous to partial evaluation in OPA: known inputs are consumed now,
while conditions over designated unknowns are emitted as a residual program.
The difference is that the residual program here is effectful, introduces new
outputs as it executes, and must support dependency ordering, failure,
reconciliation, and retry.

## Core invariants

The design should aim to enforce the following invariants.

### Closed effect world

Every cloud API or identity-state effect performed during update must be an
instance of an effect node or quantified effect family present in the persisted
preview.

Update may resolve predicates, arguments, and family members. It may not
discover an entirely new shape of infrastructure operation.

### Predicate enforcement

An effect executes only if its persisted predicate evaluates to true using
frozen plan values and journaled outputs.

### No source reevaluation

Update interprets the residual plan. It does not rerun user Starlark code,
reread source files, or invoke arbitrary preview-time native functions.

### Stable known values

A value classified as known during preview is serialized into the plan. Update
uses the serialized value rather than recomputing it.

### Journaled dynamic values

When update resolves a symbolic output, it durably records the binding. Retry
and resume reuse that binding unless the native API contract explicitly
requires and validates a refresh.

### Version and integrity binding

The plan records and validates the versions or digests of its source, Starlark
reconciliation libraries, native SDK summaries, policy, schemas, and plan
format. The artifact should be sealable or signable.

### Runtime defense

The plan runtime checks the sealed plan and policy before every cloud or local
state effect. This is defense in depth against analyzer, SDK, native binding,
and serialization bugs.

## Runtime kernel

There is no resource-aware provider engine in this design. The remaining
trusted component is a small runtime kernel for staged Starlark programs. It
provides mechanisms that cannot safely be ordinary replayed user computation.

The kernel owns:

* preview-time partial evaluation;
* residual-plan serialization and integrity;
* frozen values and assumptions;
* dependency scheduling;
* dispatch of native cloud API primitives;
* effect and completion enforcement;
* durable output and completion journaling;
* update locking and concurrency control;
* credentials and capabilities;
* secret encryption;
* policy enforcement;
* stable logical identities and transactional generations;
* recovery of interrupted operations.

Starlark libraries own:

* resource abstractions;
* current-versus-desired comparison;
* replacement decisions;
* create-before-delete and delete-before-create strategies;
* interpretation of cloud API responses;
* retry and convergence policies;
* provider-specific naming constraints;
* lifecycle, import, and adoption behavior.

The kernel does not know what an S3 bucket, Kubernetes Deployment, or database
means. It provides deterministic execution, durable identity, and
transactional recovery for a residual effectful Starlark program. This
document uses *plan runtime* or *kernel* rather than the overloaded term
*engine*.

Its persistent store is not a resource-property mirror. It contains only
generic execution and ownership state:

```text
StateRevision

LogicalIdentity
  -> physical generations
  -> active generation
  -> remote identity
  -> frozen generated names
  -> library-defined opaque metadata

Plan
  -> frozen values
  -> assumptions
  -> approved effects

Journal
  -> started operations
  -> remote operation IDs
  -> completions
  -> output bindings
  -> predicate and policy decisions
```

Cloud properties are observed through explicit SDK operations. Starlark may
decide what constitutes a logical resource, when to allocate a generation, and
when to promote or retire it. The kernel supplies uniqueness, atomic
compare-and-swap, locking, durable generation allocation, and crash recovery.

## Starlark reconciliation and native APIs

There is no provider diff or CRUD protocol. Reconciliation is implemented in
Starlark, while native code is restricted to cloud API transport and other
trusted primitives.

A library can implement:

```python
def bucket(name, desired):
    current = s3.find_bucket(name)

    if current == None:
        return s3.create_bucket(
            name = name,
            versioning = desired.versioning,
        )

    if current.versioning != desired.versioning:
        s3.set_bucket_versioning(
            name = name,
            enabled = desired.versioning,
        )

    return current
```

Preview partially evaluates this ordinary Starlark code. Depending on what is
known, it can produce:

```text
CreateBucket(name, desired.versioning)
  when current == None

SetBucketVersioning(name, desired.versioning)
  when current != None
   AND current.versioning != desired.versioning
```

The comparison, lifecycle decision, and operation ordering remain visible and
versioned as Starlark. Native code sends and decodes requests; it does not
decide desired state.

## Cloud effect granularity

A call is not inherently an effect. A cloud SDK operation contributes effects
and completions through its supplied summary. The same operation should support
multiple views without pretending that a resource-level CRUD action is the
execution primitive.

### API-level effects

Every external request has an operational identity:

```text
CloudRequest(
    provider = aws,
    service = s3,
    operation = PutBucketVersioning,
    request = {...},
)
```

This view is useful for audit, permissions, native dispatch, and closed-world
enforcement.

### Semantic cloud footprints

A native API summary can attach resource- or property-level meaning:

```text
Mutate(
    CloudObject(aws, S3Bucket, bucket_id),
    SetProperty("versioning.enabled", value),
)
```

One API operation may contribute several footprints. For example, an instance
creation may create an object, set image and instance-type properties, and
attach it to a network. If no precise summary is available, it contributes an
explicit conservative effect:

```text
UnknownCloudMutation(service, operation, targets = AllCloudObjects)
```

Every mutating API primitive must provide either a sound semantic footprint or
an explicit unknown mutation. It may not claim purity because its semantic
model is incomplete.

The API request and footprints should be represented as one operation rather
than unrelated duplicate effects:

```rust
pub(crate) struct CloudOperation {
    pub api: CloudApiOperation,
    pub request: Vec<ResidualExpression>,
    pub predicate: PredicateId,
    pub targets: Vec<CloudRegion>,
    pub footprints: Vec<GuardedCloudEffect>,
    pub completions: Vec<GuardedCompletion>,
}
```

Resource-level diffs and replacements are derived explanations. Replacement is
implemented by visible operations such as create, repoint references, promote a
new physical generation, and delete the old object.

### Cloud objects as host regions

The region model from [effects.md](effects.md) represents remote objects as
host regions:

```text
Host(CloudObject {
    provider: aws,
    account: account_id,
    region: region,
    kind: "s3:bucket",
    id: bucket_name,
})
```

A property is an abstract place within that region. If identity is produced by
an earlier operation, it remains symbolic:

```text
CloudObject(kind = "ec2:instance", id = Output(create, "instance_id"))
```

### Reads and expected errors

Cloud reads contribute observation effects even though they do not mutate
remote state. They can execute during preview and produce frozen values plus
assumptions, or remain residual reads whose outputs guard later operations.

SDK bindings should normalize expected reconciliation results. For example,
`find_bucket` should return `Bucket | None`; a not-found response is a normal
result, while authentication and transport errors remain failure completions.
This produces predicates over typed values rather than raw exception strings.

## Effect occurrence and argument knowledge

Whether an effect occurs and whether its arguments are known are independent.
The report should distinguish at least:

```text
known occurrence, known arguments
known occurrence, partially unknown arguments
possible occurrence, known arguments
possible occurrence, partially unknown arguments
```

For example:

```text
CreateBucket(name = Output(project, "name"))
    when true
```

has known occurrence and an unknown argument.

```text
PutBucketVersioning(name = "logs", enabled = True)
    when Output(config, "enabled")
```

has known arguments and possible occurrence.

Semantic footprint precision is another independent dimension. The API
operation may be known while its exact property-level effects remain
conservative.

## Residual values and expressions

The residual plan needs a small pure expression language. It should be powerful
enough to calculate predicates, resource arguments, iteration keys, and output
projections without reopening arbitrary source execution.

Candidate expressions include:

```text
Literal(value)
EffectOutput(effect, path)
Parameter(name)
ApplyPure(operation, arguments)
Conditional(predicate, then, else)
And(expressions)
Or(expressions)
Not(expression)
Equals(left, right)
Compare(operation, left, right)
Index(value, key)
Attribute(value, name)
Collection(elements)
```

Only operations known to be pure should appear as `ApplyPure`. An operation
that may interact with the world must either have been evaluated and frozen
during preview or remain an explicit effect node.

Residual expressions should be typed where possible and retain source and
symbolic provenance for explanation.

## Candidate plan representation

The exact serialization is deferred, but the logical structure may resemble:

```rust
pub(crate) struct Plan {
    pub format_version: PlanFormatVersion,
    pub program_digest: Digest,
    pub environment: FrozenEnvironment,
    pub constants: Vec<FrozenValue>,
    pub expressions: Vec<ResidualExpression>,
    pub nodes: Vec<PlanNode>,
    pub families: Vec<EffectFamily>,
    pub assumptions: Vec<Assumption>,
    pub policy: PersistedPolicyResult,
    pub dependencies: PlanDependencies,
}

pub(crate) enum PlanNode {
    Cloud(CloudOperation),
    Identity(IdentityOperation),
    LocalState(LocalStateOperation),
    ResidualControl(ResidualControlNode),
}

pub(crate) struct EffectNodeHeader {
    pub id: EffectId,
    pub predicate: ExpressionId,
    pub dependencies: Vec<EffectId>,
    pub outputs: Vec<OutputDeclaration>,
    pub completions: Vec<GuardedCompletion>,
    pub provenance: SourceProvenance,
}
```

Known values are referenced as literals. Unknown values refer to outputs or
explicit update-time parameters. Dependencies include every operation whose
outputs occur in a predicate or argument, explicit sequencing from Starlark,
and ordering required by identity-state transitions. The plan stores residual
control where a finite effect DAG is insufficient, but it never stores an
unrestricted callback capable of discovering new operation shapes.

## Freezing known values

All known preview results that can affect the plan must be serialized.
Examples include:

* source literals and constant expressions;
* configuration values;
* environment variables exposed to Starlark;
* filesystem reads;
* module contents and load resolution;
* generated random names;
* timestamps, if exposed;
* preview-time cloud API observations;
* pure computation over known inputs.

For example:

```python
name = read_file("name.txt").strip()
aws.bucket(name = name)
```

If preview reads `"production-logs"`, the plan contains that string. Update
must not read `name.txt` again.

Every source of nondeterminism must therefore be one of:

1. executed during preview and frozen;
2. represented as an explicit update-time input or effect;
3. unavailable to the Starlark program.

Native code must not be able to bypass this rule by directly reading the clock,
filesystem, network, process environment, or random source. The Starlark
embedding needs a capability boundary and effect summaries for every exposed
native operation.

### Secrets and capabilities

Secrets may be frozen semantically while requiring encrypted or externalized
storage. The plan should retain secret taint through expressions and outputs so
that previews and diagnostics do not reveal secret values.

Update credentials are capabilities, not ordinary semantic inputs. They should
be supplied at update time but should not alter resource identities,
predicates, or desired inputs. If credential-dependent behavior would change
the plan, that behavior must be represented as an observation with an
assumption or a residual output.

## Observations and assumptions

A frozen remote observation may become stale between preview and update.
Freezing it without validation creates a time-of-check/time-of-use bug.

Values therefore need provenance. At minimum, distinguish:

```text
StableLiteral
FrozenChoice
ExternalObservation
ResidualOutput
```

An external observation normally contributes an assumption:

```text
Observed(resource.version) == 7
Observed(state.serial) == "abc123"
Observed(resource.etag) == "..."
```

Update validates assumptions before relying on them. If an assumption no
longer holds, the plan is stale and must be regenerated or explicitly amended.

Some assumptions can be validated before any mutation. Others can only be
checked immediately before a dependent operation. The plan should expose that
difference because late failure may occur after earlier effects.

## Policy over possible effects

Policy evaluates guarded cloud operations, their semantic footprints, and
kernel state effects. For an effect `E` with predicate `P`, a prohibition
amounts to asking whether:

```text
P AND Forbidden(E)
```

is satisfiable.

The policy result should distinguish:

```text
proved impossible
proved permitted
possibly forbidden
unknown due to abstraction or resource limits
```

Unknown must not be silently treated as permitted.

Useful policy modes include:

```text
forbid possible DeleteObject(database)
forbid SetProperty("public_access", True)
require approval for possible IAM policy mutation
allow only selected cloud API operations
require a bound on a generated effect family
```

Policy diagnostics should display:

* the effect or effect family;
* its predicate;
* source, SDK, and native API provenance;
* which values remain unknown;
* why the effect could not be eliminated;
* where widening or an unknown API footprint reduced precision.

### Rejecting possibility versus runtime denial

There are two different interpretations of a prohibited possible effect.

**Static rejection** rejects the plan if the forbidden effect is reachable for
any permitted resolution of its unknowns. No update begins. This is the safest
interpretation of `forbid possible`.

**Runtime denial** allows update to begin but refuses the effect if its
predicate becomes true. Earlier effects may already have executed before that
decision is possible. Runtime denial is useful as defense in depth, but is not
a replacement for static rejection.

The plan should make the selected policy behavior explicit.

## Logical identity and physical generations

Logical identity is separate from cloud identity. It remains stable while
physical objects are replaced:

```rust
pub(crate) struct LogicalResourceId {
    pub stack: StackId,
    pub parent: Option<Box<LogicalResourceId>>,
    pub kind: LogicalKind,
    pub key: LogicalKey,
}

pub(crate) struct PhysicalGenerationId {
    pub logical: LogicalResourceId,
    pub generation: u64,
}
```

For example:

```text
logical: production/web/worker

physical generations:
  production/web/worker@g0 -> i-0123
  production/web/worker@g1 -> i-0456
```

Starlark decides when a new generation is necessary and how references migrate.
The kernel guarantees durable, unique generation allocation and transactional
state transitions.

A useful logical identity is derived from:

```text
stack
+ parent identity
+ library-defined kind
+ explicit or structurally stable key
+ residual iteration key
```

Dynamic execution order must not affect identity. Source position is useful as
provenance and perhaps as a development default, but long-lived resources need
explicit or structurally stable keys. Aliases allow refactoring and adoption:

```python
resource(key = "worker", aliases = ["old-worker"], reconcile = ...)
```

The kernel resolves aliases against existing state and rejects ambiguous
ownership. Duplicate or conditionally colliding identities produce a static or
residual uniqueness obligation.

### Identity as effects

Identity operations participate in the same guarded effect model:

```text
ReserveLogicalIdentity(logical)
AllocateGeneration(logical, generation)
BindRemoteIdentity(generation, remote_id)
PromoteGeneration(logical, generation)
RetireGeneration(generation)
RecordAlias(old_logical, new_logical)
```

These operations target the kernel's transactional store rather than a cloud
API. They are included in the residual plan and journal.

Promotion is a commit point. Before promotion:

```text
active(worker) = g0
candidate(worker) = g1
```

After promotion:

```text
active(worker) = g1
retiring(worker) = g0
```

A crash before promotion leaves `g0` active. A crash after promotion records
that `g1` is active and `g0` still requires retirement. Promotion and journal
advancement must be transactionally coupled.

### Autonaming

Autonaming maps a logical generation to a distinct physical name:

```text
autoname(logical_id, generation, constraints) -> physical_name
```

Provider-specific constraints such as length, alphabet, and prefix are supplied
by Starlark libraries. The kernel supplies stable entropy or a stack salt and
persists the result.

A deterministic form is preferable where possible:

```text
physical_name = prefix + hash(
    stack_salt,
    logical_id,
    generation,
    naming_algorithm_version,
)
```

A frozen random suffix is also possible, but update may not silently choose a
new suffix after collision without amending the reviewed plan. Existing names
always remain persisted regardless of later naming-algorithm changes.

Different generations receive different physical names:

```text
worker@g0 -> worker-b71d
worker@g1 -> worker-f942
```

This enables create-before-delete while retaining one stable logical identity.
If the remote system requires a fixed unique name and cannot host two
generations, the Starlark lifecycle implementation must choose
delete-before-create or reject replacement.

### Explicit create-before-delete

Replacement is a visible Starlark strategy, not a provider action:

```python
def replace_instance(logical, old, desired):
    generation = identity.new_generation(logical)
    new = compute.create_instance(
        name = identity.autoname(generation, constraints = EC2_NAME),
        image = desired.image,
    )
    load_balancer.replace_target(old.id, new.id)
    identity.promote(logical, generation, remote_id = new.id)
    compute.delete_instance(old.id)
    identity.retire(old.generation)
    return new
```

The residual plan exposes allocation, creation, reference migration, promotion,
deletion, and retirement as separate ordered effects. The kernel supplies the
atomic identity mechanics but does not choose this strategy.

Stable identity is required for matching state, ownership, idempotency, retry,
output journaling, policy approval, and plan comparison.

## Residual loops and effect families

Pulumi-level expressiveness requires more than a finite list of effect nodes.
For example:

```python
for subnet in network.discovered_subnets:
    aws.route(key = subnet.id, subnet = subnet)
```

If the collection is an unknown resource output with unbounded length, preview
cannot enumerate its route operations. The residual plan needs quantified
operation families. The Starlark route reconciler may lower to families such
as:

```text
ForEach subnet in Output(network, "discovered_subnets"):
    ObserveRoute(key = subnet.id)
    CreateRoute(...) when route does not exist
    ModifyRoute(...) when route exists AND properties differ
    DeleteRoute(...) when the reconciler's desired-state condition is false
```

The report retains both API-level families and their semantic property
footprints.

Policy must reason about the family rather than only its currently enumerable
members. It may require properties such as:

```text
all route destinations are private
no route exposes 0.0.0.0/0
iteration keys are unique
collection length is at most N
```

Some properties will be unprovable until update. Policy can reject the plan,
require an assertion with a runtime check, or require an explicit bound.

Residual conditionals and effect families are the principal mechanism that
preserves expressiveness without allowing update to execute arbitrary source
code.

## API outputs and dependencies

A native API operation declares the shape and types of outputs it may produce:

```text
Output(E1, "id")
Output(E1, "region")
Output(E1, "endpoint")
```

References to these outputs induce dependencies. An effect cannot evaluate its
predicate or arguments until the required outputs are available.

The plan should distinguish:

* API outputs persisted as logical or physical identity state;
* transient execution metadata;
* secrets;
* values recomputable from journaled outputs;
* values requiring an explicit residual read during recovery.

Output schemas and native SDK versions are part of the plan integrity boundary.

## Completions and dependency control flow

Each operation has guarded completions:

```text
Normal(outputs)
Failure(error)          # known not to have committed
Indeterminate(token)    # may have committed remotely
Cancel(reason)
```

Dependent operations require normal completion of their dependencies:

```text
P(dependent) =
    operation_predicate
    AND Normal(required dependencies)
```

Failure, indeterminate completion, and cancellation are explicit control flow.
A connection can fail after a remote service accepts a mutating request, so the
runtime must not treat every transport error as definite failure. Native
operations provide idempotency keys, operation IDs, or recovery probes for
resolving `Indeterminate` without blindly duplicating an effect.

The update journal records completions and outputs. Starlark reconciliation may
participate in recovery, but native metadata required to identify an already
submitted operation is retained by the kernel.

## Retry and convergence

Infrastructure reconciliation often polls or retries:

```python
while True:
    status = api.get_status(id)
    if status == "ready":
        break
    sleep(backoff)
```

The residual plan cannot hide an unbounded number of effects. It needs explicit
bounded constructs such as:

```text
Retry(operation, retry_policy)
Poll(read_operation, until_predicate, timeout, max_attempts)
```

Preview reports cardinality and limits, for example that `GetStatus` may occur
up to 60 times over ten minutes. Transport retries that are semantically the
same idempotent request may remain inside a native primitive; semantic retries
and convergence polling remain visible residual control.

## Determinism

The term deterministic needs qualification because cloud APIs are external and
may fail or return different values.

### Deterministic plan

The sealed artifact fixes:

* all preview-known values;
* all effect shapes and effect families;
* predicates and residual expressions;
* dependencies and resource identities;
* assumptions;
* native SDK, schema, and reconciliation-library versions;
* policy decisions.

### Deterministic control given observations

Given the same sealed plan and the same sequence of cloud API outputs and
completions, update selects the same effects and computes the same arguments.

### Deterministic replay

Once an output or completion has been journaled, resume uses it rather than
repeating the earlier semantic decision. Native recovery may still query a
cloud operation by stable ID, but it reconciles that query with the journaled
node rather than introducing a new effect.

### No update-time effect discovery

Update may instantiate members of a persisted effect family. It may not invent
a new cloud API operation shape, identity transition, or unbounded callback
that was absent from preview.

A concise target invariant is:

> Every update effect is an approved instance of a preview effect whose
> persisted predicate evaluates to true using sealed values and journaled
> observations.

## Update journal and recovery

The journal should durably record events such as:

```text
operation predicate resolved
policy checked
cloud or identity operation started
remote operation ID assigned
operation completed, failed, or became indeterminate
outputs bound
physical generation promoted or retired
family member instantiated
assumption validated
```

Effects need stable idempotency or recovery keys. On resume, the kernel uses
the native API's recovery contract for an already-started operation rather than
blindly submitting it again.

A journaled output normally becomes immutable for the lifetime of the update.
If an API requires refresh, the refresh is itself a planned observation with
explicit invalidation and policy semantics.

## Native API and SDK contract

The privileged native layer should be narrow. It handles:

* authentication and request signing;
* wire protocols and serialization;
* sending requests and decoding responses;
* API input and output schemas;
* typed classification of completions;
* idempotency keys, remote operation IDs, and recovery probes;
* API-level effects and semantic mutation footprints;
* assumptions and validation tokens for observations;
* secret propagation;
* implementation and schema digests.

It does not contain desired-state comparison, property diff logic, replacement
decisions, lifecycle policy, or application-specific defaults. Those belong in
Starlark reconciliation libraries.

An unknown or incomplete semantic summary contributes
`UnknownCloudMutation`, with conservative targets and completions. Policy
decides whether that uncertainty is acceptable. Native code must not perform an
undeclared cloud mutation during preview or update. Preview-time reads are
observation effects whose results and assumptions are recorded.

## Why a controlled Starlark language helps

Pulumi programs run in general-purpose host languages. An output callback can
perform arbitrary computation and host I/O, and it may declare resources only
after update-time outputs are known. That makes complete preview difficult.

A controlled Starlark embedding offers:

* specified evaluation semantics;
* interceptable native operations;
* no host I/O except exposed capabilities;
* analyzable control flow;
* symbolic execution over resource outputs;
* residualization of unknown-dependent computation;
* effect summaries for native functions;
* a practical path to a closed update artifact.

The type system can additionally refine output schemas, pure operations,
SDK inputs, predicates, and policy reasoning. Type information improves
precision but is not a substitute for effect and completion summaries.

## Relationship to OPA residual programs

OPA partial evaluation can be summarized as:

```text
policy + known data
    -> residual predicate over unknown input
```

The proposed IaC pipeline is:

```text
program + known data + symbolic resource outputs
    -> residual effects guarded by predicates over those outputs
```

The IaC residual program additionally needs:

* effects that introduce new symbolic outputs;
* dependency ordering;
* desired-state reconciliation;
* stable resource identity;
* failure and cancellation completions;
* persisted values and assumptions;
* policy over possible effects;
* execution journaling and recovery.

The design is also an instance of binding-time analysis and multi-stage
programming: preview is the static stage and update is the dynamic stage.

## Proposed pipeline

```text
user Starlark
    |
    +-- resource abstractions
    +-- Starlark reconciliation libraries
    +-- lifecycle and replacement strategies
    |
    v
resolve and typecheck
    |
    v
partial evaluation with symbolic API outputs
    |
    +-- execute or residualize cloud observations
    +-- freeze known values
    +-- collect guarded cloud API operations
    +-- collect semantic property footprints
    +-- collect guarded identity-state operations
    +-- residualize unknown pure computation and bounded control
    +-- collect assumptions and completions
    |
    v
policy over API operations, semantic footprints, and state effects
    |
    +-- static rejection or approval
    +-- persisted runtime checks
    |
    v
seal and persist residual plan
    |
    v
plan runtime/kernel
    |
    +-- validate assumptions
    +-- resolve residual predicates
    +-- enforce policy before every effect
    +-- dispatch only planned native API operations
    +-- execute transactional identity operations
    +-- instantiate only planned effect families
    +-- bind and journal outputs and completions
    +-- recover interrupted or indeterminate operations
```

There is no provider-planning stage. The partially evaluated Starlark
reconciliation program is the planner.

## Initial implementation phases

### Phase 1: finite residual API plans

Support:

* known values and symbolic API outputs;
* a small residual expression language;
* guarded native API operations with mock read and mutation primitives;
* API-level effects and simple semantic property footprints;
* finite conditionals;
* persistence and deterministic display;
* update interpretation without source reevaluation;
* output and completion journaling;
* policy that distinguishes known from possible effects.

Do not initially support unknown-length loops, replacement, or arbitrary native
SDK plugins.

### Phase 2: logical identity and recovery

Add:

* logical resource IDs and aliases;
* physical generations;
* deterministic or frozen autonaming;
* remote-identity bindings;
* transactional promotion and retirement;
* idempotency keys and indeterminate completions;
* locking, state revision assumptions, and interrupted-update recovery.

Demonstrate create-before-delete as an explicit Starlark reconciliation
strategy over these primitives.

### Phase 3: saved values and observations

Add:

* capability-controlled filesystem and environment reads;
* frozen preview values with provenance;
* preview-time cloud observations;
* remote observation assumptions and validation;
* residual update-time observations;
* secret taint and encrypted persistence;
* plan sealing and source, SDK, and library digests.

### Phase 4: SDK contracts and richer reconciliation

Add:

* typed native API schemas and outputs;
* property-level mutation footprints;
* expected-error normalization;
* bounded retry and polling control;
* host-region identity for cloud objects;
* conservative `UnknownCloudMutation` behavior;
* Starlark libraries implementing update, deletion, replacement, import, and
  adoption strategies.

### Phase 5: effect families

Add:

* residual iteration over unknown collections;
* deterministic member keys and generation identities;
* family-level policy;
* collection bounds and uniqueness obligations;
* journaled family instantiation.

### Phase 6: richer policy and explanation

Add:

* predicate simplification and optional solver integration;
* implication and satisfiability checks;
* approval workflows for possible API and property effects;
* explanations of unknowns and widening;
* persisted policy proofs or evidence;
* policy enforcement during update.

## Open questions

The following issues require iteration:

* Which Starlark operations may remain in the residual expression language?
* Which reconciliation abstractions should be language conventions versus
  standard Starlark libraries?
* How are logical identities and aliases derived across refactoring?
* Which kernel identity operations must be primitive, and which can be composed
  in Starlark?
* Which observations must invalidate an entire plan versus one operation?
* How are cloud refreshes represented without violating frozen outputs?
* What policy language expresses conditions over effect families?
* What assertions may users supply about unknown outputs, and how are those
  assertions checked at update time?
* How should a runtime policy denial handle already-completed dependencies?
* Which outputs are immutable for an update, and which may legitimately change
  during native API recovery?
* How much solver support is necessary before predicate explanations are useful?
* How are plan amendments represented without turning update back into source
  reevaluation?

These questions do not change the central architecture: preview produces and
persists the residual effectful program, and update is a constrained
interpreter for that artifact.
