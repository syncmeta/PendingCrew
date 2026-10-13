# M6 trusted remote pairing and shared workspace contract

Status: complete implementation contract proposed for one boundary review; no runtime capability is advertised by this document. Client owner implements the remote/store/proto/server adapters in the isolated M6 tree. Existing Unix peer/process qualification and M4 executor/terminal/native helper remain with the backend owner. Original PendingCrew Mac and iOS target build gates are recorded separately; those do not prove remote or device readiness.

## Identity and transport

Production transport is the same bounded NDJSON request/reply/events API over TLS **1.3 only**, requiring a certificate from both parties. Each party pins SHA-256 of the complete DER SubjectPublicKeyInfo, including the algorithm identifier, not raw EC bytes, certificate bytes, hostname, a user-supplied device ID or an old PSK. TLS CertificateVerify signatures must be checked by the library in addition to comparing pins. TLS 1.2, unauthenticated clients, early application data and old PSK fallback are rejected.

The server has an independently generated authority UUID, certificate and private key; the work database dbId remains a different identifier. A pairing record binds both the server authority and work dbId. The client keeps its certificate/private key locally and stores a grant record under an immutable local authority-record UUID. Changing an address never changes that UUID or grants trust; changing the server key or database authority requires a new confirmed pairing.

Server private key material lives in a mode-0600 identity file under a mode-0700 remote directory; public identity binding and grants are in the schema-11 store. Missing, mismatched or malformed existing identity material fails closed, rather than silently generating a replacement under old grants. Copying dbId alone cannot establish a TLS identity. The client uses a separate new Keychain service and new record format, leaving legacy records intact but ineligible for M6. Work restore/import must not silently enable grants for a mismatched remote identity.

The server records a fresh runtime Core-binding UUID on each daemon generation. Only the authenticated transport adapter constructs a remote principal from the accepted certificate pin, durable grant and actual AppState/Core binding. JSON params cannot provide a principal, source Core or Human identity. A hello reports the server authority UUID, work dbId, runtime binding UUID and actual remote capability; the client verifies the first two against the confirmed record before loading work.

Listener configuration is explicitly local-Human-controlled. Default is disabled. Tests bind only loopback with isolated data. No new LAN listener or change to the user's daily configuration is performed during engineering acceptance. Disabling or changing the listener closes old connection generations and never replays writes.

## Pairing interaction (no unpaired network RPC)

The workflow is local explicit out-of-band exchange, usable via copy/paste and later QR without changing authority semantics:

1. On the work computer, the actual qualified local Human creates a short-lived invitation. It contains the server certificate/pin, server authority UUID, dbId, endpoint and a cryptographically random one-use challenge. It expires after five minutes. It is not a grant and cannot call work methods.
2. The phone/iPad imports it, shows the computer label/endpoint and fingerprint for explicit confirmation, then creates its own certificate identity. It returns a signed response containing the original invitation ID/challenge, its certificate/pin and a human-entered device label. The signature proves possession of its private key; it does not by itself authorize the device.
3. The computer shows that device and fingerprint. A local Human explicitly accepts the exact response. One transaction validates the invitation, signature, expiry and unchanged server/database identity, consumes the invitation and creates the persistent grant. The immutable accepted response witness is kept for safe request-UUID confirmation, independent of later label edits or revocation.
4. The computer returns a signed grant receipt bound to both certificates, identities and the grant UUID. The phone verifies it against the originally confirmed invitation and its own private-key identity before storing the authority record. A receipt for another certificate, database or invitation is rejected. Importing the same receipt never clears a newer draft or silently re-enables a locally forgotten record.

Wire envelopes carry a version, bounded base64 JSON payload and base64 signature. Signatures cover the exact decoded payload bytes, so client and server do not independently invent JSON canonicalization. Payloads have bounded certificate/label/endpoint fields and reject unsupported versions. No private key is present in an invitation, response, receipt, ordinary RPC result, event, log or model prompt.

## Methods and persisted facts

New local-only methods are:

- `remote.status {}` -> enabled/listening state, finite actual endpoint, public identity and finite grant summaries. Never private keys.
- `remote.configure {requestId,expectedRevision,enabled,listenHost,port}` -> current configuration; UUID immutable-request witness, CAS configuration revision, real bind failure surfaced. Changing the bind address does not modify trust.
- `remote.invite {requestId,deviceLabel?}` -> signed invitation and expiry. UUID confirmation returns the original witness, not a freshly extended invitation.
- `remote.accept {requestId,responseEnvelope}` -> signed grant receipt and finite grant summary; immutable create witness, never un-revoke on retry.
- `remote.grants {}` -> finite summaries for this server identity.
- `remote.revoke {grantId,expectedRevision,note}` -> permanent tombstone, new revision and current active state. Required nonempty human reason; a repeated old receipt does not restore a revoked grant.

The initial personal-device grant is an explicit owner-workspace grant: it permits the same actual shared work/channel/document/folder operations the local Human uses. It is not an agent attach token and cannot impersonate a session or another actor. Pairing, listener configuration, grant management, local daemon process management and arbitrary agent.* RPC stay local-only. A finite allow-list is explicit; unknown future methods/topics are rejected until intentionally included. Normal domain crew/channel visibility rules remain in the authoritative stores.

Remote-specific hello facts extend the existing hello additively and are not accepted from request payloads. Grant revision, state, accepted pin, associated authority/dbId and runtime principal are checked on **each request admission**; handshake success is not permanent Human permission. Read results and event delivery also check that the principal is still live. Grant IDs and labels cannot substitute for the transport certificate.

Schema 11 stores remote public identity binding, configuration revision, invitations with consumed/expired state, immutable request witnesses, grant revision/tombstones and bounded audit events. Activation uses the existing actual Store::open upgrade backup/rollback protection. No existing event, Plan revision, H/W/# ID, work status or owner history is rewritten by pairing.

## Online revocation and lifecycle boundary

Revocation commits the durable tombstone before reporting success, signals every connection admitted under that grant, and prevents all later request admissions, subscriptions and event writes. Reconnect and TLS session resumption must consult current grants; an old session ticket cannot restore permission. A request already admitted before the revoke boundary may finish its existing owned operation; closing its socket does not imply rollback or authorize automatic retry. Its missing receipt remains an unknown submission in the original client scope. This admission boundary must be independently demonstrated, including queued later requests and streaming events; it is not enough to prove only a future handshake fails.

The remote path reuses the existing owned request/draining behavior and bounds. It must not weaken kernel Unix peer qualification, bypass process enrollment, duplicate the work dispatcher, or leave detached readers/writers/subscriptions after Core replacement. Switching authority retires the previous client before any new handshake and keeps its unknown inputs for inspection, while fresh caches/drafts stay under the new immutable record.

## Verification and ownership

Independent RED/GREEN checks cover wrong pin, no client certificate, TLS1.2, forged/expired/consumed invitation, wrong receipt owner, immutable request retry after revoke, restart-durable tombstone, same dbId/wrong identity, source Core replacement, in-flight admission boundary, live event/queued-request revoke, and unchanged local Unix qualification. Real TLS loopback uses actual certificates and the production listener and NDJSON dispatcher, with no model or daily data. Client checks exercise Network.framework TLS against that real listener, shared phone forms and iPad window/authority isolation. Synthetic wire controls cannot substitute for TLS.

Then build the actual original target, followed by explicit phone and iPad device acceptance (pair, use four views, lose/reconnect, revoke, switch authority, preserve uncertain input). Separate those from main integration, signed package, installation and release.

Implementation dependencies use audited libraries rather than custom cryptographic verification: rustls/tokio-rustls (already in the Rust lock/cache), an X.509 certificate/SPKI library for certificate generation/decoding, and Apple's supported local-identity/verify-block APIs. Any new library/toolchain input is frozen in the actual build manifest. TLS signature callbacks must delegate real verification; a pin comparison must not return a fake signature success.

Official implementation references: [Apple local-network TLS identity](https://developer.apple.com/documentation/network/creating-an-identity-for-local-network-tls), [rustls client certificate verifier](https://docs.rs/rustls/0.23.45/rustls/server/danger/trait.ClientCertVerifier.html), [rustls server certificate verifier](https://docs.rs/rustls/0.23.45/rustls/client/danger/trait.ServerCertVerifier.html), [rcgen certificate generation](https://docs.rs/rcgen/0.14.10/rcgen/struct.CertificateParams.html). These are API references, not claims that PendingCrew's runtime is implemented or accepted.
