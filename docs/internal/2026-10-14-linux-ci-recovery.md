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
