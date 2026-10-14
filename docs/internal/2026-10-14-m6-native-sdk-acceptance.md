# M6 Native client: actual project SDK acceptance

The complete original PendingCrew target now compiles the Native remote client and its fixed X509 dependencies on macOS and iOS Simulator. This is target-build evidence; it does not prove a launched window, live pairing, personal Keychain, physical iPhone/iPad use, installation, or a complete distributable artifact.

## Fixed input

- Source: `1e51045a7e5de6a9b04ec0d74ea7d87091e0c19d`, clean Swift/project inputs.
- Input inventory: `/private/tmp/pcw-M6-native-main-build-02-inputs.json`.
- Every inventoried file SHA256 was checked after both target builds; zero differences. Subsequent `2d0c445e` and `a0844703` change only backend Rust.
- XcodeGen 2.46.0 source registrations and actual checked-in Package.resolved; X509 1.21.0, swift-asn1 1.7.3, swift-crypto 5.0.1. Existing dependency pins preserved.
- Xcode 27 local SDK. This does not replace the remote Xcode 26.5 gate.
- Shared cache: `/private/tmp/pcw-codex-first-xcode` DerivedData and `/private/tmp/pcw-codex-first-spm` packages; no second dependency checkout, no app launch.

## Actual target commands and outcomes

Both commands use `xcodebuild -project PendingCrew.xcodeproj -scheme PendingCrew`, the above DerivedData and package directories, `-disableAutomaticPackageResolution -jobs 2 CODE_SIGNING_ALLOWED=NO build`.

- macOS: `-destination platform=macOS,arch=arm64`; **terminal exit 0 / BUILD SUCCEEDED**. Log `/private/tmp/pcw-M6-native-main-macos-build-02.log`, result `/private/tmp/pcw-M6-native-main-macos-build-02-result.json`.
- iOS Simulator: `-destination generic/platform=iOS Simulator -skipPackageUpdates`; **terminal exit 0 / BUILD SUCCEEDED**. Log `/private/tmp/pcw-M6-native-main-ios-simulator-build-01.log`, result `/private/tmp/pcw-M6-native-main-ios-simulator-build-01-result.json`. The complete generic Simulator build compiled arm64 and x86_64 target inputs.

Mac attempt01 is preserved in `/private/tmp/pcw-M6-native-main-macos-build-01.log`: official dependencies compiled, then three source errors (throwing EKU construction and two shadowed error assignments) were corrected before the fixed input above. It is not counted as success.

## Remaining complete-module acceptance

The shared message submission retention has an actual EOF RED/GREEN, independently run by QA; it preserves original method/params/text and sends once. That test is not Native TLS or visible-window proof. QA next reuses the actual SDK objects for a temporary memory credential store and real production TLS pairing/Core/event/revoke flow. No personal Keychain is used in that fixture.

The previous immutable Codex-first artifact and direct-swiftc builder do not contain this Native remote entry or X509 dependencies. A new complete fixed-input GUI recipe, dependency proof, normal backend witness pairing, and artifact checker are required before claiming an M6 distributable. Physical phone/tablet, visible-window, and personal-data acceptance remain distinct.
