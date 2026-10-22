# Codex session controls, native inputs and history — client contract

Status: public DTO/behavior contract frozen for parallel client implementation on 2026-10-23. Backend source and fault qualification are still in progress. This is not provider/account runtime or product delivery acceptance. The source of truth for field names is `crewd/crates/proto/src/{codex,codex_inputs,commands,orchestra}.rs`. API remains 3; the existing APNs Store19 is retained, with per-session Codex metadata version2 when this lifecycle is used. A necessary old-writer fix, if evidence requires one, will be reported separately; it does not authorize client fallback to older behavior.

## Existing command envelope and result recovery

Every native mutation uses `command.submit` with the existing `{requestId,dbId,runtimeBindingId,command:{operation,params}}`. Capture a fresh `session.controlTarget {sessionId}` before an intentional new action. `params.target` is the complete original `CommandRuntimeTarget`; never rebuild it from a thread ID. The existing `command.lookup {dbId,requestId}` is the only original mutation-result recovery. Keep the original immutable submitted envelope. A timeout/closed connection/Unknown never creates a replacement UUID or automatically resends a task, attachment, settings change or history action. Lookup is read-only and cannot resume a runtime.

Existing phases retain their meanings. In particular, queue's `confirmed` + `application:queued` confirms local next-turn queue admission, not native delivery, turn completion or business success. Settings ACK confirms accepted/desired-next-turn, while `settings`, `settingsObservedAt`, `historical`, `desiredSettings`, `settingsStatus` and native observations describe separate effective evidence. Do not require a provider field echo that its protocol does not promise. Existing native title intent/result remains separate from task submission.

The public command payload stays at 256KiB; native input bytes use the separate bounded staging methods below, not an enlarged command or `message.post.attachments`.

## One-shot native input lifecycle

Four new direct typed RPCs are added. The first three stage bytes/results only and do not start a process or submit a task. They require the captured current Local Human or live paired device authority; attached Agents are refused. Owner scope includes actual Core db identity, captured principal and crew. Another device/grant cannot use these original references. Existing crew access and current grant authorization are independently checked at transaction/dispatch.

`session.codex.inputPrepare` params:

```json
{"requestId":"UUID","crewId":"crew","sessionId":"optional existing Codex session","kind":"image|audio|text","filename":"display-name.ext","mimeType":"image/png","sha256":"64 lowercase hex","bytesBase64":"canonical standard padded base64"}
```

Omit `sessionId` for a new-session input. An existing session must belong to the original crew and be a currently usable Codex source. Result is `{"operation":"prepared","input":{"inputId":"UUID","sha256":"64 lowercase hex","kind":"image|audio|text","bytes":123}}`.

`session.codex.inputLookup {crewId,requestId}` returns `{"state":"notFound"}` or `{"state":"known","result":<original prepared/discarded result>}`. This is the immutable original staging receipt, not a claim that a previously consumed input is unused or delivered. An interrupted prepare is recovered only through its original request lookup; do not reupload with a new UUID. A known consumed original reference cannot be reused to resubmit an uncertain task. `notFound` itself does not prove a cancelled writer can no longer commit; keep the original pending request until its owning operation has a genuine terminal boundary.

`session.codex.inputDiscard {requestId,crewId,input:<complete original input reference>}` returns `{"operation":"discarded","inputId":"UUID"}`. It only discards prepared, unused original bytes and keeps the original receipt. Consumed/Unknown task inputs cannot be discarded or silently replaced. Recover this operation using `inputLookup` with its own original discard request UUID.

A selection has the exact shape:

```json
{"inputs":[{"inputId":"UUID","sha256":"64 lowercase hex","kind":"image|audio|text","bytes":123}],"skills":[{"name":"catalog name","path":"exact native catalog path"}]}
```

Omit either empty array; at least one total element is required, at most 16, with no duplicate input IDs or skill paths. The only permitted server path is an exact enabled skill catalog selection. Client paths, URLs, data URLs, arbitrary provider file IDs and arbitrary binary/PDF files are not upload substitutes.

- `session.start` gains omitted optional **`codexInputs`** containing that selection. It is allowed only inside original `command.submit` for executor `codex`; raw legacy `session.start` rejects it. Original registration, reference consumption, session binding and start receipt registration share one Core transaction. A startup failure/Unknown does not make consumed bytes reusable. Explicit later manual resume never automatically replays original consumed attachments or the first task.
- `session.codex.queue` and `session.codex.steer` command params retain `{target,text}` and gain omitted optional **`inputs`** containing the selection. Steer requires the exact active `target.turnId` and never falls back to queue. Queue is next-turn admission. Text remains the exact original Human string and must remain nonempty within its existing 128KiB limit; attachments are separate native input elements.
- Enabled skills come from the existing `session.codex.tools {sessionId}` `skills` array (native grouped `{cwd,skills,errors}` entries). Select exact `name` and `path`, only `enabled:true`. The backend rechecks the original actual workspace catalog before sending; missing, disabled, ambiguous or foreign-workspace entries reject, with no fallback to arbitrary paths.

Bounds: decoded media <=4MiB/file, UTF8 text <=512KiB/file, selected text total <=1MiB, 16 total refs/skills, 64MiB retained/session, 256MiB/Core, existing 16MiB frame. Display filename <=128 UTF8 bytes with no control or path separator. Supported MIME/magic checks: PNG/JPEG/GIF/WebP; WAV/MPEG/MP4 audio; `text/plain` valid UTF8 without NUL. Provider decoding/model modality support is a separate runtime outcome, not inferred from MIME. Immutable private bytes are held in the original SQLite backup domain; materialized media copies use owned0700 directories and0600 create-new files held for the physical runtime. Text files become separate labelled native text elements, not hidden edits to the Human task.

## Native permission profiles

`session.codex.permissionProfiles {sessionId,cursor?,limit?}` returns `{items:[{id,allowed,description}],nextCursor}`. It is read-only for the actual original session workspace, can use the bounded metadata-only child when stopped, and does not resume a thread. Limit is1..100, cursor <=8192. `description` and `nextCursor` may be null. Existing session identity/workspace must be available; no arbitrary cwd/config lookup.

Existing `CodexSettingsPatch` (both `session.start.codexOptions` and `session.codex.configure.params.patch`) gains omitted optional **`permissions`**, the exact catalog `id`. It is mutually exclusive with `sandbox`. The backend verifies the current catalog's selected `allowed:true` ID before writing settings, with bounded pagination, original permit and Stop fencing. A missing/changed/disabled catalog rejects without a settings write. Initial settings qualification/ACK gates the first prompt. Existing observed `settings.permissionProfile` reports the actual native active profile ID when provided; absence does not synthesize effectiveness. No global config is written.

## Native history management

New closed `command.submit` operations:

| operation | params besides original `target` | execution eligibility |
|---|---|---|
| `session.codex.archive` | none | physical original runtime has exited |
| `session.codex.unarchive` | none | physical original runtime has exited |
| `session.codex.delete` | `confirmNativeHistoryDelete:true` | physical exit plus explicit Human confirmation |
| `session.codex.revert` | `beforeTurnId` | already loaded, observed `paginated`, live idle, no queued input or pending Human decision |

These invoke only official `thread/archive`, `thread/unarchive`, `thread/delete`, `thread/revert`. No implicit start/resume; unarchive does not launch. Delete tombstones the native binding while keeping PCW sessions, work, posts and original receipts. Revert truncates retained conversation history before the chosen original turn; **it never reverts workspace files**. Native ACK is history-operation acceptance, not business success.

`session.codex.inspect` gains omitted optional **`historyMode`** (only actually observed native thread mode) and **`nativeHistory`**:

```json
{"status":"active|archived|deleted|unknown","requestId":"optional original UUID","operation":"optional archive|unarchive|delete|revert"}
```

Missing nativeHistory means no PCW-managed lifecycle observation, not a claim of upstream active state. Native Unknown blocks later native mutations/resume/replay; archived permits only explicit unarchive/delete; deleted cannot be resumed. Unknown is queried through the original command receipt, not repaired by a new mutation UUID. Ordinary PCW readonly data and independent metadata catalogs remain readable. Revert requires actual `historyMode:"paginated"`; a requested value is not observation, and legacy threads are never silently converted. New native threads explicitly request paginated history.

Title/fork/history mutations share the original one-owner per-session metadata gate, including reaping and durable settlement. Parallel inspect/history/models/tools/profile reads remain independent. Stop/hibernate/generation revoke the original permit; an old late ACK cannot declare the new generation synced. No history RPC is a raw command or private Codex database edit.

## Session UI capability matrix

Read `inspect.capabilities` by exact ID and availability (`available`, `requiresRunning`, `unavailable`, `providerRejected`, `pcwNotImplemented` from the existing enum), not a schema number or a guessed provider version. Protocol `experimental` is an honest wire qualification boundary, not account/model success.

| capability ID / existing surface | actual backend route and user meaning |
|---|---|
| `configure` | model, effort, fast/service tier, structured approval policy/reviewer, sandbox **or** named permissions, plan/default collaboration, summary; accepted vs observed-effective separate |
| `compact` | actual live compact request; ACK is not compact completion |
| `queue` / `steer` | next-turn queue vs exact current turn; original task preserved; optional prepared inputs |
| `interrupt` / existing stop | exact native current-turn interruption vs physical session stop; original stop receipts retain original birth proof |
| `review` | official typed review targets, original idle thread; no raw provider method |
| `title` | explicit Human title wins; native name/set; failure never resends task |
| `history` | native bounded paginated history read, no implicit resume or private transcript scan |
| `resume` | original explicit manual resume only, blocked on fork/history Unknown or native tombstone |
| `fork` | native original-thread fork plus independent PCW receipt; no prompt or automatic launch |
| `models` / `tools` | bounded native model/MCP/skill catalogs; no install/global-config side effects |
| `attachments` | one-shot prepared localImage/localAudio/separate UTF8 text; binary/PDF unavailable |
| `skillInput` | explicit current enabled skill only; not tool/plugin installation |
| `permissionProfiles` | actual current original cwd catalog; selection revalidated before write |
| `archive` / `unarchive` / `delete` / `revert` | lifecycle rules above, no automatic resume or file rollback |
| existing working directory | original session/worktree context, not arbitrary live process cwd replacement |
| existing runner requests / approvals | original structured pending request + exact answer receipt; no automatic broad approval |
| existing `session.usage` | persisted native token/context/usage read; no inference call |

Unimplemented private Codex App options, arbitrary binary uploads, plugin/skill installation, global config writes and vendor-private controls remain outside this native supported surface. Claude still uses true PTY managed tasks: no provider full-history/load or fake ACP/native completion is granted by this Codex module. APNs observations do not upgrade that capability. Native16 stays inactive.

`session.codex.changed {sessionId}` remains the refresh event for both clients. Staging result lookup is direct/read-only; real task/history/settings mutations use original command receipts. Neither reconnect, clicking a cached result nor loading a page executes or restores a task.
