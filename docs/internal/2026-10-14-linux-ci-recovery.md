# Linux CI recovery from exact 67430035

Base: `6743003576cee5a3d92cf42b963289d239975039`. This isolated branch contains no M4 ACP implementation.

## Original remote evidence

- Linux job `115504649566`, `/private/tmp/pcw-M3-linux-ci-67430035-failure.log`: 2533 run, 2529 passed, four failed, eight skipped. The CI policy correctly treats flaky retries as failures. Original logs are preserved.
- macOS job `115504649726`, `/private/tmp/pcw-M3-macos-rust-ci-67430035.log`: same source passed. This does not prove Linux correctness.

## Changes

The partial-write/last-byte test now uses the actual raw/no-echo guarded child, observes the actual positive bounded syscall count and matching queue offset, proves cancelled suffix bytes never reach its real reader, and separately captures exact `ok` once for the full positive control. A 4096-byte syscall cap is not a promise that every kernel accepts 4096 bytes. Shell-backed write controls explicitly set raw/no-echo before their ready marker, so echoed product text cannot masquerade as foreign screen input.

Linux saturation now waits for the child's real `FIONREAD=4095` raw-input-full receipt while no stdin reader exists, then fills remaining upstream capacity until the last one-byte write actually returns EAGAIN. One earlier EAGAIN from asynchronous master flip-buffer transport alone does not establish that condition. No fixed setup delay, skip or relaxed write-revocation assertion was added. See the [Linux N_TTY buffer contract](https://kernel.org/doc/html/latest/driver-api/tty/n_tty.html). Linux-only receipt behavior still requires actual Linux CI execution.

Caller qualification distinguishes concurrent registry revision changes only when a complete anchored kernel lineage was freshly captured and rechecked stable. Such a refusal triggers fresh roots/ancestry qualification inside the same parsed request and original one-second monotonic deadline, before any business dispatch. Missing anchors, changed parents, unknown ancestry and known managed roots retain denial. Drain wins. A revalidation walk that itself finishes after the original deadline cannot grant Human. Business methods are never retried.

## Executed local evidence (macOS)

- `/private/tmp/pcw-linux-ci-peer-fix-02.log`: 20 tests passed; includes managed dominance, unqualified/changed parents, same-request revalidation, continuous revision churn expiry, drain and a synchronous walk crossing expiry.
- `/private/tmp/pcw-linux-ci-output-drain-mac-01.log`: preserved RED, 24 passed/one failure exposed raw short write of 1022 where old assertion assumed 4096.
- `/private/tmp/pcw-linux-ci-output-drain-mac-02.log`: full 25 guarded output/write tests passed after actual-progress and reader-capture correction.
- `/private/tmp/pcw-linux-ci-orchestra-restart-mac-01.log`: three actual synthetic daemon lifecycle tests passed, including hibernate result delivery/restart note once.
- `/private/tmp/pcw-linux-ci-clippy-01.log`: strict workspace/all-targets/all-features clippy passed.
- `/private/tmp/pcw-linux-ci-fmt-01.log`: format check passed.

No Linux GREEN, independent review, main merge, package, installation or provider runtime is claimed by these local results. Root owns isolated-branch publication and new actual Linux/macOS CI. No paid provider or personal data was used.

## Second actual Linux run and follow-up

Root published exact `f80e6e52`; run `38460564434`, Linux job `115514341225` remained RED: 2535 run/2532 passed/three failed/eight skipped. `/private/tmp/pcw-M3-linux-ci-f80e6e52-failure.log` is preserved. This is new evidence, not a successful retry of the initial gate.

The new real reader capture found one legitimate user `z` after the closing marker. Capture now enqueues already pending raw/reply actions before its marker; the partial-cancellation case requires exact `z`, proving user input survives while cancelled bot suffix is absent. Linux rate fixture logged one 4095-byte actual read despite 14336 accepted filler bytes. Its reader now uses the same raw FD that `poll` observes, eliminating buffered `Stdin` state invisible to readiness checks. Linux behavior of that correction remains to be proven by the next remote gate; no filler count or zero-trailing assertion is relaxed.

The permanent-enrollment actual socket test expected the original `spawn_enrollment_in_progress` denial, but the added expiry boundary replaced that fact with a generic expiry message. The original transient refusal is now retained across the fixed deadline. Late Human still cannot be granted; drain still wins, and no business retry is added.

Local follow-up:

- `/private/tmp/pcw-linux-ci-output-drain-mac-03.log`: 25 passed, real guarded reader captures retained.
- `/private/tmp/pcw-linux-ci-peer-fix-03.log`: 20 passed.
- `/private/tmp/pcw-linux-ci-enrollment-physical-mac-01.log`: two actual socket enrollment/drain tests passed, including original request ID, bounded refusal, unchanged database/hub effects and fresh external Human after guard retirement.
- `/private/tmp/pcw-linux-ci-clippy-02.log` and `pcw-linux-ci-fmt-02.log`: strict workspace clippy and format passed.

These are macOS evidence only. Root still owns the next exact-source remote two-platform gate and subsequent main decision.

## Native submission consumption and the remaining macOS CI failure

Exact `1b0c85fe` remote Linux job `115520579160` passed all 2535 tests (eight skipped; one leaky classification), while macOS job `115520578965` failed the delayed-chunk 68000-byte brief. Original evidence remains in `/private/tmp/pcw-M3-linux-ci-1b0c85fe-green.log` and `/private/tmp/pcw-M3-macos-ci-1b0c85fe-failure.log`. Retry success is not counted as repairing that gate.

The new real slow-consumer trace exposed a second necessary condition: the first collapsed paste redraw can describe only a prefix of the queued body. Matching its public line count is only necessary readiness evidence: a same-count partial last line can still be in flight. It does not attest the complete body. Space and Backspace now each require the corresponding native-painted input-cursor change, rather than an elapsed gap or arbitrary output. The witness is scoped to the actual prompt and wrapped input region (or owned body tail); VT cursor visibility/movement and unchanged redraws do not qualify. Collapsed same-count cursor/space changes are refused. In the tested native codec, a newline-terminated body has at most N lines in any prefix: only its subsequent Space consumption can generate N+1, with the same paste identity, then BS must restore N. Opaque single lines and non-newline-terminated collapsed bodies have no such witness and remain explicitly unconfirmed; complete directly rendered input instead binds the whole sanitized body. A missing consumption witness expires within the existing verification budget, without resending the body or submit beats. Only actual completed final Enter plus fresh post-completion output can confirm delivery. Terminal reports revoke all remaining submission keys; late completion/echo cannot revive them.

These tests are synthetic native TUI evidence, not a paid-provider compatibility claim. Fake peers now paint the native inverse cursor explicitly. The original business assertions for drafts, placeholders, long-body tails, unanswered dialogs, raw collisions, stale empty frames, pending final bytes, and actor receipts remain intact. Their positive fixtures explicitly draw each consumed key; completion helpers do not invent output. The two original unstyled/unknown-input negative fixtures remain unstyled and still deny writes.

Evidence retained:

- `pcw-ci-submit-echo-physical-red-01.log`: real held-reader PTY demonstrates elapsed-gap-only Backspace was issued too early.
- `pcw-ci-submission-independent-red-01.log` and immutable same-name source directory: independent cursor-noise false acknowledgement and missing bounded terminal failure.
- `pcw-ci-huge-native-echo-01.log`: preserved slow-consumer RED and actual read/consume boundaries; `pcw-ci-huge-native-echo-02.log`: complete-body consumption gate then 68000-byte exact-once GREEN.
- `pcw-ci-submit-echo-lib-01.log`: 22 old positive fixtures lacked consumed-key frames; `pcw-ci-submit-echo-lib-03.log`: 294 passed, two unstyled preconditions identified and retained explicitly.
- `pcw-ci-submit-echo-exec-full-02.log`: complete exec module passed: 371 tests, three ignored, including all 19 real synthetic CLI workflows and the four independent Core cases. A prior full-01 RED retains the three older synthetic peer cursor omissions.
- `pcw-ci-submission-independent-green-02.log`: independent four-case Core proof includes cursor-only/unchanged output denial, no-echo bounded terminal outcome, completed-Enter/fresh-frame receipt, and native input wrapping at columns 77/97.
- `pcw-ci-submit-native-noecho-physical-01.log`: actual held-reader capture is exact `BOT ` after expiry and late echo, with no BS/CR/body retry/cleanup bytes. Injected Core clock is explicit; actual byte writes and reader capture are real.

Final workspace and strict lint gates and the next exact-source two-platform CI remain separate acceptance layers. No M4 source, main, daily data, model launch or package installation is included in this repair.

The first final-workspace attempt (`pcw-ci-native-submit-workspace-01.log`) exposed two older CLI prompt-file/stdin glue fixtures relying on canonical shell line editing and kernel echo, with no native consumed-key cursor. The exact synthetic reader was identified and terminated to end the stalled test; no real provider session was touched. The fixture now uses actual raw/no-echo Python input, draws the inverse cursor after each consumed key, and records actual input bytes and argv separately. Its five-second lifetime only bounds fixture failure; it is not a provider budget. All original assertions remain, including complete file/stdin body via PTY and absence from argv. `pcw-ci-submit-cli-glue-02.log`: all five CLI glue cases passed. Production input is unchanged by this fixture correction. The new workspace gate must include this correction.

The same-count partial-content counterexample is independently preserved in `pcw-ci-collapsed-samecount-independent-red-01.log` and its immutable source/hash directory. `pcw-ci-submission-samecount-green-01.log`: all five independent Core cases passed after strict whole-body binding for plain input and refusal of opaque same-count acknowledgements. `pcw-ci-native-prefix-witness-01.log` records actual native consumer states: 68 original-body consumption steps never exceed 2000 lines; the real Space yields 68001 bytes/2001 lines once; actual BS restores 68000/2000 before CR submits. This proves the tested synthetic codec, not every Claude version's display-block semantics.

`pcw-ci-native-submit-workspace-02.log` retained four S42 actual-Task fixture failures (712 server unit cases passed), whose controlled peers supplied an opaque paste caption and no individual key-consumption frame. Their fixture now supplies the complete short multiline body and each actual Space/BS cursor frame. Actual reader capture still gates every next phase; the real observer, database CAS, pending-first-ack and independent hook-basis assertions remain unchanged. `pcw-ci-submit-s42-02.log`: all four actual Task/DB cases passed. No producer success is replaced by a synthetic delivery receipt.

## Production capability boundary

`CoreConfig::verified_collapsed_paste_codec` is a trusted executor configuration capability, defaults to false, and has no environment, CLI, RPC, Serde or native-screen enabling path. Only the declared synthetic peers in these tests enable their verified codec. Unknown production collapsed input is explicitly Unsubmitted before any submit key is emitted. It holds mixed/unknown input rather than silently replacing it with another queued job; only explicit human raw clear plus fresh Empty can release that hold. Native PTY remains available. This is a temporary compatibility boundary, not completion of M4 or a claim that real Claude opaque content is fully supported. M4 requires actual authenticated UserPromptSubmit/content witnesses for the complete automatic opaque-content path; Codex native app-server is unchanged.

`pcw-ci-native-codec-lib-01.log`: all 297 exec unit/real PTY cases passed. `pcw-ci-native-codec-independent-01.log`: six independent public-Core cases passed, including default-false capability, native画面 cannot self-grant the codec, stale empty cannot replace the unknown body with NEXT, and explicit human clear restores exactly one NEXT. The trusted-codec same-count negative remains enabled, so default-false cannot conceal the original counterexample.

The normal-default boundary exposed six C13 literal/library serve workflows whose synthetic peer had rendered only an opaque caption (`pcw-ci-native-submit-workspace-03.log`). Their peer now explicitly renders the visible complete input via a **fake-binary-only** display option; it does not enable any production codec capability. `pcw-ci-native-c13-01.log` used the older separately built fake artifact and is retained as an invalid updated-fixture run; `pcw-ci-native-fake-build-01.log` builds the changed actual peer. Subsequent C13 controls exposed hard/soft line rendering and the Backspace baseline. One attempted replacement did not match the formatted source (04); the asserted edit actually landed for 05. `pcw-ci-native-c13-05.log`: all six actual service workflows passed, with original complete-body, routing, callback, restart and exact-once assertions intact.

The term witness now distinguishes actual soft-wrap flags from hard line breaks and preserves the current painted cursor's full visible prefix. `pcw-ci-native-exact-input-01.log` verifies trailing newline, 80-column boundary and wide UTF8 positive controls. **Visible content is not a raw-byte witness**: hard-line grid padding is indistinguishable from meaningful trailing spaces. `pcw-ci-native-visible-padding-01.log` explicitly demonstrates both `a\nb` and `a \nb` have the same normalized visible text. Actual writer-byte and synthetic consumer/got-file assertions remain separate evidence; real provider content fidelity still requires the M4 authenticated content witness, and no universal raw-content assertion is made from terminal cells.


The next workspace gate (`pcw-ci-native-submit-workspace-04.log`) exposed old opaque rendering in the restart fixtures. `pcw-ci-native-orchestra-restart-02.log` passed kill/restart and SIGTERM but retained the hibernate interrupted-note failure. `pcw-ci-native-hibernate-inspect-01/02/03.log` retain the actual long native input and synthetic-only diagnostic: the full prompt exceeds the legacy 15-row search, a CJK glyph at a wrap boundary inserts a leading wide-character spacer, and two meaningful blank-line spaces alias default grid padding. No budget, retry or original note assertion is relaxed.

The pending narrow correction admits a unique actual prompt immediately below its visible top border, within the current screen, and excludes both physical wide-character spacer kinds from visible content. The declared synthetic full-input peer paints meaningful hard-line trailing blanks with real underline attributes; only such painted blanks can be distinguished from default padding. Expected body bytes are not normalized to grant acceptance. The original unpainted padding-alias negative remains. Long visible-region, mixed ASCII/CJK wrap and painted-blank ANSI controls are prepared; these changes have not yet completed the final workspace/lint/format gate. M6 temporarily owns the unique shared target for its independent directed build while these changes remain static.


`pcw-ci-native-long-input-term-01.log`: all 15 input-box controls passed, including CJK leading-wide spacer at a column boundary, nineteen visible input rows, explicitly underlined hard-line blanks, and the retained unpainted padding-alias negative. `pcw-ci-native-painted-fake-build-01.log` rebuilt the actual changed synthetic native executable. `pcw-ci-native-orchestra-restart-03.log`: all three real synthetic service restart/hibernate/SIGTERM workflows passed; complete interrupted-note content and exact-once delivery assertions remain intact. The temporary synthetic-only diagnostic was removed. Final workspace/clippy/format and new remote two-platform CI are still required.


Final local integration gate on fixed Rust source `278ec74b` (QA-only documentation `027b8084`): `pcw-ci-native-submit-workspace-05.log` completed exit 0, 78 suites / 2547 passed / zero failed / eight ignored, with `--workspace --all-targets --all-features --locked --offline`. `pcw-ci-native-submit-clippy-02.log` completed exit 0 with strict `-D warnings`; `pcw-ci-native-submit-fmt-02.log` completed exit 0. No production source changed between these gates. These are macOS local results; root must publish the precise final checkpoint and obtain new real Linux/macOS CI results. Historical failures remain intact. M4 and M6 are independent in-progress modules, not included in this completion claim.
