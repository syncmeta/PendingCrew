# Independent incremental review: f990bf71

Reviewed commit f990bf71195b260d56d976a88c61a85f4947c182 in decision-backends/PendingCrew. Read-only source/diff and existing acceptance-log review. No Cargo command, real model, app launch, or daily-data migration performed.

## Result

No actionable correctness/security blocker found in the reviewed increment. This is independent source review, not independent re-execution of the tests. Root owns the final combined gate.

## Findings checked

- Raw evidence fidelity: production Codex usage parsing retains the parsed tokenUsage object, including unknown nested members, null, missing fields and explicit zero. Canonicalization changes key order/whitespace; it does not pretend to retain original wire bytes. serde_json signed/unsigned 64-bit integers are not routed through JavaScript numbers. Existing exec/store/socket evidence explicitly covers 9007199254740993 and 9007199254740994.
- Ownership: CodexCore rejects notifications for other explicit threads and requires its active thread. Orchestra Core first rejects missing live session/stale generation, then independently verifies snapshot.thread_id against current agent_session_id. No tracker-derived turn is fabricated.
- Dedup: turn_id_json is NOT NULL and stores JSON null or a JSON string, so null-turn records participate in the SQLite UNIQUE key. Missing and null provider turn intentionally share the unknown identity; empty explicit string remains distinct. Session/provider/thread/turn/canonical snapshot all participate. Exact repeats survive restart; changed cumulative snapshots are retained, never summed. Identical unknown-turn observations may collapse even across different logical turns; this is the expressly documented snapshot policy, not complete per-turn accounting.
- Bounds: notification serialization stops at 64 KiB; store repeats payload/object validation, enforces 4 KiB session/thread/turn identities and 128-byte provider. Schema imposes object/JSON/byte constraints. List pages fetch at most limit+1 with limit 1..200. Cursor must be canonical positive decimal within i64; IDs are serialized as strings. Session-scoped SQL prevents another session cursor from exposing its records.
- Persistence: DbPort uses the normal single-writer transactional write path. Core Usage branch returns directly without publishing, updating progress, enqueueing wakes or restarting the runner. The tested real SQLite trigger refusal leaves no record and populates database health; evidence is not replaced by fabricated zero.
- Authorization: session.usage is a human RPC, not an agent method; conn kernel authority / peer authorize_method rejects managed and unknown peers. Existing session lookup occurs before reading usage, so unknown sessions are rejected. Current design gives authenticated Human access across their local sessions intentionally.
- Schema 8: formal migration is transactional CREATE TABLE/index plus user_version update, not IF NOT EXISTS. Newer schema guard and pre-migration protection remain in existing owning migration code. Historical version fixtures explicitly remove v8 table when emulating old schemas; they do not weaken production migration.
- Cross-layer tests are not mirror implementations: db.rs usage_from_provider_core constructs the production CodexCore, completes its actual initialize/thread/start actions, feeds on_line notifications and extracts actual Usage events. Actual orchestra Core.handle and DbPort persist to SQLite. Reopen and refusal tests use real stores; socket test separately seeds store evidence and tests public RPC pagination. Controlled executor is synthetic and is correctly not claimed as live provider acceptance.

## Evidence inspected

- docs/internal/2026-10-13-usage-evidence-acceptance.md
- /private/tmp/pcw-usage-exec-final.log: 295 passed.
- /private/tmp/pcw-usage-db-core-02.log: 3 targeted tests passed.
- /private/tmp/pcw-usage-socket-01.log: 1 passed.
- /private/tmp/pcw-usage-store-final-02.log: 87 unit + 49 integration passed, 2 ignored.
- /private/tmp/pcw-usage-clippy-final.log.

## Nonblocking evidence limits

Current tests demonstrate >JS-safe integer preservation, not explicit i64::MIN/u64::MAX boundary cases. Code uses the unchanged serde_json 64-bit parser; no lossy intermediate was found. Missing-to-null turn equivalence follows directly from Option parsing and NOT NULL serialized JSON storage, but the end-to-end fixture only repeats the missing-turn form. Adding extremes and explicit null replay would strengthen evidence if desired; neither is a reproduced fault. No live-model accounting accuracy or pricing/classification/client UI acceptance is established by this increment.
