# Local native HTML Demo seed

This one-shot, offline assembler uses the existing `crewd-collab`/`crewd-store`
functions and the unchanged production SQLite-knowledge edit compiler. It does
not run `crewd`, a provider, a model, a network client, or an executor.

`content.json` is a curated **two-crew / ten-channel** slice of the actual React
product tour after its current presentation/content refresh chain. Regenerate
it from the repository root with the already-installed tour loader:

```sh
node --import ./design/prototypes/2026-10-14-react-product-tour/node_modules/tsx/dist/loader.mjs scripts/dev/native-demo-seed/export-content.mjs
CARGO_TARGET_DIR=/private/tmp/pcw-decision-target CARGO_INCREMENTAL=0 cargo build --offline --locked --manifest-path scripts/dev/native-demo-seed/Cargo.toml
```

The seed executable accepts no arguments. It creates only the **absent**
`$HOME/Library/Application Support/PendingCrewDemo` with private directories.
Do not point `HOME` at real daily data or run it on an existing Demo root. An
existing entry, including an unfinished seed, is a hard refusal; nothing is
removed, adopted, reseeded, or overwritten. The actual Demo App uses the macOS
user home rather than this assembler's shell environment.

The native App must be built with `PENDINGCREW_LOCAL_MAC PENDINGCREW_LOCAL_DEMO`,
without `PENDINGCREW_REMOTE_PRODUCT`, DEBUG, HotReload, or Apple service flags,
and with bundle ID `com.pendingname.pendingcrew.demo`. Ordinary opening validates
the fixed private root, ready marker, and regular unlinked database; unsafe or
missing state refuses startup rather than falling back to daily data. The
original local-only credential and enrollment factories refuse real vaults.
Both executor programs are always `/usr/bin/false`, after all other sources.

The curated records contain 40 original messages, 16 Agent requirements, eight
Human Todo examples, 42 plan examples, and 38 native saved documents. The actual
snapshot retains original IDs, text, source hashes, and historical statuses.
Native plans are created as unassigned open examples, with original status and
owner retained in the goal; they are not active execution. Human questions are
ordinary inert Human Todo text, not manufactured live approvals. Documents use
production `DataEdit` compilation and `Knowledge.apply`, then the original
`knowledge_channels.change` reference association. Original table/dataView
examples are preserved as static document data, without pretending an HTML
interactive widget is the native renderer.

All 11 original sample roles are registered through `sessions::register` as
Stopped. Captain designation is applied only after content, avoiding a new
captain's requirement wake during seeding; the original `wakes::human_stop`
persists each role's automatic-work pause. Final guards require stopped/nonworking
roles, no pending/inflight wakes, empty native runtime/input/receipt tables, and
SQLite quick_check. No session start or execution receipt is fabricated.

`source-map.json` records the new native IDs alongside HTML IDs. `demo-ready.json`
is written last, after checks and checkpoint. App startup never runs the seeder;
ordinary reopen therefore preserves subsequent Demo edits.
