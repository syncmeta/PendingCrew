# Jev public contract 02 — interface snapshot, not module acceptance

User approved the first TypeSafe single-provider judgment module on 2026-10-15. This interface snapshot is for the original frontend settings adapter. Product/runtime/source review and whole-module acceptance remain separate. No new schema/DDL, Native15 activation, OAuth, other vendor adapter, model request or sidebar ordering is introduced.

## Methods and authority

* `judgment.settings.get`: `null`, `{}`, or `{requestId:UUID}`. Pure read; returns `JudgmentSettingsResponse` below.
* `judgment.settings.set`: `JudgmentSettingsSet`. Returns `JudgmentSettingsResult`.
* `judgment.inspect`: `{crewId,kind:requirement|message,id}`. Returns `{source,record:null|JudgmentView}`.

Settings get/set require the existing genuine local Human anchor and the complete live `local.clientTrust` authority checks. Being a code-admitted frontend, same UID, a claimed JSON Human, or an owned-child label does not grant this permission. Remote/agent/unknown callers are refused. Ordinary Human reconnect and direct owned-child Quit are distinct authorities. The backend does not invent another direct-child requirement for settings. Inspection requires current genuine Human principal and the original crew/source ACL; a remote principal must also retain its real paired grant/method permission. Attached agents cannot use these methods.

## Settings shapes

`JudgmentConfig`: enabled(bool), provider(string, currently only `typesafe`), baseUrl(string), model(string), choiceMin(number), wakeYes(number), credentialSource(`platform` or `file`, default platform). The existing thresholds default in the judgment factory to 0.5; a configured DTO supplies them explicitly. Values outside [0,1] and unavailable providers are rejected. The new production settings and runtime factory require HTTPS, reject URL credentials/query/fragment, and preserve normalized endpoint path distinctions. The old client library retains its pre-existing loopback fake HTTP support; only an explicit private cfg(test) injection can enable that transport in this new runtime. There is no wire/environment switch to relax production HTTPS. Redirects are not followed. Model is bounded to 256 bytes and endpoint to 2048.

`JudgmentSettings`: revision(string|null), config(JudgmentConfig|null). A missing config is unconfigured: deterministic existing rules/captain, no external request. Disabled config does not call a provider.

`JudgmentSettingsView`: settings, credentialState(`missing`|`recorded_unverified`|`update_unknown`|`unknown`), pendingRequestId(string|null). Get reads recorded state only; it does not retrieve a secret, validate it or contact a provider. `recorded_unverified` is not evidence of availability or valid billing/account access.

`JudgmentSettingsSet`: requestId(UUID string), expectedRevision(string|null), config(JudgmentConfig|null), credentialUpdate. `credentialUpdate` is `{action:"keep"}` (default), `{action:"set",value:"..."}`, or `{action:"clear"}`. Set values occur only in the request, never in settings/get/result/inspection/log output. Key input is bounded to 8192 bytes and uses the existing key syntax. Server Debug formatting redacts it.

Keep requires the original recorded provider/normalized full endpoint/credentialSource/database binding. Changing provider, endpoint path/origin or credentialSource rejects Keep; choose a new explicit Set/Clear. Disabling without secret removal can use `enabled:false` with the same config/binding. Clearing with config null disables and removes the active binding. Platform uses scoped macOS Keychain slots; file uses scoped private 0700/0600 storage. Existing unrelated/default CLI credentials are not silently adopted into an unknown endpoint binding.

`JudgmentSettingsResult`: requestId, historical(bool), outcome(`committed`|`unknown`), view. Committed proves the SQL configuration/binding transaction and the specifically acknowledged KeyStore operation, not API validity or model execution. After official restore/import a stored result from another database identity or restored-context epoch is explicitly historical and must not be applied as a current configuration result.

## Pure request lookup (02)

`JudgmentSettingsGet` has only optional `requestId`; null/{} is compatible. The response is `{view:JudgmentSettingsView, requestLookup:null|Lookup}`. With no ID, lookup is null. With a valid UUID it is exactly one of `{status:"known",result:JudgmentSettingsResult}`, `{status:"not_found"}`, or `{status:"historical"}`. Only known carries an original result; current view is always a separate field. Historical never promotes an old result into current settings. Unknown/missing/rollback/late external completion cannot be disproved by not_found. None of these reads creates a barrier, updates configuration, retrieves a secret, calls KeyStore or submits a provider request.

The frontend persists the request UUID, original non-secret intent/scope and Unknown presentation state, **never the credential value**. After disconnect/restart it calls this pure get(query) path. It must not reconstruct/re-send settings.set to learn status and must not auto-generate a replacement mutation for not_found. Only a new explicit Human action can supply a new secret and request ID. Original known-ID set readback remains defensive idempotence, not the UI's query API.

## External secret mutation and recovery

The server durably records a pending barrier and original request result before a KeyStore operation, then commits config/binding and clears the barrier only after acknowledgment. A timeout, interruption, write failure or SQL failure keeps Unknown. No automatic retry, fallback secret backend or repeated KeyStore operation occurs. New explicit Human recovery uses a fresh requestId and Set/Clear; Keep is refused while Unknown remains. Each Set uses a distinct slot so a late unknown operation cannot overwrite a newer active key.

A repeated known requestId is **readback of the original result**, not a new mutation; changed intent always needs a fresh UUID. It never applies edited fields under that old ID. An unknown/missing reply must not cause the frontend to generate a replacement request automatically. A request not known to the server may still be a first submission; retain the original intent journal and Human confirmation. No request body or secret hash is persisted in ordinary metadata. Clear concerns the active recorded credential; it is not a claim that an earlier unknown, unbound or legacy KeyStore item has physically disappeared.

## Closed inspection

Record fields: decisionId, phase(`pending`|`dispatched`|`settled`|`suppressed`|`historical`), reason(closed enum in DTO), backend(`typesafe`), model(string|null), probabilitySource(`native`|`generated_estimate`|null), calibrated(false), attempts(number|null), latencyMs(number|null), usage(null|{inputTokens:number|null,outputTokens:number|null}), recipients(session IDs), attribution(null|{kind:`requirement`|`work`|`message`,id,revision}).

No provider body, raw error, full prompt, captured floors, DB identity or secret is returned. Unknown attempt count stays null; it does not mean zero requests. A historical/suppressed record does not authorize re-dispatch. Native probability at the task threshold may be adopted after same-crew/visibility/semantic-version/Stop revalidation; generated estimates are suggestions only. No other provider is implemented in this round.

Routing durably relates an immutable Requirement to a recipient and emits a distinct judgment-route wake; it does not create a Work, owner, Plan or acceptance rules. Message attribution is an independent link and never edits reply/mentions or answers an Ask. SQL routing/attribution settlement is not runner delivery acknowledgment. The existing Core and actual executor delivery event remain the sole execution authority.

## Frontend behavior

Use one global manually selected configuration, preserve the original authority namespace and request UUID in its durable Unknown presentation journal, show recorded/unverified status, and never print a secret in an error/result. Setting an unconfigured service is not a successful provider test. A code-admitted App without the old anchor can read ordinary business scope as authorized but cannot self-upgrade into settings secret administration. The original frontend must implement this entry before user-facing configuration is considered delivered. This interface snapshot alone is not UI or full-module completion.

## Version and evidence boundary

02 supersedes immutable01 for client implementation; 01 is retained as history. Restore epoch rotation occurs only in existing official restore/import transactions, without DDL or a second effects authority. Ordinary same-DB reopen preserves context. Unknown raw filesystem rollback cannot be observed reliably and does not gain an exactly-once guarantee. This is the frozen interface-only snapshot; whole production business/fake recovery checks and client UI delivery remain independent.
