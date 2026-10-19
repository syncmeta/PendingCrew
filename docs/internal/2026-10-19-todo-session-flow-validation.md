# Todo / session flow validation — 2026-10-19

Changes are local and uncommitted, based on `25e8317401979dc95ae935a00894226d4c3d79ab`. During initial validation no push, release, installation replacement, daily Todo mutation or real-model execution was performed. The subsequent user-authorized installation is recorded below. Existing product-tour edits were excluded.

## Reproduced and fixed

- The narrow center pane hid group channels behind a conversation popover. It now shows direct group channel buttons, a clearly named group/session picker, and a return-to-group action while viewing a session.
- A stopped crewd projection was treated as a new-session composer. It now retains its original session identity; explicit avatar/conversation clicks read actual live state and resume only the original stopped session. Running sessions do not relaunch; concurrent clicks coalesce; selection changes during the read cancel that restoration.
- Confirmed crew creation did not start a captain on either the atomic first-message path or the compatibility creation path. Explicit creation now starts one captain, checks the existing authoritative roster first, and preserves the created crew if startup fails. Appearance/reconnect does not invoke this startup.
- Normal public session controls return an asynchronous dispatched receipt before the physical result. The original UI treated that intermediate result as a lasting startup/stop failure. It now follows only the original request UUID by read-only lookup for up to five seconds. Historical, unknown, missing or timed-out outcomes remain unresolved and are never replayed. Terminal observations append journal reconciliation without rewriting original outcomes.
- Restoration could reconnect the original thread yet leave a message queued if it arrived during startup. An idle resumed thread has no working-to-idle edge. The backend now checks queued work when the actual handle is ready, preserving all normal stop, inflight and admission gates.
- Session controls could reuse a cached target from before restoration. Explicit original-session controls now use the workspace's fresh CAS target resolution. A genuine subsequent conflict is still refused, with no retry. Transient backend failure text clears when a fresh running lifecycle is observed.

## Automated verification

- Full native macOS App build: succeeded. `/private/tmp/pcw-todo-flow-build-final.log`.
- 442 orchestra library tests: passed, including human-stop cancellation boundaries and inflight/late-result protections. `/private/tmp/pcw-orchestra-final.log`.
- 25 integration tests: passed across channel work (4), Codex runtime (3), requirements (5), lost owners (5), required owners (2), transfers (6). `/private/tmp/pcw-todo-lifecycle-final.log`.
- 7 tests importing the actual final compiled App module: passed (stopped/running/malformed live state, selection switch, double click, captain creation, captain refusal). `/private/tmp/pcw-original-flow-final.log`; per-scenario wire captures and actual module hash are in its printed evidence directory.
- `cargo fmt --check` and `git diff --check`: passed.

The existing hello capability test compared an unordered capability set as a serialized list. Its test-only assertion now compares sorted names, preserving membership and duplicate checks. The original Codex runtime test was extended with explicit restoration and bounded failure diagnostics. Its delivery-after-resume failure was reproduced before the backend fix and passed afterward.

## Native GUI checks

All writes used disposable local databases and an explicitly configured fake Codex executable. The original daily App was inspected and its group message was visibly present under the existing conversation picker; no daily requirement was created or completed.

- New crew + first group message: captain appeared automatically.
- Final asynchronous launch: captain running, no false unknown-result warning.
- Group composer checklist: created requirement #1; exact original text and the creation receipt were visible.
- The test captain used its actual generated agent credentials and formal work tools to create W2 referencing #1. The GUI showed the requirement as in progress.
- Completing W2 through the formal agent tool changed the requirement, plan and right-hand summary to done, without manual refresh or changing original text.
- Safe pause/reopen retained the crew and session.
- Clicking the exited captain avatar restored the original session; the runtime integration test independently asserted the original thread ID was reused.
- A message sent after restoration reached the actual fake app-server turn and was visible in the session transcript.
- Final restore then stop: stop succeeded without stale-target or dispatched-result warnings.
- Narrow-pane group shortcuts and return-to-group were visible throughout.

## Limits and delivery

This validates the original native UI, formal local RPC, actual daemon/process lifecycle and synthetic executor integration. It does not validate real Codex account availability, quota, network or arbitrary model task performance. The final candidate is `/private/tmp/pcw-todo-flow-build/Build/Products/NativeCandidate/PendingCrew.app`; it uses test data and is not an installed release. The existing “end result unconfirmed” inactive badge reflects the projection's unavailable exit code, and is not evidence that restoration or Todo completion failed.

## Authorized installation follow-up — 2026-10-19

The user explicitly requested replacement/installation after reviewing validation. Built the same fix sources as the ordinary Release App with `PENDINGCREW_LOCAL_MAC`, normal bundle identifier `com.pendingname.pendingcrew`, version 0.1.44, build 20746.1. This is a local build with uncommitted fixes; it is not a published or notarized release. The source base remains 25e8317401979dc95ae935a00894226d4c3d79ab; the package marks its dirty source and embeds the tested source-file manifest.

- Build: `/private/tmp/pcw-todo-production-build.log`, BUILD SUCCEEDED.
- Prepared package: ad-hoc hardened runtime signing, matching the previous installation's signing tier; deep/strict signature validation passed.
- The old App was quit using its actual Safe Pause action, and both old installed App/backend processes had exited before replacement.
- Installed at `/Applications/PendingCrew.app`; staged/installed Info.plist, GUI and helper hashes match.
- Prior installation backup: `/Users/hey/Library/Application Support/PendingCrewInstallBackups/2026-10-19-todo-session-flow-215013/PendingCrew.app`. No database directories were moved or replaced.
- Actual installed App launched through the native UI. It connected to its own new helper child, both executing from the installation directory.
- The original Qincheng crew and existing group message were visibly retained. The new direct “大厅 / 与我沟通” shortcuts and “群聊与会话” control are visible. No daily Todo was created or completed and no model session was explicitly resumed during installation verification.
- Exact installation and runtime evidence: `/private/tmp/pcw-todo-install-20261019/installed.json`.
