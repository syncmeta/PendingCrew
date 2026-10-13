# Original project integration acceptance — 2026-10-14

The original `PendingCrew` Xcode target builds and its macOS tests pass with the newly registered Mac/Crewd sources. The same target also builds for a generic iOS Simulator. This is original-project compatibility evidence, not proof that the new crewd client has been implemented on iOS, that a watchOS product exists, or that an app has been installed or used with a real account.

## Inputs and environment

- Worktree: `/Users/hey/.codex/worktrees/global-work-flow-20261013/PendingCrew`.
- Generator checkpoint HEAD: `11d020f2dd8824693666c790cadc71e83350027d`. At completion, tracked `Sources`, `Shared`, `Resources`, `Tests`, `Config`, `project.yml`, project and Package.resolved had no diff from the frozen a9c3014d inputs. Other QA documentation/scripts committed meanwhile do not change these compilation inputs.
- Cached, pinned XcodeGen **2.46.0** ran through `scripts/gen-project.sh`; project SHA256 was `cd16c703f7a400be125c674f54e3acae17593181015fbd38914f0df69a1aa177` both before and after. No generated drift or product change.
- Local compiler: **Xcode 27.0, build 27A266a**, macOS/iOS Simulator SDK 27.0. CI pins `/Applications/Xcode_26.5.app`; that installation is absent here. These local results do **not** replace the remote CI 26.5 gate.
- Reused this chat's existing `/tmp/pcw-codex-first-xcode` DerivedData (1.0 GiB initially) and `/tmp/pcw-codex-first-spm` (232 MiB), with automatic resolution and package updates disabled. No second dependency tree, network download, Rust build or cache cleanup was needed. Free space was 6.3 GiB before and 5.1 GiB after.
- Scheme supplies `PENDINGCREW_DATA_DIR=$(SRCROOT)/.test-data-root`; no daily application was installed or launched. The opt-in subscription/network recorder remained skipped.

## Actual commands and outcomes

```sh
XCODEGEN=/Users/hey/Untitled/Pendingname/PendingCrew/.tools/xcodegen-2.46.0/bin/xcodegen scripts/gen-project.sh

xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'platform=macOS' \
  -derivedDataPath /tmp/pcw-codex-first-xcode \
  -clonedSourcePackagesDirPath /tmp/pcw-codex-first-spm \
  -disableAutomaticPackageResolution -skipPackageUpdates \
  -skip-testing:PendingCrewTests/HelperBuildPerMemberTests \
  -resultBundlePath /private/tmp/pcw-M3-main-macos-01.xcresult build test

xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew \
  -destination 'generic/platform=iOS Simulator' \
  -derivedDataPath /tmp/pcw-codex-first-xcode \
  -clonedSourcePackagesDirPath /tmp/pcw-codex-first-spm \
  -disableAutomaticPackageResolution -skipPackageUpdates build
```

All three commands completed with **exit 0**. macOS reports `TEST SUCCEEDED`; xcresult confirms **3202 total, 3187 passed, 15 skipped, 0 failed**. iOS reports `BUILD SUCCEEDED`. There is no product-source compilation blocker in this local toolchain.

`HelperBuildPerMemberTests` was explicitly excluded exactly as in existing CI, independently of the 15 runtime skips. Those 15 are not counted as passes:

| Group | Skipped | Actual reason |
| --- | ---: | --- |
| AgentTuiFixtureRecorder | 1 | Subscription/network recording is opt-in and was not enabled |
| CrewChatExpandAnchorProbeTests | 2 | SwiftUI top/bottom anchor probe could not be calibrated in this environment |
| CrewChatOpenCostTests | 8 | Required real-whiteboard performance fixture is absent |
| CrewLastMessageCacheTests | 1 | No live whiteboard benchmark directory was supplied |
| CrewMentionFilterRealWhiteboardTests | 3 | Real-whiteboard reads reported errors; these were explicitly skipped, not reclassified as missing data or passes |

## Platform boundary and retained evidence

`project.yml` supports **iOS and macOS**; `PendingCrewTests` supports macOS. CI runs macOS build/test and iOS Simulator build. Despite its historical “three platforms” job label and an installed watchOS SDK, this project has **no watchOS target/scheme**, so no watchOS compilation is claimed. Mobile crewd functionality and any watch product remain subsequent implementation work.

Evidence:

- `/private/tmp/pcw-M3-main-project-generator-01.log`
- `/private/tmp/pcw-M3-main-macos-01.log`
- `/private/tmp/pcw-M3-main-macos-01.xcresult`
- `/private/tmp/pcw-M3-main-macos-01-summary.json`
- `/private/tmp/pcw-M3-main-macos-01-tests.json`
- `/private/tmp/pcw-M3-main-ios-simulator-01.log`

The independently accepted immutable a9 candidate and its b064 normal-backend witness were preserved without rebuilding or altering them. Main merge/push, remote CI 26.5, actual window actions, real-account use, installation and release remain separate gates.
