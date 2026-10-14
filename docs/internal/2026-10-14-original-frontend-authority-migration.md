# Original frontend → crewd authority migration (in progress)

2026-10-14 human correction: retain the actual original PendingCrew frontend and integrate the new backend behind it. The independent Native workspace remains a connection/business migration fixture, not final product delivery.

## Retained product components

- `PendingCrewApp`, its scene, `RootView`, `MacThreePaneView`, `CrewSidebarView` and existing sidebar modes/rows/navigation.
- `CrewCenterView` → `CrewChatView`: original bubbles, `CrewChatAdapter` sender mapping, reply/mentions, paging/scrolling/search/navigation and per-crew draft handling.
- `CrewSessionWindowView`, `CrewSessionRun`, `AgentTerminalView`/SwiftTerm mirror and `CodexTranscriptView`. These become backend-driven presentation; they cannot own a second runner.
- `IPadShell`, `CrewListView`, original `IOSRemotePairingView` Form and `IOSRemoteSessionView` List/navigation also remain the iPhone/iPad surface. The signed invitation → signed response → final confirmation adds the required third step; no old PSK/restart pairing is used. The same selected trusted crewd authority supplies original chat and the four work views.
- Existing settings, menu bar and appearance. Backend-dependent actions must move to the selected authority rather than legacy whiteboard files.
- Existing new work stores/actions/folder controls are reused in the original center pane's messages/Todo/documents/progress views; original chat stays mounted while changing views.

## Authority cut

`AppModel` owns one selected `CrewdRemoteWindow` authority. `CrewdPendingCrewBackend` projects existing frontend models from actual `crew.get/list`, session and message RPCs. The adapter is not a ledger or executor. A read-only workspace query and authority event publisher avoid re-query/sync loops.

Candidate entry now enters the original app, before the legacy GUI orchestration gate. Legacy frontend display caches use an independent `frontend-presentation` namespace beneath the isolated candidate root; the original daily legacy path remains separately recorded and forbidden as the crewd root. `SessionHost.begin` is presentation-only in this mode: no old local takeover, mention delivery or runner startup. `CrewStore` watches/direct mutations must be authority-gated; no new work is written to old whiteboard/outbox stores.

## Exact compatibility boundaries

- Management crew/captain/session/parent IDs are preserved from RPC responses, never replaced by old builtin identifiers. Parent edges map to `crew.adopt/release`; old financial shares are not fabricated.
- Public message fields include `inReplyTo` and typed mentions. The current authoritative contract rejects every nonempty message attachment (M3a decision 14), for local and remote clients. Preserve historical metadata and original controls, but refuse before old attachment persistence and never open a remote path as a local file.
- Original first-message creation cannot use the old LocalBackend rollback/outbox assumption. It must retain the original authority and submission snapshot, preserve unknown results and read back before another explicit action.
- Current public terminal contract is `session.inspect` screen + runner items, and formal Start/Stop/Resume/Interrupt/answers. Snapshot display is a migration intermediate, not terminal completion. Backend owns public terminal inspect/watch/custody/input/resize/release lifecycle, bounded lag, disconnect/revocation and unknown input recovery.
- Closing a window preserves the running private team. Explicit local Quit may hibernate only this App's own still-associated Process/continuous IPC and must await actual Process exit. A dropped/retired IPC lacks authoritative kernel peer/child-birth evidence; Quit remains unconfirmed instead of killing any historical helper. Public JSON PID/path statements alone cannot establish ownership. Remote Quit closes this client's connection/observation/lease only; it does not stop the remote daemon or team. Mobile background/exit cannot claim remote runtime termination. Old `DaemonStopper` paths are never used for this authority.
- Private 0700/0600 append-only frontend observations preserve prepared payload + original authority/DB/grant scope across restart, with fsync and immutable outcomes. This is a presentation journal, not a server success ledger. Unknown snapshots are read-only and never replayed automatically; general crew/message/session methods currently have no public requestId/status. A UI UUID and same-text match do not establish admission. Schema14 public command receipts/atomic crew+first-message remain required for final reliable recovery. Specific document/folder/remote idempotent request contracts are retained.
- Selected authority changes retire old writers/caches, preserve drafts/unknown receipts for readback and cannot replay into another crew or computer. Nested workspace/client publication and a single connection task must drive original toolbar/admin binding directly.

## Delivery and evidence

No new final frontend package has been frozen or published for this migration. Original independent Native fd195/5c packages and their window/source evidence remain unchanged. Actual complete original-source typechecks/builds and full original-window flows are required after the complete migration is frozen. Stable backend/TLS/work migration acceptance is reused.

Shared Cargo source-switch evidence is also being strengthened: full frozen Rust inventory, owning-source mtime invalidation immediately before build, normal/build production dependency graph, required own artifacts with exact package/manifest/source and `fresh=false`, no missing targets, raw Cargo JSON revalidation, hashed sidecars copied to packages. Old proofs retain `legacy-byte-inventory-only`; they cannot be relabeled as the new fresh production graph tier.

## In-progress source checks (not final delivery)

Actual private-journal filesystem controls at `/private/tmp/pcw-journal-controls-result-02.log` passed stable namespace, reopening prepared unknown, immutable outcomes, refusal separation, 0600 data and unsafe directory/symlink rejection. Original Mac/iPhoneOS source typechecks use the retained actual SDK/SPM flags and caches. These establish source compatibility only; no final original-frontend package/window/device installation is claimed. Previous compiler failures and input-invocation logs are retained.

Original sidebar previews and human-Todo descendant aggregation now read actual crewd messages/H identities; the original cache algorithm is reused without reading old whiteboard files. Global search retains its original result/navigation components and matcher, reading the current authority's message replies; the present API bounds are shown (up to 1000 returned rows per crew), never advertised as complete unlimited history. Menu unresolved source requirements leave the count only on formal closure, while deleted entries and closed H asks are omitted. Original Stop/Interrupt/Resume/captain selection route through formal RPCs; old runner remove/profile/admission actions cannot fall back to old local writes. Historical attachment metadata renders as an explicit opaque crewd reference; remote paths are neither images loaded locally nor file URLs opened on this device.
