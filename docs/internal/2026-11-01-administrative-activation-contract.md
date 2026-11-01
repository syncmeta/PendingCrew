# Controlled automatic administrative execution

Baseline: `722788b9`. This is an implementation contract, not a completion claim.

The original backend design §8.4 requires launching an administrative session only when a real matter needs action, then releasing its process after its work or explicit handoff. The current production administrative judgment persists its result but has no caller of the trusted Core factory. The next module connects that gap using the lifecycle already implemented. It does not add another Work ledger, executor factory, permission system or credential owner.

## Scope and authority

Consume complete immutable typed escalation sources and their current, adopted native judgments. Existing typed cases may be considered if still current and never dispatched/bound; untyped historical announcement import is excluded. Keep the current same-crew boundary and original executor/model/identity. No fallback to another executor or account. Unsupported restricted Claude and an unavailable original native authentication owner are explicit waiting/refusal reasons, not invitations to launch something else.

Use the existing judgment configuration and enabled state. Do not add a second product switch solely to avoid existing test fixtures. Unconfigured, disabled, low-confidence, generated, missing or stale judgments create no executable grant. Current configuration, Stop, restore, source, captain and target authority are rechecked through the original permit/registration path. A source change during startup still releases no first task. Wider organisational administration remains a separate explicit scope design.

## Scheduling and recovery

Add a durable scheduling record, not a second execution outcome. It describes pending eligibility, concrete waiting reason, actual claim and unknown initiation. The existing bound execution record remains authoritative for session identity, process generation, work effects and execution state. Pure list/get projects this information without creating a schedule or dispatching a task.

Runtime scans bounded source pages fairly and avoids duplicate in-flight commands. Waiting has a stable reason and bounded reprobe/backoff; it does not reevaluate the model or rewrite identical status on every tick. Core uses typed preflight results for capacity, executor policy, live original auth owner, admissions and workspace. Do not parse error-message strings to make scheduling decisions.

Prefer the persistent single-winner claim inside the Core actor after preflight and before registration, sharing the registration transaction where possible. Before that claim no external task may be dispatched. After a claim, failed/unknown startup and daemon restart must preserve Unknown rather than allocate another case session or replay its business task. Concurrent commands cannot produce a second binding. A preclaim queued command lost with the daemon has no external effect and may be considered again under current facts; this is distinct from replaying a claimed attempt.

Disabling/changing existing configuration retains its current invalidation semantics. Core shutdown/restore/source loss fences existing administrative generations through the existing native lease. Registration or launch uncertainty must not become Done, Parked, successful handoff or an empty responsibility.

## User-visible behavior and acceptance

The native case detail shows the scheduling state and concrete waiting/unknown reason alongside the existing decision, execution, outcome, responsibility and retirement records. It must not label an adopted judgment as an already running session. Historical/late connection replies cannot replace current state.

Independent acceptance starts with a real typed stop escalation and the production periodic judgment, with no manual `start_admin_case` call. It observes one actual binding, lawful typed effects and periodic verification, then genuine exit or explicit handoff/retirement. Cover repeated ticks, concurrent commands, capacity recovery, disabled/unconfigured/low-confidence decisions, unsupported adapter or absent live owner, startup revocation and restart without replay. Preserve the prior native profile and first-delivery gates.

Existing unit tests that deliberately exercise the manual factory may explicitly isolate the new automatic pump through an internal test-only Runtime option. Production defaults remain enabled, and new acceptance must use the production-enabled pump. This isolates a module under test; it cannot globally disable automatic behavior in the new suite or weaken any native gate.

Separate evidence layers: StandIn/Core/socket and synthetic native CLI proofs, live provider/account acceptance, same-source installed application, GUI acceptance and release. Finish the affected source/test/schema/client integration before reporting the module complete. Existing unrelated untracked design files remain untouched.

## Source acceptance checkpoint

The source module and affected integration gates are complete at this checkpoint. The independent automatic Core suite passes ten; the complete related server judgment suite passes 100 and ordinary Core suite passes 71. The actual production-native combination automatically starts, calls the real crew helper, handles one 401 through the captured original authentication manager and records exactly one close effect, then confirms physical exit before cleanup after source admission revocation. Provider/model endpoints and credentials in these proofs are synthetic. Full Mac compilation, Swift fixtures, production clippy, affected test targets and public schema parity pass. Detailed evidence and remaining installed/live-account boundaries are in the decision implementation ledger's controlled automatic execution section.
