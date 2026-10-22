# Shared glass, active computer and original footer

Baseline: `acd097828a9917411e06c9f74f71a0fc6faa2680`, following installed native UI source `d4866cb705abbf3c5c65d039c4404d19a0df0d4f`. The latest user instruction supersedes the prior opaque system chat canvas and plain segmented workspace appearance in NATIVE-RULING.md. Existing full-height sidebar, native search and actual work/session flows remain.

## Presentation and availability

The window has one system material background. macOS 15+ uses public SwiftUI `containerBackground(_:for: .window)`; macOS 14 uses public AppKit NSVisualEffectView behindWindow. iOS 18+ uses the native navigationSplitView container material; earlier supported iOS uses Material. There is no new sidebar card, wallpaper, chat-wide plate, or material per message. Local message bubbles, input surfaces and document blocks retain their necessary reading contrast. System Reduce Transparency / contrast settings control material rendering.

The five original workspace values remain `消息 / 真人Todo / Todo / 文档 / 进展`. SwiftUI buttons inside one Capsule use public Liquid Glass on macOS/iOS 26+, system regularMaterial on earlier systems. The original section binding, mounted chat instance, per-conversation drafts, permissions and receipts remain unchanged. Horizontal scrolling keeps all five domains reachable on narrow screens; selection scrolls into view. Buttons expose selected accessibility traits, with mobile 44pt minimum targets. No third-party visual dependency or deployment target increase.

Official sources checked, alongside the installed SDK's exact availability declarations:

- https://developer.apple.com/documentation/swiftui/applying-liquid-glass-to-custom-views
- https://developer.apple.com/documentation/swiftui/view/containerbackground(_:for:)
- https://developer.apple.com/documentation/swiftui/material
- https://developer.apple.com/documentation/appkit/nsvisualeffectview/material-swift.enum/windowbackground

The current MacOSX27.0 SwiftUICore interface declares glassEffect at lines 3062–3064 available macOS/iOS 26. `ContainerBackgroundPlacement.window` is public macOS 15+. SDK-supported fallbacks retain the project's macOS 14 / existing iOS deployment configuration.

## Actual host identity and connection

The current authority owns the crew list projection. A single active-computer title above that list uses the selected existing paired-summary name only when its UUID matches the workspace's authorityRecordID. A local workspace is “本机”; a remote connection without an available verified catalogue label is “当前电脑”. No B directory, enrollment, credential lookup, new host cache or RPC is added. The legacy machineOnline(local || historical online status) predicate is deliberately not used for this crewd host.

Green means `client.readyForWork`: acceptHello has verified API/runtime identity and the original remote binding/TLS or local kernel peer, then completed the events subscription on the current connection epoch. This does not inspect managementRole or writable ACL: an authenticated read-only connection is connected too. Raw transport connected but incomplete hello is verifying (orange); connecting is orange; disconnection clears readyForWork and is grey. The accessible name includes the actual textual state; paired-only records and socket presence are insufficient.

An unfinished original operation is independent of connectivity. The machine entry retains the existing complete error text, original request recovery and manual management flow; the compact toolbar entry remains reachable when the sidebar is hidden. No error/Unknown is cleared, acknowledged, automatically retried, resumed or resent by this visual change. Existing explicit lookup remains original UUID only.

## Exact old footer boundary

Git `fd0af51b` / `488631b1` CrewSidebarView shows `if !usesCrewdAuthority { quotaFooterLine }`, followed by the existing identityFooter. QuotaRingsFooter and its layout/hover/refresh implementation are preserved byte-for-byte; the existing 26pt seeded avatar and “人” row are also preserved. The later c37 provider-name / “额度未读取” rows and the added connected footer are removed from the crewd presentation.

The current Core has session token/context snapshots, not an authenticated subscription-quota feed. Those are not interchangeable. This module restores the original footer visibility/visual conditions, and does not claim restored subscription quota support: unavailable windows remain absent/unknown. It does not restart QuotaCenter probes, legacy quota-file following, old Swift runners, background refresh, login, or session startup. There are no fake zero rings or cross-host quota values.

## Verification boundary

Full ordinary Mac and iOS Simulator targets use the existing aeh DerivedData/SPM caches. Their logs, exact argv, before/after Swift/project/lock hashes, and historical d486 helper identity are preserved in the module's ignored validation packet. This is source/target verification, not a new candidate or live UI acceptance. Root freezes the complete integrated input before a new accurate normal helper/candidate.

The installed d486 product, daily data, old candidates/zip/log bytes, paused B f0 checkpoint, Cloud source and APNs remain untouched. No GUI, installation, account, pairing or model action is performed here. Future GUI starts require the reviewed normal execution context; the earlier sandbox AppKit registration failure is not worked around through product/guard changes.

Next actual UI acceptance should inspect continuous window material and readable long content, five-domain selection on Mac and phone/iPad, original footer absence for unavailable quota, active-host green versus verifying/disconnected and independent Unknown, and hidden-sidebar access to the same original status/recovery. No real quota/model probe should be run merely to produce a screenshot.
