# Native APNs App integration — complete source module

This is the normal Mac/iPhone/iPad consumer, based on accepted Cloud CDF and accepted APNs backend `04ee2af2d5d39576b455d03146eba7529f5e3aad`. It does not change Core, schema, transport trust, existing work views or installed products.

## Normal App entries and ownership

`PendingCrewApp` installs `CrewNotificationEntry` on the actual `RootView`, shares the existing AppModel/workspace, and connects the existing Mac application delegate plus the iOS application delegate. The notification settings entry opens status, explicit subscriptions, original registration recovery and delivery records. The original shell, footer, host dot, crew tabs, drafts and quit rules remain in place. There is no new global status banner or alternate workspace.

`CrewdWorkspace.notifications` only exposes the feature after the original journal is bound to the current authenticated authority/db/grant/revision, the client handshake is ready and hello advertises `notification.nativeApns`. A Local Human Unix connection may read provider status but is never a recipient mTLS grant. The App does not expose `notification.configure` or provider p8 input; provider configuration stays protected on the server.

Opt-in and selected subscriptions use a sorted, stable encoding of that exact original scope. A new/revised grant cannot inherit consent. Registration identity comes from the private original journal, never a list position or timestamp. Multiple prior installation IDs require explicit selection and `installationGet`; current generation is read before token-rotation CAS. Device tokens remain memory/RPC-only: durable prepared observations retain only a digest and original UUID/CAS/subscription metadata. Tokens and p8 are absent from UI, errors, drafts and cloud payloads.

## Lifecycle and original outcomes

Default `CrewNativeNotifications.qualification == nil` prevents notification-center construction, permission queries/requests and OS registration. The UI states that this version is not enabled. No signed-aps capability or Apple availability is inferred from an existing certificate or a paired record.

When a later qualified signed source enables this boundary, only an explicit user confirmation may request system authorization. An already-consented startup/foreground path observes the actual handshake/journal/grant and checks existing permission without prompting again. No path chooses or connects a computer or starts/resumes a session. A single owned callback sequence processes changing tokens with the current installation generation; duplicate tokens are ignored in memory. Original connection/operation ownership is checked after permission/network awaits. Cancelling registration waiting does not erase a prepared network request.

Register/revoke prepare the existing private journal before dispatch and strictly validate the actual backend `NotificationRequestLookup::Known {kind,operation,result}` envelope and original installation receipt before finishing it. Lookup uses the same validator. Bare summaries, wrong operation/kind or mismatched result identities cannot finish a mutation. Malformed/lost replies stay durable Unknown. Reopen blocks new registration/revocation until original request lookup is known. `notFound` and `unknown` never authorize resend. Known lookup must match the original operation, installation, next generation and registered subscription metadata. Local opt-out is durable even when revocation is Unknown; Apple may already have accepted a prior delivery.

## Read-only tap chain

The delegate accepts only the default action and a bounded exact `pendingCrew` envelope. It does not execute APNs text or open a URL. Without the currently selected verified original authority/db/grant, the result is unavailable, with no automatic connection.

Every tap performs original `installationGet` (not revoked), exact `deliveryGet` and full context equality, then current crew/object ACL reads. Work events reuse public W/H identifiers, never internal work/ask IDs. Messages use actual `channel.list {crewId}` plus exact message id/sequence/channel. Session sources use `session.get` to prove crew ownership before `session.inspect`. All awaits recheck the same connection/observation/grant. The resulting sheet is read-only; its native workspace navigation is guarded by the same origin again. It neither runs generated pages nor approves/answers/starts/resumes work.

## Verification and delivery boundary

The original source packet contains twenty Store/Journal/Wire/default-off modes, including persisted consent/subscriptions, token/CAS rotation, Unknown reopen and lookup, malformed replies, grant/connection changes, multiple-installation selection, revocation Unknown, and exact work/message/session tap reads. The Store's initial unsorted preference encoding failed the actual consent seam; the original source/bin/log remain preserved, and the sorted-key fix passes the same fixture.

Independent review caught that the original synthetic mutation route returned bare summaries instead of the server Known envelope. That original twenty-mode result did not establish successful mutation wire compatibility. A corrected fixture first fails against the frozen original source, then verifies actual Known register/rotation/revoke success and rejects bare, wrong operation, unknown/notFound and wrong-result replies while recovering by the original UUID. The delta packet preserves this correction, exact red/green sources and logs.

Both complete ordinary Debug targets for the original shared UI were compiled serially using the existing aeh/SPM cache: Mac `LOCAL_MAC + REMOTE_PRODUCT`, and arm64 iOS Simulator. Frozen before/after input hashes identify that target source; the replacement delta identifies the two changed non-UI Swift files and compiles their actual Store/Wire/Journal seam. It does not relabel the original whole-target logs as the replacement source. The historical S5553 helper was only a metadata build input, not a new APNs normal or distribution artifact. Existing Swift-6 migration warnings outside this module remain distinct from successful current target builds.

No new normal/candidate/signing, installation, GUI, Cloud SDK exchange, system notification permission query/request, OS token registration, APNs delivery, Keychain operation, actual provider or model was executed. Actual visual entry reachability and platform callback delivery remain runtime acceptance, not synthetic-test claims.

## Next real Apple qualification action (not executed)

Use Team `M42BKJN82S` and the existing standard `com.pendingname.pendingcrew` identities. Existing profile metadata alone does not establish push entitlement. Root should approve one precise combined qualification window: inspect the actual AppID push capability and permitted distribution/development environment, obtain/choose matching profiles if necessary, sign the complete combined source with the correct per-platform entitlements, and verify actual signed topic/environment before enabling the source qualification. Mac and iOS profiles are not interchangeable. Provider p8 remains server-side protected config.

Then explicitly opt in on the qualified device, register only against a real current paired mTLS grant, verify original receipt/token rotation/revocation, and test a generic push against exact current delivery/context plus a denied/revoked case. System permission/login/Keychain prompts belong to the human. Until that window the product remains honestly unavailable for native APNs, default off; the local source/compile results do not claim Apple registration or delivery.
