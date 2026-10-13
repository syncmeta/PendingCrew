# Main CI scroll measurement recovery — 2026-10-14

Input: exact main `67430035`, isolated `codex/mac-ci-recovery-20261014`. Product source, project definition and M2 candidate are unchanged. This fixes the offscreen test instrument; it does not claim a product scrolling fix or remote CI success.

## Observed failure and cause

The current Xcode 26.5 CI evidence is `/private/tmp/pcw-M3-main-ci-67430035-failure.log`, lines 10051–10059. NSScroll geometry offsets are finite and move exactly 40 points at all five steps (1689 → 1649 → 1609 → 1569 → 1529 → 1489). The long row's viewport y is 384.333 → 424.333 → 464.333 → NaN: the next expected y is 504.333, beyond the 500-point viewport. The lazy row is legitimately reclaimed; its PreferenceKey supplies the default NaN. Comparing successive missing row positions falsely reports a scroll failure.

The local Xcode 27 original source instead places the same row at -1002 initially; all five steps remain measurable. That explains why the local original suite was green without establishing compatibility with 26.5.

## Change and sensitivity

Keep the original initial layout, wheel priming and all five measured gestures. Capture one finite row baseline and calculate expected row coordinates from actual wheel distances. Missing row geometry is acceptable only when that expected row lies completely outside the measured viewport. Missing geometry while in the viewport still fails. Do not retain a stale coordinate or substitute a synthetic measured value.

For every gesture independently assert actual AppKit clip bounds and SwiftUI offset move exactly 40 points. Require nonempty geometry callbacks and bound every intermediate offset; retain the existing intermediate row-frame assertions. Thus virtualization cannot conceal an actual viewport jump or a jump-and-return.

The existing old visible-ID getter counterfactual still produces a measured 1002-point jump. An actual one-case mutation enabling that getter also fails the new instrument (25 assertions, including actual 48-point movement rather than 40 and intermediate-offset violations). Its source and result remain separate from the candidate.

## Executed validation

Local environment: Xcode 27.0 (27A266a), macOS SDK 27. Reused `/tmp/pcw-codex-first-xcode` and `/tmp/pcw-codex-first-spm`; no dependency downloads, Cargo, app installation or model invocation.

```text
xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew -destination platform=macOS
  -derivedDataPath /tmp/pcw-codex-first-xcode
  -clonedSourcePackagesDirPath /tmp/pcw-codex-first-spm
  -disableAutomaticPackageResolution -skipPackageUpdates
  -only-testing:PendingCrewTests/CrewChatBottomReboundProbeTests
  -only-testing:PendingCrewTests/CrewChatExpandAnchorProbeTests
  -only-testing:PendingCrewTests/CrewChatWindowTests
  -resultBundlePath /private/tmp/pcw-M3-mac-ci-scroll-recovery-04.xcresult test
```

Actual exit 0: 64 tests total, 62 passed, 2 pre-existing runtime calibration skips, 0 failures. The two skips are `test_未走的路_人滑上去之后bottom锚与top锚读数相同` and `test_锚标定_贴底时必须分得出bottom和top`: both anchors produce zero movement in this local SwiftUI environment, so the existing probe declines to infer behavior. No skip was added or expanded. History expansion/streaming position preservation runs are included.

Evidence:
- Green: `/private/tmp/pcw-M3-mac-ci-scroll-recovery-04.log` and matching `.xcresult`.
- Mutation: `/private/tmp/pcw-M3-mac-ci-scroll-mutation-red-01.log` and matching `.xcresult`, actual exit 65.
- Exact restored candidate test SHA256: `fa7fc0ac6aabaa31eaf484fb24b811b7541a2ec983946e8c34f6a6a9f69f46b6`; `/private/tmp/pcw-M3-scroll-recovery-green-source.swift` has the same hash.
- Deliberate mutant: `/private/tmp/pcw-M3-scroll-recovery-mutant-source.swift`.
- Earlier setup experiments 01–03 and their full logs/results are preserved. They changed the eager→lazy setup and exposed estimated-height clamping/priming behavior; they are not product regressions or acceptance evidence. The final change removes those setup modifications. Trial03 source is preserved at `/private/tmp/pcw-M3-scroll-setup-trial-03.swift`.

## Remaining gate

The exact restored source must pass the real Xcode 26.5 CI; local 27 results do not replace that gate. This unit does not establish device, installed runtime, visible window or account acceptance. No main merge/push was performed by this owner.
