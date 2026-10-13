# C4 raw usage evidence acceptance (2026-10-13)

Implemented provider notification -> actual Codex Core -> SessionEvent -> orchestra Core/DbPort single writer -> schema 8 SQLite snapshots -> `session.usage` paginated RPC and CLI. Records retain explicit provider/thread/turn identity, first actor observation time and canonical JSON of the already-parsed tokenUsage object. JSON text and decimal-string record IDs preserve tested integers beyond JavaScript safe precision. Missing fields remain distinct from explicit zero; unknown members remain present. Different cumulative snapshots are retained rather than summed; exact canonical duplicates are idempotent across restart. A missing/null turn remains unknown and is not inferred from tracker state.

Boundaries: 64 KiB canonical usage payload, 4 KiB identity, 128-byte provider; RPC pages default 50 and max 200. Old actor generation and foreign thread snapshots are rejected. Usage writes do not update progress, enqueue wakes, publish live refresh or restart runners. Actual SQLite refusal records existing database health fault and creates no fabricated usage receipt. There is no pricing conversion, billed-cost estimate, task classification, client UI or new model run in this unit. Canonical JSON retains parsed numeric values, not original whitespace/wire bytes; tested integer range uses the existing serde_json parser (signed/unsigned 64-bit), not arbitrary-precision provider numbers.

## Offline evidence

- `/private/tmp/pcw-usage-exec-final.log`: 295 exec unit tests passed, including production notification boundary and existing compaction behavior.
- `/private/tmp/pcw-usage-db-core-02.log`: 3 targeted actual Core/DbPort tests passed; provider notification, duplicate/different snapshots, durable reopening, old generation/thread rejection and real SQLite refusal.
- `/private/tmp/pcw-usage-socket-01.log`: actual Unix-socket RPC pagination passed, exact raw JSON, >JS-safe cursor IDs, unknown session and invalid limits/cursors.
- `/private/tmp/pcw-usage-store-final-02.log`: store full suite passed (87 unit + 49 integration, 2 ignored). Earlier `/private/tmp/pcw-usage-store-final.log` retained three old-version fixture failures; fixtures now remove schema 8 runner_usage before rolling back their version. Formal production migration remains strict CREATE TABLE and is not masked by IF NOT EXISTS.
- `/private/tmp/pcw-usage-cli-every-method-final.log`: actual CLI every-method test passed; usage empty response means unavailable evidence, not fabricated zero. Nonempty pagination is covered by actual socket test.
- `/private/tmp/pcw-usage-schema-final.json` and `.log`: successful CLI schema generation before replacing committed artifact.
- `/private/tmp/pcw-usage-fmt-final-02.log` and `/private/tmp/pcw-usage-clippy-final.log`: format check and locked offline workspace/all-target clippy with warnings denied passed.

The first formatting failure and prior failed migration logs are preserved. The main agent owns independent review and final combined workspace test gate. No push, main merge, app install, real model call, or daily-data migration is claimed by this acceptance.
