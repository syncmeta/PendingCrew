# M4 Claude true-PTY through ACP: complete backend unit

Baseline: b06489a092e051e7d62fa1376a26cca9b8629a08, isolated branch `codex/claude-pty-acp-20261014`. M2/M3 global candidate sources and immutable normal helper provenance are frozen separately. This is an implementation and acceptance plan, not delivered compatibility.

## Scope and reuse decision

Core keeps native Codex app-server. Claude is a real interactive CLI child with controlling PTY, environment filtering, no `-p`/`--print`, no prompt argv, and the existing native permission system. `crewd` remains sole team/work/permission/store authority. The adapter does not read personal transcripts or auth during this unit; tests use actual PTY with fake CLI/ACP peers and private synthetic config.

Use an isolated same-toolchain ACP helper mode built from PendingCrew's existing exec/term driver. The daemon speaks ACP to the helper; the helper owns native Claude PTY. Reuse the existing delivery state machine, input draft guard, ANSI/UTF-8 emulator, native decision fingerprint/keys, quotas and process-group stop. Do not replace these with the candidate's simpler screen/Enter retry implementation or duplicate team logic.

Pinned upstream comparison, source-only: `moabualruz/claude-code-cli-acp@ef7a01436fb4b552f66dabb7bac0e9d2b46e31c0`. Source manifest and fetched relevant files are preserved under `/private/tmp/pcw-m4-upstream-ef7a0143`. Its Cargo declares Rust1.95 and unstable ACP SDK0.11.1; source reads default `.claude/projects` transcripts for completion/load; terminal geometry is only set at spawn; prompt argv is optionally supported; native child kill/ctrl-C and synchronous writes do not establish our required process-group/confirmed-delivery contract. These are static comparison facts, not executed upstream failures. No runtime code or artwork copied. Apache-2.0 LICENSE and NOTICE are retained in research evidence; any later selective copied code requires explicit attribution/notice preservation.

## Wire contract and capabilities

ACP control is stdio JSON-RPC2.0 with bounded frames and outstanding requests. Stable v1 initialize, session/new, session/load, session/prompt, session/update, session/cancel. Prompt accepts text only in this unit; images, embedded resource bodies, arbitrary tool terminals and unverified dynamic model/mode switching are explicitly unsupported. CLI model/effort launch configuration remains available through the negotiated native-launch metadata. Native usage unavailable is unknown, not fabricated zero.

Native capabilities are advertised under `agentCapabilities._meta["pendingcrew.native"]`, with an explicit extension version. Necessary native capabilities must match before launching Claude: true controlling PTY, separated native byte stream, confirmed delivery identity, native opaque decisions, safe raw input/resize, native identity/replacement and group stop. An ordinary ACP agent or client without these does not silently become a Claude SDK/print substitute. Standard ACP extension rules use `_meta` and underscore-prefixed methods; no custom root fields on standard objects. [ACP extensibility](https://agentclientprotocol.com/protocol/v1/extensibility).

Native extensions provide state/snapshot/raw/resize/decision answer/stop and bounded event receipts. PTY bytes travel on a distinct framed inherited transport, never ACP stdout JSON. Slow terminal viewers cannot block native output drainage or ACP cancel/stop; overflow requests a coherent snapshot/resync rather than treating dropped output as conversation history. Epoch and monotonic sequence identify all streams. Normal JSON tool/history messages never masquerade as terminal bytes.

Native TUI menu options and fingerprint/selected-before facts remain opaque real choices. Do not invent `allow_always` for an unrecognized menu. A real standard ACP permission request retains original request id/options; native menus use the negotiated native decision extension and the existing safe-key engine. Unknown, cancelled, superseded or old-generation responses do not answer the current menu.

## Single registry owner and birth barrier

The existing ProcessRegistry is a process-local cached map plus restrictive enrollment history; two processes must not write its persistent file. The core remains the only persistent registry writer. It owns one logical session admission, registers the helper's actual kernel identity, and enrolls each actual native identity with session/generation/nonce and actual parent/process-group/birth checks.

A controlled pre-exec native birth gate is required to close the helper-crash-before-native-registration window. It uses a private inherited descriptor: after setsid/TIOCSCTTY the child reports its actual PID, waits for the daemon's durable enrollment acknowledgement, then execs native Claude. EOF/refusal/timeout exits before exec. Core checks actual ancestry, PID/PGID/start/boot identity and current launch generation; a claimed PID alone is not authority. Parent owns adapter/native orphan obligations and restrictive enrollment barriers. Successful normal stop waits for raw OS terminal proof; stopping adapter alone or receiving ACP cancel is not process-group termination. Unreadable identity and failure to settle remain visible and restrictive.

The gate is optional and scoped to new adapter launches; existing native/Codex launch behavior stays unchanged. No provider-specific work book or independent SQLite writer. Tests must include parent/helper death before and after birth acknowledgement, PID/nonce mismatch, actual descendants, failed persistence, and no ordinary default-Human escape during enrollment.

## Turn, delivery and recovery semantics

A new/load operation records actual native identity in the existing daemon ledger before prompt submission. Resume refusal preserves the existing truthful old/refused/new ordering; replacement native identity is explicitly signaled through the negotiated extension, never hidden behind a stale ACP id.

Input accepted/queued is not Delivered. Delivery receipts bind daemon output ownership to helper generation, native identity and native driver's confirmed final delivery. Standard `session/prompt` completion requires the actual trusted Stop-hook turn signal, not a short output quiet period or personal transcript polling. Working/idle is a separate fact. Sidecar hooks must preserve existing PostToolUse/Stop/SessionStart crewd effects without double injection or blocking Stop.

Cancel interrupts the current turn; stop ends the actual native process group and helper. No shared ACP id can cancel another session or a new generation. Disconnect never resends an accepted/possibly submitted prompt on its own. Duplicate request/receipt and replayed history are idempotent projections, not new native input. Uncertain submission stays explicit and is not silently relabeled undelivered-safe-to-retry. The bounded launch-local producer/request journal is an execution receipt, not a second team ledger; durable core wake/cursor authority stays in crewd.

## Vertical implementation order and acceptance

1. Freeze protocol/capability/epoch/request and dual-stream codecs; adversarial fake peers validate limits, malformed/unknown methods, reordered/duplicate/late events, unsupported required capability and zero prompt before admission.
2. Native birth gate and single-owner registry, actual process identity and teardown. Fake program proves isatty(0/1/2), controlling tty, interactive argv/no prompt argv, descendants and pre-exec crash gates.
3. ACP helper reuses native driver for new/load/prompt/raw/resize/menus/cancel/stop; actual PTY outputs include split UTF-8, ANSI/query replies, geometry and backpressure; ACP/stdout stays independently parseable.
4. Trusted Stop-hook lifecycle and confirmed delivery/recovery/producer semantics; direct public crewd controls, real SQLite authority and mixed Codex-native/Claude-ACP routes prove no cross-session delivery or new team ledger.
5. Exact frozen source, unique existing target, relevant current workspace/strict fmt+clippy and independent review; keep real-model gates separate. Actual Claude/account/real mixed-provider behavior remains a later explicit authorized acceptance layer.

M3 necessary compiler windows have priority. No second cold workspace target, whole-toolchain upgrade, global install, personal transcript/auth access, paid model probes, main merge, push, daily-data switch, Jev or memory implementation.
