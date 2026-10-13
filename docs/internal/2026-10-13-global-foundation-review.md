# Independent organization foundation review

Frozen scope: fee678cd (including 487f213b scoped core and 1741a16c attribution correction). Read only using git show for frozen sources. Current HEAD/ongoing production cutover were not accepted by inference. No Cargo, model, app, or real-data migration performed.

## Result

No actionable blocker found in the frozen pure-core/store foundation. Existing logs were inspected, not independently rerun. This checkpoint does NOT establish production cutover, schema-9 activation, API/MCP/actor acceptance, or full A-to-B-to-A delivery.

## Identity and conversion

merge_legacy_books orders crews deterministically, reserves each prior issued W/H counter range including holes, and computes explicit type-specific maps. Checked offsets reject exhaustion rather than wrapping. Existing authoritative # entries receive deterministic disjoint offsets; their external registry numbers stay unchanged. Item parent/dependency/source/blocker references, HumanAsk.item and stop-streak item manifests are translated through their respective maps. Item revision, owner history, pending transfer, reports, timestamps, missed checks, ask state and entry closures are copied without artificial commands or new revisions. Clock is max(source clocks). Session stop streaks combine counts by max and escalation by OR, not by sum. Persisted global stop-check budget tables and public wake/receipt numbers are not rewritten.

Store migration maps each prior work: alias using exact crew/type/local identity. It rejects dangling issued refs or ask_meta references and requires every authoritative object to have a prior published reference. Registry number/source_id/created_at remain unchanged. New objects bind organization:<core> aliases with the existing AUTOINCREMENT public registry, preventing collision with old work:<local> aliases. Type+core mapping is unique. Existing references remain resolvable by their old crew aliases; callers must then obtain core_id before addressing the new authority.

## Permissions and graph

ScopedSession obtains a role for the actual object's crew; absent scope falls back to Worker. Unscoped legacy Session captain/admin roles also downgrade to Worker in organization mode. Ownership and transfer-target rights remain identity-based. Creating children, assigning owners, relinking parents, withdrawing asks, claiming/closing entries and administrative completion pass object crew context into permission checks. Trusted scope construction remains a production integration responsibility; the pure serializable type is not evidence that untrusted RPC scope is accepted.

Graph direction is unchanged: parent -> child; work -> dependency/blocking work. Whole-book invariants remain applied at restore/write, so parent waiting on child stays legal and child waiting on ancestor forms a cycle. Organization max_parent_depth is pinned to 3. Depth is limited only along parent chains. Crew attribution cardinality and nonblank identities are checked; standalone asks require explicit crew when there is no work/home inference. Existing original single-crew recovery remains migration input, not parallel writable authority.

## Events, persistence and rollback

Migration emits no invented historical events. Original legacy work/event data remains original source evidence; legacy per-crew sequence provenance is stored separately and organization last_seq starts at zero. New save requires exactly contiguous real events and a matching resulting sequence, inserts attribution-tagged organization events, and writes the single singleton state within the caller transaction. It checks reference integrity and prevents disappearing issued nodes/counter regression.

Migration function requires an existing transaction and deliberately does not set user_version or activate the production version chain. Formal schema 9 activation must wrap it with the normal backup/version guard. Existing tests cover failure before schema change, injected failure during registry remap, discarded state+event+ID transaction, immutable old states, retry and restoring a pre-migration SQLite snapshot. They do not establish authorized migration of daily data.

## Cutover facts requiring explicit attention

1. After migration work_books is a READABLE legacy evidence view. It rejects writes, but old read paths can silently return stale pre-migration data. Every context/checkpoint/idle/patrol/ask/detail and render/claim read must change before production schema 9 activation; merely switching writes is insufficient. This was sent to root and implementation author.
2. External W/H/#, wake source/targets and receipt IDs intentionally stay stable. Only local BookState references change. Runtime must translate every external handle via registry core_id; core IDs are not public IDs.
3. Current CAS revision remains preserved because conversion modifies fields directly rather than running transact. Public CAS may retain the old expected revision against the mapped node. Genuine subsequent commands increment revision normally.
4. organization_work_events has its own new sequence beginning at 1. It must not be merged as if it continued an old per-crew seq; old evidence requires provenance.
5. old books are archived immutable evidence and may remain readable for audit. They must never be consulted as live authority after activation.

## Inspected evidence

- docs/superpowers/plans/2026-10-13-global-work-flow.md at fee678cd.
- /private/tmp/pcw-global-work-core-final-03.log: 149 tests passed (130 unit, 2 fuzz, 14 organization, 3 scenarios).
- /private/tmp/pcw-global-work-store-final-01.log: 139 tests passed, 2 environment/cost tests ignored.

The implementation plan already states the atomic cutover boundary and version-8-before-version-9 order. Missing in-flight production APIs were not classified as defects of this explicitly staged checkpoint.
