# Codex completion: shared Mac/iOS consumer

Complete client source, not provider/runtime, Apple qualification, signing, installation or live network acceptance. Base 65976a073e49124a43808fa24630f54d88d6301d; internal WIP 6dfa3eb6 retained. This extends the existing shared session panel and first-task forms rather than replacing their controls. Rust/schema remain the backend author's ownership.

## Actual entry points and contract

Mac original CrewSessionWindowView → CrewdOriginalSessionControls → shared CrewdCodexControlsPanel; iPhone/iPad IPadShell → original member detail IOSRemoteSessionView → the same shared panel. The original Mac new-session form and shared CrewdSessionStartView use the same CrewdCodexStartOptions. Existing model/effort/fast/approval/plan/usage/title/queue/steer/stop controls remain; new permissions catalog, local inputs, enabled Skills and native history actions use real closed producer DTOs.

Producer interface accepted at catalog 5b3676a1 + projection 6d08a43b: CONTRACT SHA15f2daea130e83b5aad07ac2f432845c8b500fb49638f37c89dc64dcdf01c938, schema141 b5ade7f996cf4debd9b6ea1a5db34e7bd6836ec8c377ed35c2d83be516d469bf. Current canonical contract snapshot SHA09cba9420af23cb6ea3c13d7b061c4012731c87dbc60bf4ad0fb60607d5b8ebc includes later restore-unconfirmed/Store20 clarification; codex_inputs.rs remains6b0b3465dcf857355e308e450e8c2be373c55be0fa001572ba0ace2c5f26969a. The backend's whole source acceptance/version remains separate. No unaccepted Rust is merged here.

Input prepare/discard return bare operation results; inputLookup returns state known + result or notFound. Command operations retain PublicCommandReceipt/command.lookup, never the input wrapper. Tools/profile reads use exclusive original sessionId or crewId; the first-turn crew catalog is only a canonical cwd preview. Only actual allowed profiles/enabled Skills are offered; no freely entered path or profile guess. Final actual workspace checks remain authoritative, with rejection instead of silent substitution.

## Inputs, original text and durability

Task text stays verbatim. Start, queue and exact-turn steer have independent input selections. Prepare is staging, not task delivery or proof that a reference is unused. PDF/other binary inputs are explicitly unsupported. The original command journal records references before dispatch; any attempted original command/discard prevents client reuse. Unknown holds its original item and UUID, lookup notFound does not authorize a retry, and independent files/read-only pages remain usable.

Before any upload or await, the file is bounded-read and copied to the existing private original journal as an owned0600 immutable file. Failure/capacity refusal means not added/uploaded and the original user file remains untouched. Accepted originals survive normal reopen and are exportable, even offline, through the existing Mac connection menu / iOS menu → 本机Codex原输入. No bookmark or user file path is retained. The read-only archive cannot connect/upload/send/replay.

Selections (full source scope + original prepare observation IDs + exact Skill name/path) are local-only, split by crew/session/start|queue|steer. Synchronous durable revision CAS precedes adopting UI changes. Reopen restores selected and Unknown/used relationships without upload/send; old scope or stale revisions cannot overwrite another window/connection. Original raw files are hash/count verified on export. References, media bytes and native Skill paths are not added to personal Draft, Cloud or cross-device synchronization. Scope uses the existing local journal identity solely for this private source-bound record, not a cross-device content key.

Original queue/steer text uses the existing personal input owner, snapshot and clear CAS; only successful original submission clears matching raw text. Unknown leaves text and inputs intact. File bytes never exist as an accepted VM-only choice, so no new quit subsystem is needed. Original save/export/quit protection for raw text is unchanged.

## Native controls and restore

History reads do not run a session. Archive/unarchive require the actual capability and do not start. Delete explicitly confirms irreversible native-history deletion while retaining PCW work/messages/receipts/files. Revert selects a loaded original turn, requires observed paginated/live idle/no queue/no decision, and explains that files are not rolled back. All four successful result application values are metadata. Missing nativeHistory is unknown, not active. Producer restore-unconfirmed is displayed honestly; no new UUID or automatic reuse/reupload/first-task replay repairs it.

Every explicit native confirmation retains its inspected exact target. A fresh controlTarget is read before dispatch and checked against that confirmation; steer additionally retains exact generation/turn. Scope change during await cannot update another panel or clear its text. Current capability IDs govern controls; version/schema numbers do not grant them.

## Evidence by actual input

The actual old prepare seam at6dfa3eb6 compiled and failed originalBytes (-5): successful prepare did not preserve the local raw file. Current actual production Foundation slices pass19 input/catalog/durability modes and5 SessionStore target/late-scope/Unknown modes. Earlier missing permission/archive DTO red logs and the original fixture-helper correction are retained separately; fixture-construction failures are not product reds. Fixtures live in scripts/dev/fixtures/crewd-codex-inputs.swift and crewd-codex-fresh-target.swift, and run only synthetic RPC closures, with no SDK/provider/network call.

Final ordinary Mac Debug LOCAL_MAC+REMOTE_PRODUCT and ordinary arm64 iOS Simulator targets both exit0. Before/after all400 Swift/project/package-lock hashes are equal, in codex-completion-client-targets-round5-20261023. Historical5553 helper dc3f920bc0158f6a9c50c33516397ff7c8521b0ba00e8e9782a6b40271d78067 is only a compile input, not this module's Core/candidate. Original65 logs from rounds1–4 remain. Accepted APNs0800/CloudCDF/B/shell proofs are inherited, not rerun or relabeled; these final full targets newly cover all current source together.

Remaining: backend whole-module/Store20 source acceptance, root integration and exact default helper, final signed normal Mac/iOS candidates, real Keychain/network/provider/media/catalog/native-history qualification and actual GUI. No model, daily App, GUI, installation, notification permission, Apple service/account, signing or new cache was invoked for this source module.
