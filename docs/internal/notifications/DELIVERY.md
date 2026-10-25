# Native APNs backend module — source/local handoff

This module adds an optional default-off native HTTP2/TLS/ES256 sender, eight typed API3 notification methods (schema137), the Store19 writer barrier, original-source in-transaction observations, protected provider/token material, and the existing Local Human/rootFD configuration routes. The canonical contract is CONTRACT.md; exact final commit, full tracked input inventory, artifact hashes, commands and terminal results are sealed in the primary ignored `native-apns-20261023` source-freeze and verification artifacts. The worktree is `/private/tmp/pcw-native-apns-20261023`, branch `codex/native-apns-20261023`.

## Actual production path

Existing WorkBook/message/native-Codex/sealed-Claude/Core-decision producers -> their original owning transaction/accepted observation -> matching installation outbox -> original Core-owned session eligibility queue or DB-owned Work/message claim -> durable Unknown with original apns-id -> bounded native reqwest/rustls HTTP2 sender -> original attempt-only settlement. Restore, grant revocation, session generation/decision/working changes and installation/provider rotation preserve the no-replay boundary. Enqueue fixes a24h expiry. No subscription/configuration means no observation flow or secret/network read.

Local Human configures through the existing owned App/Core capability; Linux uses the existing true-root-FD typed host.notificationConfigure and host.notificationRequestLookup. The bounded stdin/root0600-file CLI protects write-only material and its original request witness. Paired devices register only under their own current mTLS grant/pin/crew permission. APNs context hints carry original installationId+deliveryId; exact read-only deliveryGet and current object ACL are required before navigation, including after restore or ID reuse. They never authorize execution or retry.

## Affected verification

The replacement qualification is44 actual passing cases, using the corrected original testkit remote case and the two repaired local h2 fixtures, with all other unchanged source/earlier evidence reused. The original397 claim that its default-feature remote exit0 counted as one test was incorrect: that binary was cfg-disabled and ran0 tests. Its original logs/manifest remain unchanged; they do not qualify the remote route. This is not a stable-suite rerun:

| Original seam | Cases | Evidence log(s) |
|---|---:|---|
| Native ES256/actual negotiated TLS+h2/exact headers/bounds/known rejection/lost response/no retry |4| h2-fixture-fix-native-final.log |
| Full durable Runtime -> actual local h2; Unknown before I/O, exact hint and24h expiry, reopen/rotation no connection; cipher/AAD/redaction |3| h2-fixture-fix-runtime-final.log (1 actual Runtime); original cipher/redaction2 unchanged |
| Core owned claim/generation/decision/idle plus native/approval observations preserving original completion chain |2| final-current-core.log |
| Real synthetic managed PTY/receipt/hook prompt/session/subagent/stop qualification |1| final-current-managed.log |
| Original rootFD typed configure/lookup/secret failure/final permit revocation |1| final-current-rootfd.log |
| Original Store durable ownership/CAS/dedup/Unknown/reopen/revoke/late response/expiry/source identity/no-interest/same-TX/restore click ID boundary |13| final-current-store.log |
| Guarded current18->19/retained17 protected path + accurate preserved normal old18 refusal with DB bytes unchanged |3| final-current-store19.log; final-current-old18-refusal.log |
| Actual collab structured Human message + internal/literal exclusions and rollback |1| final-current-message-producer-02.log |
| Existing local real mTLS route/default-off/remote configure denial/exact read-only empty lookup |1| h2-fixture-fix-remote-one.log (correct --features testkit; running1/passed1) |
| Existing Jev fake HTTP bearer/secret tests with the actual unified reqwest http2 feature |2| final-current-jev-http2-post.log; final-current-jev-http2-secret.log |
| Exact active noncompaction native Codex outcome filter |1| final-current-codex-source.log |
| Original installer/package/static CLI qualification and secret-safe input/witness |12| final-current-installer.log |

Schema137 generation equals checked JSON; every original129 method and original definition retains its prior value. Only h2.0.4.20 and fnv1.0.7 were added from the approved official registry; no existing locked package was upgraded. Exact versions/checksums are in Cargo.lock and the original dependency evidence. Jev's existing client/configuration is unchanged; the feature-unification cases use only its original local fake server.

Three original-seam mutation controls are genuine assertion failures with restored source and matching green cases: omit the original grant-revoked hook -> pending instead of suppressed; select/reclaim durable Unknown -> claim after actual DB reopen; remove managed prompt equality -> foreign prompt invents failure. Earlier process-birth sandbox/fixture-layout failures are retained separately and are not these red assertions. The managed red/green uses the same original deps/fake_claude layout and actual synthetic PTY birth gate; rootFD scope.close is not represented as a real socket EOF observation or real Linux root/systemd proof.

## Independent quality correction

The independent reviewer preserved an original60s Runtime hang and35s native local_case hang without an assertion. Locked h2.0.4.20 server.rs explicitly requires Connection::accept/poll_close polling; operating only on RecvStream does not advance socket I/O. Both original fixtures waited on DATA without simultaneously polling their Connection, and awaited their server task without a deadline. The replacement changes only their cfg(test) code: select DATA together with Connection::accept, reject an unexpected second stream/early close, print safe phase names and bound the entire synthetic server to12s; the Runtime pump also has a12s test deadline. Production sender/Runtime/DTO/Store behavior is unchanged. The old hangs remain evidence of test unreliability, not invented production failures. No guard, response assertion or timeout contract was weakened. Correct-feature remote qualification is a real exact one-test run, not another exit0 count inference.

## Remaining delivery layers

This is complete backend source and local synthetic evidence, awaiting independent quality. It is not a production normal/release package, Linux/systemd install, real Keychain/provider-key qualification, signed device-token registration, Apple APNs acceptance, device delivery/display/tap, or Swift opt-in/navigation acceptance. No real Apple material or network, model/provider/SDK, public service, Worker, CI, VPS, publication, global configuration change or native16 activation was used. Default remains off. APNs HTTP200 means only acceptedByApns, never business Done or device read; Unknown is retained without automatic resend.

The accepted B baseline is a normal merge ancestor; its frozen bb033/20c artifacts/current Store18 candidates are unchanged. This source qualifies Store19 installer metadata, not the old B-only Linux input. The integration owner may select a future single frozen B+APNs+client/UI source for its separately authorized one manual Linux build; this module does not dispatch it.

## Secret-safe ordinary Core management CLI (2026-10-25)

The ordinary `crewd` now exposes the existing Local Human API without putting provider JSON in process arguments. Use the current Core's canonical data directory and a configuration JSON file owned by the current user, mode0600, regular and not a symlink:

```sh
crewd --data-dir '/absolute/canonical/Core-data' notification configure --input-file '/absolute/private/apns-configuration.json'
# The same bounded JSON may instead arrive through stdin, without terminal output:
crewd --data-dir '/absolute/canonical/Core-data' notification configure < '/absolute/private/apns-configuration.json'
crewd --data-dir '/absolute/canonical/Core-data' notification lookup --request-id '<original request UUID>'
```

Input is the existing `NotificationConfigureParams` JSON (`requestId`, `expectedRevision`, `enabled`, optional `provider`), bounded to16KiB. `provider.privateKeyPkcs8Base64` retains the existing API format and the same preparation contract as Linux `notification-configure`; `--input-file` is not an Apple `.p8`/PEM reader. Prepare that JSON privately through the existing provider-configuration process. Do not put Apple key material in argv, an `echo` command or terminal output. The CLI opens the operator's input read-only and does not change it.

Before its single configuration call, the CLI writes only the Linux-shaped `{format,kind,requestId,payloadSha256}` witness under the Core data directory's private `notification-cli/notification-UUID.json` (directory0700/file0600). It never saves provider JSON or a private-key copy. An existing UUID witness cannot be overwritten or submitted again. A lost/invalid reply leaves that original witness; explicit `lookup` uses only `notification.requestLookup`. Unknown and notFound never trigger a configuration resend, replacement UUID or push. Output is a typed non-secret status summary, not raw RPC/error bodies or proof of Apple delivery. Existing Local Human permission checks still apply; this CLI does not create host identity, trust or sender authorization. The sender, rootFD Linux route, API/schema and protected server key storage are unchanged.
