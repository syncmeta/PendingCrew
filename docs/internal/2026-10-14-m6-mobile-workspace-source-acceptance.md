# M6 shared mobile workspace source checkpoint

This checkpoint adds the iPhone/iPad four-view workspace using the real shared Crewd data stores and commands. It is a source/type-check checkpoint, not remote pairing, device acceptance, installation, or a mobile release. The former daily app/PSK path is not connected to this workspace.

## Product scope

- Contacts retain the existing multi-parent crew hierarchy; the messages list uses actual cached latest messages and explicitly labels unread sources. iPad uses a split layout; narrow phones navigate from the mixed group/DM list into the selected conversation.
- Messages use stable member/group emoji seeds and the existing adaptive chat palette. The actual group Todo and members panels are independently collapsible. Hidden group work does not acquire an active read owner. Conversation destination gates keep the previous channel from appearing or receiving input under a newly selected destination.
- Todo and progress reuse one WorkStore and Plan tree. Requirement creation/detail/ruling, Plan creation/assignment/report/block/split, Human answer/withdraw, lost reassign and readable handoff/cancel remain the same typed operations; no mobile copy of Plan state is created.
- Documents and all four independent view-folder controls reuse their capability-gated authority APIs. Phone forms scroll, the Plan action picker uses a menu, and document list/detail adapts to available width. A late document read is gated by selection token and crew.
- Actual runner requests, restart decisions and manual session interrupt/stop/resume are available. Starting a Codex member requires an explicit user action and preserves the task draft. An uncertain start retains the original full input; another start requires a fresh member-list read and human acknowledgement. No runner starts on appearance.
- Crew creation uses actual title/workingDirectory/parentId fields. The directory belongs to the connected host. The API has no idempotency request key: unknown outcomes preserve the immutable original input, require a fresh roster read and explicit acknowledgement, and never retry automatically. Successful older submissions only clear the matching draft version.
- Workspace/authority owners keep separate inputs and caches. `authorityRecordID` is only an immutable local namespace; it does not authenticate a remote peer. The client permanently refuses a retired connector before any new handshake (independent RED/GREEN proof lives in the existing shared-core acceptance record).

## Actual local checks

All current `Sources/Mac/Crewd/*.swift` and `Sources/Mac/Design/CrewColorHash.swift` were type-checked together with `xcrun swiftc -parse-as-library -typecheck` on macOS, and with the iOS simulator target `arm64-apple-ios17.0-simulator` plus the installed iPhoneSimulator27.0 SDK. Both final invocations completed without diagnostics; the enclosing command exited 0.

- iOS final log: `/private/tmp/pcw-M6-mobile-workspace-ios-typecheck-07.log`
- Mac final log: `/private/tmp/pcw-M6-mobile-workspace-mac-typecheck-06.log`
- Cached XcodeGen 2.46 project registration log: `/private/tmp/pcw-M6-mobile-project-generator-01.log`. Generated pbxproj changes are only the four source registration entries for CrewdMobileWorkspaceView.swift; project.yml and generator version are unchanged.

Previous compile diagnostics and intermediate logs were preserved. No Cargo, full Xcode dependency rebuild, visible app, account, paid model, daily database, simulator runtime, physical device, installation, or release was executed for this source checkpoint. Independent mobile flow and rendering acceptance is still pending with the quality owner.

## Remaining dependencies

The root application must create one workspace per window and paired authority. Production remote transport requires the already-decided TLS 1.3 mutual SPKI pinning, explicit pairing, durable revocation and a server-authenticated remote actor; the legacy local Unix Human trust cannot be applied to an arbitrary network peer. The backend listener and public pairing/identity DTOs are not yet frozen. Therefore no temporary open port, old PSK, fabricated pairing screen or raw network endpoint is enabled here.

After those contracts exist: implement paired transport and record persistence, wire the mobile root and iPad window ownership, run shared form/transport runtime controls, then perform actual iPhone and iPad connection/revocation/reconnect/work-flow acceptance. Current type-check success must not be reported as those later gates passing.
