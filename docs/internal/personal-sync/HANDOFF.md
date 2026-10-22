# Frozen Apple personal sync source module

Owner: apple_sync_module. Location: /private/tmp/pcw-apple-sync-module-20261022. This is a separate candidate, not A and not integrated into any project. No existing Swift source, Xcode project, entitlement, Rust, shared plan, app, Keychain, iCloud account or Core was changed/accessed. Frontend owns integration.

Sources (copy together into the shared Mac/iOS source group only after source review):
- Sources/AppleBranchStore.swift: stable business keys; actor local storage; per-device coalesced branch checkpoints; explicit merge/retire; permanent body-free tombstones; local-only recovery of offline edits hidden by remote retire; account partitions and in-flight response invalidation.
- Sources/ApplePortableDirectory.swift: closed host allowlist with decode-time validation and tiny explicitly injected KVS preferences.
- Sources/AppleCloudKitTransport.swift: explicit configured CKContainer/privateCloudDatabase, typed branch records, query pagination + direct fetch, CAS ifServerRecordUnchanged, per-record failure handling, account identity checks and notification cancellation.
- CONTRACT.md: interface and UI behavior, disabled/local-only defaults and signed qualification requirements.

Payload integration: existing CrewComposerDraftStore.Draft includes text/stagedMentions/replyTarget; use the full Codable DTO and make its value-only types safely Sendable. Do not reduce to text. The independent test Payload has structured mention/reply fields; this module was not compiled against the full existing composer UI. Do not use existing journal namespace (local record UUID/grantRevision) as content key.

16 local behavior checks PASS in evidence/green-final.txt. These use the actual AppleBranchStore and directory implementation plus a controlled MemoryCloud actor, temporary files, and no production services:
1 offline reopen structured payload; 2 coalesced checkpoints (32 edits, two cloud branches); 3 concurrent device originals; 4 offline stale tombstone suppression; 5 local-only original recovery; 6 recovery/tombstone reopen; 7 account-switch in-flight response rejection; 8 account/environment namespace isolation; 9 switch-back local restore; 10 no-cloud local-only; 11 capacity retains memory+visible error; 12 unsaved input blocks discard on account switch; 13 corrupt original preserved; 14 portable directory rejects local/secret endpoints; 15 unknown private fields rejected; 16 public IPv4/IPv6 and explicit LAN hints accepted.

Real behavior red: first runnable offline fixture failed exit133 on incorrect Foundation path canonical comparison, reproduced twice, then minimal POSIX realpath fix passed. evidence/behavior-red.txt gives exact failure. No pre-existing production sync baseline existed; no missing-symbol compiler error is claimed as behavior red.

Mutation: evidence/AppleBranchStore-mutant.swift changes only tombstone dominance to return the nonretired input. It compiles; running mutant-checks fails exit133 exactly at “tombstone prevents offline resurrection” (evidence/mutation-red.txt). Normal final binary passes all 16. Mutation source is evidence only, never integrate it.

Validation: full source+Tests compiled with xcrun swiftc -swift-version 6 -parse-as-library, existing verified aeh ModuleCache.noindex under bounded frontend-owned compiler windows. All compile steps exit0. Final normal and mutant binary runs use /private/tmp only. No new dependencies, cold DerivedData, App/SPM build, real cloud calls or signing. Current test binary is checks; rerun with a new temporary fixture root to avoid stale test files. Source list is exactly Sources/*.swift plus Tests/main.swift; never glob evidence/*.swift into shipping builds.

Boundary: CAS is official SDK source/type verified, not a real server conflict test. CloudKit entitlements/container/schema/index/user account and Mac+iOS signed runtime behavior remain unqualified. Normal app must default cloud disabled and local-only; do not call accountIdentity/exchange/KVS on startup merely because source exists. Cloud access requires approved actual config+opt-in. No claim of actual two-device/iCloud delivery, no server schema/API changes, no automatic connect/enroll/Core takeover/send/claim/session resume. Account notification cancellation is best effort for already accepted server operations; actual account-switch timing must be verified on signed two-device builds.

Directory endpoints support portable wss roots using public IPv4/IPv6 or explicit LAN/DNS hints. Localhost/loopback/unspecified/multicast, Unix paths, userinfo/query/fragment/path credentials are refused. Private LAN addresses are permitted as user-supplied hints, with reachability/enrollment still required. DNS resolving unexpectedly to loopback must also be refused by the actual connection/enrollment layer; this discovery module never resolves or connects. Tombstones and local recovery count toward an 8MiB file cap and are not silently pruned; failure retains current text, exposes error, and requires user-visible save/export handling. Persistent deviceID and single writer per account+collection are integration obligations. No device enrollment secrets are synced.

SHA256 frozen source:
f73cd1356a7bb4d2cf4699f9a4fa8dcca32f00d2893d7d0819e97ecde8ff9d09  Sources/AppleBranchStore.swift
1c61180431e6481d26efde3e0d041f84c70cae509a356d4646896dacefe24615  Sources/AppleCloudKitTransport.swift
4ce69cb9c68e90c467184ba2615eefdb6f6fba71f47f5ec36d52d457fa0bc7a3  Sources/ApplePortableDirectory.swift
7ca651de6459396f00df9e2ab95a082834de99430d4fc60b31e5cc80ca89a59d  Tests/main.swift
fec7677a23b62495a092a8f7c94e675ed9b503a5f0502d8dd2de8d73e1121469  CONTRACT.md
