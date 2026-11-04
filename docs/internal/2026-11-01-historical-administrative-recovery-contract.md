# Historical escalation recovery

Baseline: `0d3df835`. This is the next implementation contract, not acceptance.

## Evidence and product result

The old escalation implementation (`0ea01f92^`, also the original `4f9943df`) wrote a System announcement whose body contains a session hint and at most 50 `{id,kind}` pairs, plus total/partial. It did not persist a source-session foreign key, target revisions, a complete target inventory, budget epoch or restore epoch. The current per-session budget is mutable; earlier migration merged counters and used epoch zero. Old reminder notes have complete targets/revisions but no immutable announcement/epoch relationship, and delivered reminders may be pruned. Category alone is also insufficient: work_stop_escalation is not a reserved post category.

Therefore the module must preserve and expose old history, and provide a real path from that history to current handling. It must neither fabricate the original snapshot nor stop at an unusable inventory. A Human explicitly selects a real current source member, reviews current complete evidence, and confirms a new current revalidation. That new source enters the existing judgment, automatic execution, effects, outcome, custody and retirement pipeline. Old model decisions, admissions, grants, delivery receipts and unknown dispatches are never imported as authority.

## Historical discovery

Scan original messages in bounded seq pages, capturing a frontier and keeping cursor/result updates transactional. Preserve original messages and anchor each discovered record by message ID, seq and immutable content/identity digest. Validate actual System/channel/mention structure; body parsing may supply hints only. A Human/session post using the same category cannot become a trusted escalation. Malformed or missing evidence is visible and isolated per record, without erasing raw data or blocking healthy later pages.

Modern typed cases are exactly deduplicated by source announcement ID. Build an announcement index in bounded case pages before treating an absent mapping as legacy. New case registration updates that index in its own transaction. Missing/corrupt indexes must be distinguishable from genuine absence using independent completeness evidence. Identical body text on different message IDs is not an idempotency key. Restoring the database does not turn the historical anchor into a new dispatch.

## Current Human revalidation

Human-only list/get/preview/confirm/receipt methods are authenticated at entry and inside DB reads/mutations. Attached agents cannot borrow raw Human methods; paired Human grants are rechecked. Historical listing may be global because old records have no reliable crew attribution. A confirmation explicitly names a current crew and a real member of that crew. The UI labels the global historical list and the current-crew revalidation separately; no global Admin/session access is introduced.

Preview is pure and includes the historical limitations separately from a complete current source snapshot: actual member/crew, current full stop-violation set and revisions, actual escalated budget, Stop/configuration/restore versions and source/target state. Do not pair the latest old wake with an announcement or require a reminder that may legally have been pruned. A mismatch between the full current violation set and the relevant current budget is a visible block, not a truncated grant. Out-of-scope targets remain in the source and do not gain in-scope write authority. An empty or incomplete current issue cannot produce an executable case.

Confirmation requires a request UUID, exact preview revision and a nonblank explanation. Recheck original anchor digest and all current facts in the actual transaction. Record an idempotent Human receipt and create/link the source atomically; changed intent on the same UUID conflicts. A lost reply is Unknown and queried by its original UUID, never automatically resubmitted. Preview or confirmation must not reset the stop budget, synthesize three reminders or increment a migrated epoch to make it look native.

The case has explicit HumanRevalidation provenance with the historical message digest, current evidence revision, request identity, explanation and current observation time. Provenance participates in source integrity and judgment authority. Legacy sources without this optional field preserve their original serialization/hash. Only the explicit new Human-revalidation contract may represent a real epoch-zero migrated budget; NativeStop retains its existing epoch requirement. Current budget/source identity remains unique, including races with ordinary new escalation: link an existing case instead of creating a parallel one. A historical association does not overwrite an existing execution/Unknown/retirement result.

## Acceptance and ownership

Backend/proto/client have one writer; Runtime/RPC have one writer; independent tests have one writer; root integrates and verifies schema/build/commit. Preserve the five unrelated untracked design files.

The primary independent case uses a real old announcement for 55 targets containing only 50 summary items. Discovery creates no execution grant. A Human preview covers the complete current 55-target set; confirmation creates one clearly current source and natural production judgment/automatic dispatch handles it. Verify lawful effects and current outcome, not just a suggestion. Also cover modern-source deduplication, forged category/source hints, identical text/different messages, concurrent confirmations, missing index evidence, rollback, Work/Stop/config/restore/crew changes after preview, out-of-scope targets, actual file-database reopen, Unknown no replay and pure GET/preview.

Record normal and fault evidence, production-native versus StandIn boundaries, client fixtures and full Mac compilation separately. This source module does not claim daily-data migration, real-account acceptance, installation, GUI acceptance or publication.

## Administrative automation sequence after the 2026-11-04 decision

1. Finish same-crew recovery and case handling. A historical announcement is evidence only. A Human confirms complete current facts, the existing judgment and Core decide whether to admit the restricted executor, and actual effects or an accepted captain handoff discharge the obligation. The process exit remains separately verified. Exercise missing or damaged witnesses, revoked authority, an unknown response, and restart before accepting the module.
2. Present an evidence-backed secretary brief from the existing patrol run. Show its time, scope, completion and limits, with references to the original findings. Keep work, original requests and channel observations distinguishable. Use the same patrol record for both the scan and the brief.
3. Add semantic discovery in separate reviewable units. Start with suggestions linking similar open original requirements within the same crew. Keep the original requirements and their work links. Capture both IDs and revisions, the decision and its evidence. Require explicit authority for any later correction. Extend archive discovery separately so the existing factual archive and closure gates can remain authoritative.
4. Establish real configured-provider and same-source client/Core acceptance. Keep failure, abstention, delayed results, restart and physical process exit distinct in the acceptance record. Offline-source authentication, restricted Claude execution and any multi-crew grants have separate authority and lifecycle requirements.

For uncertain historical confirmation, the Mac client uses its existing authority-bound presentation journal to persist the original request before submission. Reopening the same journal restores the original UUID for receipt lookup. The receipt must match the original evidence and confirmation before the journal records reconciliation.

### 2026-11-04 integration evidence

- The source scan checks the independent case sequence before completing its index. A missing alias blocks confirmation. Repair is retried one source per tick. Explicit confirmation before a discovery tick records the same discovery witness atomically. A damaged original keeps verified saved history with an invalid status. Recent ordinary chat and DM IDs return not found, and forged category rows cannot disclose their text.
- The isolated DAO and runtime runs passed 73 administrative collaboration cases and 109 judgment cases. The full runtime first exposed a test race between the periodic and explicit decision callers. The fixture now waits for the original durable decision. No production deadline or policy changed. Three actual Unix socket administrative cases, one paired TLS workflow and the generated schema parity check passed.
- The native replay-free fixture used a real Codex process with synthetic credentials and a loopback provider. It recovered 55 current targets from an old 50-item announcement, verified one actual work closure and observed the restricted process exit after source revocation. This run verifies one native effect. The separate isolated runtime case verifies eventual outcome with all targets discharged.
- The Mac workspace tests passed, including original request restoration and the new factual brief projection. The unsigned Mac target built using the already installed Core solely as compiler input. This verifies compilation of the two client surfaces. Same-source packaging, installation, GUI use and configured external provider acceptance are separate gates.
