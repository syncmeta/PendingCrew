# Native APNs notifications — current module contract

Status: implementation in progress on `codex/native-apns-20261023`; not release, independent acceptance, or real-device delivery. Rust DTO authority is `crewd/crates/proto/src/notifications.rs`. This branch is independent of Bbb033/20c and the frozen UI/B normal inputs. No real Apple credentials, device tokens, APNs traffic, Apple portal operations, public listener, Worker, bridge, model, or deployment is authorized by this source work.

## Sources and meanings

Every notification identifies its original accepted source. A delivery ID indexes the delivery ledger; it is neither a task ID nor another business truth. Identity is original transport authorityId + actual Core dbId + original crewId + tagged source. Local pairing recordUUID, display name, token generation and installation revision are not source identity. The payload contains no task/message/prompt text.

| Kind | Qualified original source | Durable owning transaction | Meaning |
|---|---|---|---|
| `done` | WorkBook Done | `organization_work_events` insertion and `work_organization::save` | business work Done |
| `reported` | WorkBook Reported | same save | progress, not success |
| `blocked` | WorkBook Blocked | same save | work blocked |
| `askOpened` | WorkBook AskOpened | same save | Human ask opened |
| `replyReady` | exact active non-compaction Codex native completed turn | generation-checked Core/DbPort original turn trace transaction | provider reply ready, not business Done |
| `responseFailed` | exact active non-compaction Codex native failed turn | same accepted outcome transaction | provider response failed, not WorkBook failure |
| `replyReady` | sealed Claude managed same runtime/request/prompt Stop followed by actual idle_prompt | original managed receipt update, Completed with NULL turn ID | response waiting for instruction, not native turn completion |
| `responseFailed` | only an authentic Claude StopFailure correlated with the current managed runtime/request/prompt | generation-checked original failure observation | response failed; missing/foreign correlation remains Unknown |
| `humanApprovalRequired` | current structured runner approval/question, original Core request UUID | current Core generation's durable attention observation | needs Human, never auto-answer |
| `humanMention` | committed non-Human message with actual structured Human mention | original message insert transaction | Human mentioned, never inferred from punctuation/text |

The WorkBook source is current `organization_work_events`, not historical `work_events::append`. Preserve original `eventSeq`, WorkId `W<n>`, AskId `H<n>`, EntryNo `#<n>`, and already-issued public objectId. Never substitute a UUID, strip the ID prefix, or assume unrelated registries share a W number.

Codex completed/cancelled/failed qualification comes from the matched active native turn, not generic Health or Working(false). Compaction and foreign/replayed outcomes cannot create reply reminders. Claude's legacy raw Stop hook, generic StopFailure appearance without request correlation, receipt retirement, process exit, operation Completed, NULL native turn ID, screen idle, hub messages, and model text are insufficient. No source means no invented success/failure.

Answered/cleared decisions, later response activity, stop, generation retirement, and Core restart suppress unsent stale session reminders. Claims verify current live observation; they never start or resume a session. A still-working or temporarily unpublished observation is not treated as idle. Disabled configuration or no matching subscription creates no notification observation rows and reads no APNs secret or connection.

## Exact tagged context and pagination

`NotificationContext` is `{action:"open_context",authorityId,dbId,crewId,kind,source}`. Source is one of:

- `{type:"workEvent",eventSeq,workId?,askId?,entryNo?,objectId}`
- `{type:"codexTurn",sessionId,threadId,turnId}`
- `{type:"managedResponse",sessionId,runtimeId,requestId,promptId,receiptRevision}`
- `{type:"decision",sessionId,generation,decisionId}`
- `{type:"message",messageId,messageSeq,channelId}`

Numbers on wire are canonical decimal strings; original typed Work/Ask/Entry IDs retain their prefixes. A managed receipt revision proves the original observation; dedup uses runtime/request/prompt, not changing revision or outcome label. Codex dedup uses original session/thread/turn. Decision dedup uses original generation/request UUID. Message dedup uses original ID/sequence/channel. Token/provider rotation does not replay an existing source. No first-subscription historical catch-up or scanner invents old events.

Clicking may only navigate through an already verified connection for the exact authority/database, recheck current grant and crew/object ACL, and read current state. It does not pair, connect, grant permission, execute, claim, start, restore, or resume anything. Missing/revoked/stale context yields neutral unavailable navigation. A cloud directory copy is not authorization.

## API3 typed methods

All use existing bounded RPC and CamelCase wire; no HTTP server or generic management RPC. Configuration is Local Human only under the existing managed-lineage floor. Installation operations require the captured actual mTLS Principal, current grant/revision/pin, and current crew permission, rechecked inside the same DB transaction. Agents, unauthenticated callers, sameUID-only callers, and another grant guessing an installation UUID are refused. OwnerWorkspace access retains its actual existing crew scope; no extra read/execute privilege is introduced.

1. `notification.status {}` -> `{available,enabled,reason?,revision,environment?,topic?}`. Available describes local configured credentials, not model eligibility or device delivery. Disabled/unconfigured is explicitly unavailable. No team/key ID/token/private material is returned.
2. `notification.configure {requestId,expectedRevision,enabled,provider?}` -> original request result. Provider is write-only `{teamId,keyId,environment,topic,privateKeyPkcs8Base64}`. It is redacted from Debug/log/receipt. Disabling requires no provider. A provider mutation uses a new protected key slot and replaces the client binding; old pending observations are suppressed rather than sent under another team/topic/environment.
3. `notification.installationRegister {requestId,installationId,expectedGeneration,deviceToken,environment,topic,crewIds,kinds}` -> original result containing installation summary. First generation is `0`, rotation CAS uses exact positive generation. Device token is bounded even-length hex, not permanently32 bytes. Environment is `sandbox|production`, topic/environment must match provider. Crew IDs unique1..64; kinds unique1..8. Per-device installation UUID belongs to its one current grant/pin. Token changes increment generation once. Revoked installation tombstones cannot be resurrected.
4. `notification.installationRevoke {requestId,installationId,expectedGeneration}` -> original result with revoked summary. Clears sealed token and suppresses pending. Grant revocation independently forbids later claims. It cannot withdraw an already possibly sent notification.
5. `notification.installationGet {installationId}` -> installation summary or JSON null. Summary is `{installationId,generation,revoked,environment,topic,crewIds,kinds}` and contains no token.
6. `notification.requestLookup {requestId}` -> `{kind:"notFound"}` / `{kind:"unknown",operation}` / `{kind:"known",operation,result}`. Result is limited to the original authentic owner; configure lookup is Local Human only. Original UUID and body digest bind each mutation. Response loss queries only this receipt; neither a fresh installation read nor reissuing another UUID proves the original outcome.
7. `notification.deliveryList {installationId,afterDeliveryId?,limit}` -> `{items,nextAfterDeliveryId?}`, limit1..100. Item is `{deliveryId,context,state,apnsId?,reason?,expiresAt}`. States are `pending|unknown|acceptedByApns|rejected|suppressed`. Cursor is deliveryId, never Work eventSeq. Expiry is original Unix milliseconds as a decimal string. Only fixed safe APNs reasons are disclosed, never raw response/error/header/token/URL/body.

Installation re-pairing is explicit new enrollment and installation, never copied trust from iCloud. Revoking notifications does not revoke the independent work grant; revoking the work grant always forbids notification delivery. No APNs token grants authority to the Core.

## Store19, secrets, and durable send boundary

Store19 is the necessary writer barrier for the new in-transaction outbox. An old18 writer could otherwise commit WorkBook changes without their required observation. Use the existing guarded current18→19 migration and original backups/rollback; existing15/17 paths reach18 then19. Experimental Native16 remains refused/inactive. This is not a historical import or parallel migration framework. Tests target fresh19/current18→19, accurate old18 refusal, and affected original backup/restore behavior.

Protected KeyStore uses original macOS Keychain policy or current-owner non-symlink0600 files under0700 on Unix. Provider key and a separate AES256GCM seal key use unique slots. DB token ciphertext AAD binds authority/database/grant/installation/generation/environment/topic. Never silently generate a replacement seal key for existing registrations. Key absence/corruption is explicit unavailable, not a retry or deletion of original work facts. Configure reserves its original UUID durably before protected external key writes; interrupted writes remain Unknown and do not run again through a receipt lookup. No new KMS/account service.

Matching registrations receive pending observations inside the accepted source transaction. Original transaction rollback leaves neither source nor its notification. Fixed expiry is original commit/enqueue time +86400000ms, stored once; restart/claim/token rotation cannot extend it. Expired pending observations are suppressed. Apple receives that exact nonzero expiry in Unix seconds.

One owned sender task uses the existing FIFO DB actor. Atomic claim is the in-flight authorization point: current configuration, identity, crew, installation generation/subscription, live original grant and live session source are checked; pending becomes durable Unknown with its one canonical UUID apns-id before HTTP I/O is polled. Unknown is never automatically selected again, including after crash, timeout, GOAWAY, response loss, rotation or another request UUID. Explicit rejected responses including429/5xx are also never automatically retried. Fresh subsequent source events remain independent.

Revocation suppresses pending and forbids later claims; already claimed Unknown/accepted may have reached Apple. A final pre-I/O check can suppress a known not-started attempt but cannot retract Apple acceptance. Late responses update only the matching original delivery/attempt, cannot reactivate an installation, and invalid-token errors revoke only the attempted generation. Restoring a DB uses the original staging trust-reset transaction to disable notifications, tombstone registrations/clear tokens, suppress pending and retain historical receipts/Unknown. It does not restore authorization or replay pre-restore external effects.

## Native transport and evidence boundary

Existing reqwest0.13.5/rustls with official http2 feature and ring P256 ES256; no custom HTTP2 stack. Minimal registry additions are h2.0.4.20 and fnv1.0.7, checksums recorded separately; no unrelated package upgrades. Feature unification adds HTTP2 availability to Jev but APNs owns a separate client and credentials; Jev configuration is unchanged.

APNs hosts are fixed by sandbox/production. TLS>=1.2 and actual HTTP2 required; local qualification verifies negotiated h2. Client has no proxy, redirect or retry, 3s connect/8s total limits, bounded body, sensitive Authorization header, alert push type, topic, original apns-id, original nonzero expiry, fixed priority5 and JSON content type. JWT caches by exact binding for30min; clock reversal/invalid material makes sending unavailable, no auth-error retry. Fixed generic PendingCrew alert and tagged context together fit4096 bytes.

HTTP200 with matching apns-id and expected body is acceptedByApns, not device display/read or business success. Known explicit APNs errors are rejected; malformed/oversized/mismatched-ID/transport responses remain Unknown. Invalid token reasons only invalidate the attempted registration generation. APNs has no general query proving an Unknown request never happened.

Source/local acceptance must prove same-transaction producers and rollback, exact types, permission/CAS/original receipts, secret redaction/AAD, stale source suppression, dedup/restart/rotation, Unknown-before-I/O, lost response, revocation/late response, restore behavior, actual local TLS+h2 request/response and malformed/failure bounds. It does not prove real credentials, signed entitlements, device registration, APNs delivery/display/tap, or production deployment. Those remain separately authorized actual-device gates.

Primary Apple documentation inspected2026-10-23: Establishing a connection to APNs; Establishing a token-based connection to APNs; Sending notification requests to APNs; Handling notification responses from APNs. Historical Cloudflare502 Unknown/deleted temporary Worker evidence remains unchanged and is not replayed or generalized into a transport limitation.
