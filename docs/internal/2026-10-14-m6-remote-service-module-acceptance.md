# M6 remote backend: schema11 delivery checkpoint

The complete remote backend now has a recorded normal helper, actual public pairing/TLS/Core/revocation and restore acceptance, and an independently run production schema10→11 upgrade/rollback chain. Main integration remains subject to the complete platform CI gates. Native runtime, candidate SDK configuration, physical devices and installation are separate layers.

## Frozen production and actual normal helper

- Production reference: `28ccf9c75c99ccb9c2e48d3de6c6e8d9ba93faca`.
- Immutable evidence: `/private/tmp/pcw-M6-normal-backend-20261014-28ccf9c7`; sibling `-witness.json` contains all **360** committed Rust input files, actual Cargo stdout/artifact configuration and preserved pre-sign binary.
- Rust digest: `ad17c77a76c0e55efb7b2f7193155ffbcfb14410182b2e86667abbe1efa29289`.
- Binary SHA256: `4b2e9f183bd29bdda9e76e3a3c9446fc58cba0a81b56643ebd4a5caf7ee2084d`.
- Actual configuration: Rust/Cargo 1.94.0, aarch64-apple-darwin, debug, default features enabled; no explicit/all features; every effective workspace feature set empty; compiler artifact `test=false`.
- Root independently compared every witness source SHA against the complete Git tree and verified binary/log/config: `/private/tmp/pcw-M6-normal-backend-28ccf9c7-root-proof.json`.

Default remote service remains disabled. Unpaired `remote.status`/`remote.grants` are read-only and do not generate an authority or private key. TLS1.3 mutual full-SPKI pinning binds a real authenticated principal to the current Core. Local admin and remote owner-workspace operations use distinct admission; per-call and per-frame grant checks deny revoked or stale connections. No PSK/local-Human fallback is used.

## Actual public service boundaries

Independent QA preserved source, original logs, temporary data and negative controls:

- Public configure/invite/signed response/accept, original UUID confirmation, actual TLS/Core writes and events, wrong challenge/replay rejection, caller-declared capability rejection, and online revoke: `pcw-M6-service-workflow-01.log`.
- Actual backup of an active grant → revoke → same-key/same-dbId restore → disabled restart → old accept UUID returns the current tombstone → explicit listening rebind still rejects the old certificate: `pcw-M6-service-restore-01.log`.
- Attached local agent is rejected by all six remote-admin methods and leaves SQL/events/wakes/witnesses unchanged: `pcw-M6-service-attached-admin-01.log`.
- Explicit synthetic provider only: actual remote session start/approval/user response/stop/inspect/resume/stop: `pcw-M6-remote-fake-flow-04.log`. This does not use a paid model/account.
- A TLS write accepted without the client observing its receipt remains durable after revoke, with one request and the old UUID returning a tombstone: `pcw-M6-admitted-revoke-03.log`.
- Real kernel-saturated one-million-byte output: old `2d0c445e` writer completed the frame after revoke (RED); `a0844703` cancels the same partial frame, denies the queued tail mutation and closes within the same budget (GREEN): `pcw-M6-slowwriter-{red,green}-01.log`. Already-sent bytes are not retractable. A cancelled partial frame is never replayed.
- The default read-only controls and an unrevoked blocked connection also pass: the default 10-second global drain completes in about 5.33 seconds. QA checkpoint `1144fef4` records the exact evidence. The final lint grouping in `28ccf9c7` retains these lifecycle semantics.

All service tests use isolated data and temporary loopback endpoints. No real LAN listener, personal Keychain, account, installation or model task is used.

## Actual formal migration and old-version refusal

QA `d9f84043` personally ran the old normal schema10 helper and the above normal11 helper. `pcw-M6-normal-schema11-migration-04.log`, terminal exit0; raw root `/private/tmp/pcw-remote-migration.ZMLLlK`:

- Original #/W, edited document, view-folder association and immutable create UUID witness are produced through the real old daemon.
- Protective backup failure keeps original bytes/facts and version10.
- A second-DDL collision rolls back the already-attempted first-DDL; version10 and original facts remain.
- Only the disposable test copy has its synthetic barrier removed; actual upgrade succeeds to11, the automatic backup retains version10 and every original fact.
- Old normal10 refuses the11 database without changing its bytes.
- Reopen keeps edited document/classification/request witness; status/grants remain disabled and no authority is silently created.

Attempts01–03 were fixture/environment failures (copied inode/WAL ownership, sandbox binding, and context closing); their logs remain. They are not product migration failures.

## Complete checks and pending whole-platform gates

- Local strict `clippy --workspace --all-targets --all-features -- -D warnings` and `fmt --check` pass: `pcw-M6-remote-final-clippy-03.log` and `pcw-M6-remote-final-fmt-02.log`.
- The local full all-features test attempt stops during linking with **errno28**, before tests. `pcw-M6-remote-final-workspace-01.log` is preserved; this is not reported as a passed full test gate.
- Actual default-feature platform CI on `28ccf9c7` executes 2557 tests, with 2555 passing and two stale coverage/version assertions failing. `c42828e3` repairs only those tests: six real CLI handlers (setup explicitly disabled, CAS revision read from actual status) and an exact backup prefix derived from current `SCHEMA_VERSION`. QA independently exercises the six handlers against the original normal28 helper: `pcw-M6-ci-cli-six-independent-01.log` GREEN. That does not relabel the old binary as built from the test-only reference.
- The subsequent complete CI and extra all-features execution coverage must retain their own exact inputs/outcomes. No failed test is skipped and no warning level is reduced.

The accepted schema11 source unblocks the separately owned schema12 native-execution receipt migration. Its restore invalidation must be composed transactionally; staged12 tests alone do not prove a formal upgrade.
