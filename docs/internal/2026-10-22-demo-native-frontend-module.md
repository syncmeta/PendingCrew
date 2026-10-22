# Native frontend following the latest product demo — complete-module WIP

Writer: frontend_demo. Backend/schema owner: backend_continue. Root owns integration and the two program ledgers. This is a preserved implementation checkpoint, not a frozen or delivered candidate.

## Exact baseline

- Working tree `/Users/hey/.codex/worktrees/m6-client-20261014/PendingCrew`, branch `codex/mac-workspace-polish-20261021`, source HEAD `111b8a030ae0a6a8d347ec074a7a782b56ecf5e0`; initial Git status clean. `5a0d931f` verified ancestor. No applicable AGENTS.md found in checkout or parent directory.
- Initial committed demo has since been superseded by the other session's uncommitted main-checkout demo. That author's files were read only and never staged, reverted or overwritten by this writer.
- Stable read snapshot: `/private/tmp/pcw-demo-baseline-20261022-frontend/manifest.json`. 65 source/README/package files hashed before and after copy, identical; manifest SHA-256 `2165a5f75f1037b6cc203e6eae51e28d3e730e49df48071b7ff969214644d12b`. Original path: `/Users/hey/Untitled/Pendingname/PendingCrew/design/prototypes/2026-10-14-react-product-tour`.
- Primary executable reference: `PendingCrewTour.tsx`, ConversationList, Chat, Workspace, Inspector, TodoTags, TodoOwner, reducer, todoModel, displayText. README is historical and does not override the actual five top-level tabs. Root subsequently saved this exact snapshot as design-only commit `fdf1209e`; its source hashes match the 65-file snapshot. Root program-ledger checkpoint is `ee1404252f6b73bfd5ab6f184f934f81bd55ae8e`. This WIP is saved as its own local checkpoint and ordinarily rebased onto that baseline, without discarding edits, mixing other writers' files, or rerunning stable checks for design/documentation-only ancestry changes.

## Product module implemented so far

The original CrewSidebar/CrewCenter/Chat/SessionWindow remain the application. No alternate app shell, model execution, local demo database or synthetic product records were introduced.

- Permanent mixed channel rail outside the group's content switch; five visible routes: 消息、人待办、AI待办、文档/数据、计划/进展. Existing internal `Todo` routes remain compatible as AI待办. Narrow center columns have a full conversation popover and horizontally scrollable navigation, so no controls require a wider screen. Agent sessions stay explicit separate entries in the same rail; opening their terminal does not replace the route header.
- Original chat remains mounted and disabled/hidden outside its route. Existing channel-bound drafts, scroll/pagination identity and keyboard focus-reset implementation are retained. The rail uses actual channel purpose and unread/@ counts; the real ChannelSummary has no last-message/time field, so demo times are never copied as product facts.
- Group AI/human lists consume only channel.workList items; requirement creation and work association retain exact backend draft/request semantics. Searches are keyed by crew/channel/kind in the parent view so returning from other tabs retains each filter. Inspector human task rows open 人待办. The explicit menu opens the machine's shared crew work ledger and organization-wide human inbox; those are not presented as this group's ledger.
- Group progress uses only explicitly linked W identifiers and calls real work.get into the same WorkStore, including readable foreign-crew references. It never guesses group membership from crew, owner, requirement source or Plan parent. Existing expand/collapse, details and operations are reused.
- Mac library separates real document resources, native document placements/knowledge slices and free Agent-authored web pages. Page-type selection and persisted page preferences are separated; leaving a page revokes its renderer. This is still explicitly the crew library while group resource scope is a backend dependency. No fixed Agent page or new rendering contract is invented.
- Already accepted iOS composition is guarded by the original `#else` implementation; shared view additions have defaults preserving existing behavior. No resource-cancellation fix was duplicated. No launch/quit/session policy or installed build label was changed.

## Required remaining integration

Root has accepted that true group resource placements/folders are part of this requested corresponding logic. Backend will deliver the next complete module after its M4 freeze: shared SQLite content sources with independent per-channel references/placements, channel-owned new content and lists, per-channel per-view folder scopes, revision/CAS/events/ACL. Frontend has not fabricated DTOs or claimed the current crew library is group isolated.

Latest demo also requires durable multi-tags on Request/HumanAsk (preset plus custom trimmed names; add/remove, chip navigation and per-kind counts), direct Request/HumanAsk owner assignment, direct Request completion toggle, and new human-created H tasks. Existing single-folder classification, Plan owner/status and Agent ask.raise are not asserted equivalent. Exact UI behaviors and current DTO gaps have been sent to backend/root for one contract decision. Pending implementation must consume that decided contract; never persist these mutations in Swift dictionaries.

Group progress folder controls still consume crew-scoped existing resources pending the decided channel folder API. Final review must verify this becomes true channel scope together with the resource module. The current WIP is not to be packaged as completed group isolation.

## Writer boundaries and verification

Swift files touched: `Sources/Mac/Crewd/CrewdOriginalWorkIntegration.swift`, `CrewdChannelTodoView.swift`, `CrewdWorkPanels.swift`, and `Sources/CrewdRemote/CrewdKnowledgeViews.swift`; this module handoff only. No Rust, proto, shared Rust target, design files, root program ledgers, normal app, private VPS material or installed binary touched.

Current verification: `xcrun swiftc -frontend -parse` for these four actual Swift sources and `git diff --check` pass. This is syntax verification, not a full typecheck, UI acceptance or executable candidate. No new validation framework, new CI, dependency download, cold DerivedData, cache deletion or model was run.

When complete: freeze a single exact source ref, root/quality independent affected-scope review, resolve real findings, then one accurate macOS candidate build using existing `PendingCrew-aehzthzfjzjnxdfaqjkozipivxpn` DerivedData/SourcePackages. No installation, normal app launch or user-session test is authorized by this checkpoint. Preserve earlier iOS/VPS/module evidence; do not rerun stable accepted business flows.
