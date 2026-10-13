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

The new real slow-consumer trace exposed a second necessary condition: the first collapsed paste redraw can describe only a prefix of the queued body. Its public line count must match the complete sanitized body before starting the submission beats. Space and Backspace now each require the corresponding native-painted input-cursor change, rather than an elapsed gap or arbitrary output. The witness is scoped to the actual prompt and wrapped input region (or owned body tail); VT cursor visibility/movement and unchanged redraws do not qualify. Exact multiline collapsed-count expansion/restoration is accepted, with the same paste identity. A missing consumption witness expires within the existing verification budget, without resending the body or submit beats. Only actual completed final Enter plus fresh post-completion output can confirm delivery. Terminal reports revoke all remaining submission keys; late completion/echo cannot revive them.

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
