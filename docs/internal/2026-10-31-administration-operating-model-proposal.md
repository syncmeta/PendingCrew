# Administration operating model proposal

Status: researched operating-model recommendation, not a grant of cross-crew authority. The user asked how real enterprises organize administration rather than choosing the earlier binary scope question. Existing same-crew Admin permissions remain the implemented baseline; implementation of bounded lifecycle modules continues without waiting for another scope questionnaire.

## Evidence and interpretation

GitLab's People Operations is a centralized support service, while its People Business Partners work with business groups. Its DRI policy preserves a clearly accountable decision owner for projects. CIPD describes centralized shared services supporting business partners and expertise functions. APQC describes a hybrid model with global governance and regional/local delivery. These are examples and operating principles, not proof that every company has the same organisation or that HR alone represents all corporate administration.

Sources consulted:
- https://handbook.gitlab.com/handbook/people-group/people-operations/
- https://handbook.gitlab.com/job-description-library/people-group/people-business-partner/
- https://handbook.gitlab.com/handbook/people-group/directly-responsible-individuals/
- https://www.cipd.org/uk/knowledge/factsheets/shared-services-factsheet/
- https://www.apqc.org/What-Mix-of-In-Country-Regional-and-Global-Headcount-Do-Shared-Services-Organizations-Use

PendingCrew's proposed administration mixes operational follow-up, records, staffing and executive assistance. It should borrow the division of responsibility rather than copy a physical office administration department or a payroll organisation.

## Proposed PendingCrew design

One logical organisation-wide administrative service owns common rules, finding deduplication, assignment of administrative cases and the user's consolidated brief. Actual work is performed by temporary sessions with explicit case scopes. A service-wide remit does not give each session unrestricted access or write authority. Cases name crews/objects, permitted action types, fact versions, expiry and budget. Authority comes from deterministic runtime policy, never from a model's prompt or self-declared role. The independent management crew is a hosting location, not a newly invented root of the business graph.

Business captains retain goal selection, priority, acceptance standards, normal assignment and business cancellation. Administration can investigate missing ownership and stale records, gather evidence, correct factual links, perform explicitly authorized factual closure/archival, and coordinate escalation. Disagreement about business value returns to the captain; cross-business resource conflicts or new commitments return to the human. A recorded model suspicion, notification ACK or administrative session exit is not a resolved case.

Examples:
- A worker misses checkpoints: preserve the existing verified-loss and reassignment path, record actual ownership transfer and notification receipts; do not create duplicate tasks.
- Similar work exists in two crews: a bounded cross-crew case may inspect the authorized objects and propose a correction. It cannot cancel either business objective merely because a model says they look similar.
- An idle channel has completed work: inspect current activity, pending inputs and authoritative links, then use an explicitly scoped archive capability. Archival must be traceable and reversible.

Start with one logical administrative service and bounded per-case sessions. Do not instantiate a permanent administrative department for every top-level crew merely because the hierarchy exists. Regional/subtree ownership can be added when independent workloads or policies justify it; case scope and responsibility remain explicit in either arrangement.

## Implementation boundary and sequence

This proposal recommends central governance and case-scoped execution. The earlier top-level-subtree versus entire-organisation dichotomy was too coarse: organisational remit, data visibility and action authority are separate decisions. No global scope, tool privilege or background administrative activation is granted by this document.

Build the lifecycle against the existing same-crew boundary first: persist the source and judgment, bind one executor, enforce case-local tools and native execution policy, verify business effects, explicitly hand unresolved responsibility to its accountable captain, and retire only after a real exit receipt. Several of these modules now exist; their current evidence and remaining gaps belong to the decision implementation ledger. Native authentication continuity, authority invalidation fencing, exceptional custody recovery and older escalation import remain implementation work rather than reasons to reopen the organisational scope questionnaire.

Once this lifecycle is sound, extend the common administrative service with explicit multi-crew case grants and consolidated reporting. That extension must define both visibility and permitted actions. It cannot obtain authority merely from the service's organisational remit or from a model assigning itself an administrative title.

## 2026-11-01 implementation checkpoint

The same-crew lifecycle now includes exceptional stale-custody succession and a captured native-source authentication bridge, in addition to verified effects, explicit handoff, real retirement and authority fencing. Their actual tests and limitations are recorded in the decision implementation ledger. The next integration is controlled automatic execution of new complete typed escalations; the current production judgment still only persists its result. Old escalation import and wider organisational grants remain independent. A logical central administrative service is the recommended organisational design; it is not an implicit global capability for an individual session.
