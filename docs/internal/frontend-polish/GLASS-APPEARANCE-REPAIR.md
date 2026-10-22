# GLASS-CONTRAST-01: one material and foreground contract

Base `2feb1b771ab3e14446ee78d81ac51b1a8367afff`. This repair supersedes only the material/foreground implementation described by GLASS-HOST-RULING.md. Its real host identity, original footer, five section bindings, mounted conversations, native search and all business/Unknown/permission behavior are retained.

## Actual evidence and limits of diagnosis

QA's exact 2feb preview produced low contrast in both wide and actual 900pt layouts: the continuous content region was pale grey while five-domain labels, unselected conversation captions, folder/tool controls and draft captions retained light foregrounds. Solid message bubbles and the original Recovery sheet remained readable. See the immutable 2feb candidate packet's GUI-DISCOVERY-FINAL.json (`eaf0b4ea1acc9d65cd6be98d6ddee8d58d55f8158bf84fbb3520da757e416335`) and visual-block-2feb-glass-contrast.md; no image or data is modified for this repair.

Read-only preference evidence: the preview's pendingcrew_appearance_mode key was absent (follow system); AppleInterfaceStyle was Dark. RootView applies the existing preferredColorScheme. No source assigns NSApp/window appearance. This establishes the contract mismatch, not a proven erroneous runtime NSWindow appearance. The material's exact runtime NSAppearance was not observed and is not claimed as a diagnosed wrong value.

The added ultraThin window background exposed a brighter region than the old opaque canvas. Existing static warm secondary colours were specified for that opaque light/dark canvas. Apple also documents that custom foregroundStyle colours disable the vibrancy that hierarchical primary/secondary styles preserve. Full-target compilation alone did not prove the resulting rendered contrast.

## Complete repair

- Every supported Mac version now uses the same public NSVisualEffectView semantic windowBackground, behindWindow, followsWindowActiveState. The view receives the current SwiftUI colorScheme and updates its own NSAppearance to aqua/darkAqua on make/update. No system preference, NSApp appearance, window appearance or forced global theme is changed. The window remains non-opaque with a clear backing; there is no black rectangle, opaque chat-wide plate, sidebar card, new theme manager or parallel Mac14/Mac15 implementation.
- iOS uses public regularMaterial rather than ultraThin; the iOS18+ navigationSplitView background content explicitly inherits the same current scheme. Earlier supported iOS retains regularMaterial. Existing public Liquid Glass capsule availability is unchanged.
- The shared reading/chrome foreground roles now return hierarchical primary/secondary styles. Shared panel/field headers, detail values, tool labels and capsule labels use those roles directly. The existing original chat reply/loading tools, crew captions and right-side member/work labels use the same system semantic hierarchy. Capsule selection retains its original binding, fill, selected accessibility trait and scalable target; unselected labels no longer rely on a pale brand/secondary colour to be legible.
- Message bubble fills and their paired text colours, source content, selection fills, brand/status colours, error/Unknown semantics and disabled submission guards are retained. Draft captions already use semantic secondary; native search and folder controls already use system rendering. They inherit the repaired continuous material rather than receiving local ad hoc colour patches. The clear Recovery flow is not rebuilt or replayed.

Public references checked:

- https://developer.apple.com/documentation/swiftui/material
- https://developer.apple.com/documentation/swiftui/view/foregroundstyle(_:)
- https://developer.apple.com/documentation/appkit/nsvisualeffectview/material-swift.enum

## Source and delivery boundaries

The B merge checkpoint is preserved on codex/host-enrollment-consumer-20261022 at b8413bf9aa1a4d317c5746422d10296734bea11f (original f0 checkpoint retained). This repair is separately based on 2feb and contains no enrollment, Cloud, APNs, backend, schema, authorization or journal changes.

Affected verification is the ordinary complete Mac and iOS targets, serially using the existing aeh DerivedData/SPM cache. Exact argv, both logs, 388 before/after/current Swift/project/lock hashes and the accurately historical 2feb normal helper are preserved in the repair's ignored source-freeze packet. No new candidate, provider process, network pairing, real Keychain operation, GUI, account, installation, permission or system-theme action occurs here. Installed daily d486 and every prior candidate remain untouched.

Actual contrast/material quality is not claimed by source review or compilation. After the complete integrated input and accurate normal are frozen, the next real preview must confirm the same existing wide/narrow content: five-domain labels, nonselected conversations/summary, folder/tools/search, right work/member/error area and draft status/disabled affordances, while glass remains visible and solid bubble/selection text remains clear. Existing original UUID recovery and business evidence are reused; no new test matrix or synthetic content is required.
