# M3 submission/echo recovery independent acceptance — 2026-10-14

## Exact scope and source

Independent review of frozen production repair `53bccb58`, with the dedicated public-Core regression committed separately as `40254c7e`. Root owns the next exact-source Linux/macOS CI and main integration. No real Claude or Codex account, visible window operation, daily data, installation, or new artifact build occurred in this acceptance. The independent a9 fake window remains paused.

## Independently executed RED and GREEN

The initial real held-reader RED from the backend author (`pcw-ci-submit-echo-physical-red-01.log`) proved that an elapsed key gap released Backspace without post-write native output. QA then independently exercised the actual public Core seam with no private completion helper inventing output.

`/private/tmp/pcw-ci-submission-independent-red-01.log` exited 101: cursor-only native output falsely released Backspace, and absent echo failed to terminate within the original verification budget. The early/unfinished Enter plus stale empty-frame denial and fresh receipt positive control already passed. Exact failed source, product diff, HEAD and SHA256 remain `/private/tmp/pcw-ci-submission-independent-red-01`; the original input is retained rather than rewritten as the newer inverse-cursor fixture.

QA independently ran `/private/tmp/pcw-ci-submission-independent-green-02.log`: four tests passed, zero failed. Exact successful source/diff/hash remains `/private/tmp/pcw-ci-submission-independent-green-02`.

1. Cursor-only output and unchanged native input redraws cannot acknowledge Space or Backspace.
2. No key echo terminates honestly within the existing verification budget, with no submission/body retry; an obsolete completion token and late echo cannot revive it.
3. A queued Enter with no final completion cannot use an empty frame as Delivered. Completing Enter cannot reuse an older empty frame; a fresh post-completion empty frame still succeeds.
4. Default 80-column input at 77/97 body cells retains submission liveness when the inverse cursor crosses the final column or lies on a wrapped input row.

These four tests use public Core and explicit synthetic completion calls. They are not physical syscall or reader-consumption evidence. The complementary real driver evidence below supplies that layer.

## Whole-chain review and actual driver evidence

The frozen implementation reads native-painted inverse blank cursor cells only within the actual prompt/input region up to the lower border, or the owned visible body tail. Plain VT cursor position/visibility and unrelated inverse output are not consumption evidence. Input-region prefix and physical-cell cursor position must change exactly for Space and restore for Backspace. Flattened physical cells preserve the 80-column wrapping behavior; different body/caption identity does not qualify.

Follow-up independent review identified an unresolved boundary in this freeze: equality between collapsed caption line count and sanitized body line count does not prove complete native consumption. A last-line suffix or a single long line may still be in transit with the same count. Thus the four-case GREEN and 371-case gate above do not close opaque body-consumption correctness. Root requested an actual same-count partial-consumer counterexample; dedicated static tests are prepared separately while the running workspace source remains fixed. This section supersedes the earlier claim that matching caption count proves complete consumption. No paid-provider/version compatibility is inferred.

The final Enter completion flag is set only by the complete, currently valid delivery-write token. Partial/blocked or revoked writes cannot establish it. Any frame ingested before completion is invalidated as a submission receipt. Remaining SubmitBeat entries are revoked at terminal outcome; current authorization/dialog checks still run before physical writes.

The echo deadline starts from actual key-write completion and uses the unchanged verification window; unrelated output does not extend it. Its next wakeup is included explicitly, so a held SubmitBeat cannot hide the deadline. Existing dialog behavior retains the safe-key refusal path and submission-aborted transition; the original unanswered-dialog and after-answer recovery business assertions pass. No timing budget, max attempt count or test timeout was increased.

Evidence independently read, with execution attribution kept explicit:

- Backend `/private/tmp/pcw-ci-submit-echo-exec-full-02.log`: complete exec module, 371 passed/0 failed/3 existing live-provider tests ignored. All original 22 affected business cases are restored using explicit consumed-key frames. Two unknown/unstyled-input negative fixtures remain explicitly unstyled; their original deny assertions are retained. This gate includes 19 actual synthetic CLI workflows and QA's four cases.
- Backend `/private/tmp/pcw-ci-submit-native-noecho-physical-01.log`: actual held reader receives exact `BOT ` after default-budget expiry and late native echo, with no Backspace, Enter, body retry or cleanup bytes. Core time is injected and identified as such; PTY writes and final byte capture are real.
- The strong held-reader physical test rejects both cursor noise and unchanged native redraws, then captures exact `BOT \x7f\r` only after the proper native input changes. Original final-Enter WouldBlock/new-dialog cancellation capture and actor observer-before-broadcast checks still pass in the complete gate.
- Backend `/private/tmp/pcw-ci-huge-native-echo-01.log`: genuine slow-consumer RED exposed the partial collapsed-paste baseline. `/private/tmp/pcw-ci-huge-native-echo-02.log` and the complete exec gate pass the actual 68,000 UTF8-byte Chinese brief with delayed chunk reads. The complete submitted log is exactly one entry equal to the full original text (newlines escaped only for the log), alongside the driver Delivered receipt. Earlier failures are retained, not described as successful retries.

## Remaining delivery layers

The author is running the single workspace integration/lint gate after the freeze; root owns its actual results and the new real two-platform CI. The previously successful Linux `1b0c85fe` gate does not prove this new repair, and local macOS evidence does not replace Linux PTY behavior. No full-suite QA duplication or independent Cargo target was introduced. Visible-window clicks, real account operation, installation and daily-data switch remain separate acceptance layers.

## Follow-up same-count gate

Root raised the incomplete-last-line counterexample after the first four-case acceptance. The backend author agrees that a same-count collapsed view cannot certify full body consumption. The intended narrowed witness is a terminated multiline body whose maximum prefix count is N: only a consumed post-body Space can produce N+1, with the actual completed Body-before-Space FIFO write order; BS must restore N. Opaque single long lines or nonterminated final lines cannot gain confirmation from the same count plus cursor movement. A fixed delay is not evidence. This is still a proposal pending actual RED/GREEN, not an accepted capability or a final fix.

## Same-count counterexample: independently executed follow-up

`/private/tmp/pcw-ci-collapsed-samecount-independent-red-01.log` is QA's actual same-input RED (exit 101): the native same-total-count caption with a Space/cursor movement released Backspace while a single long line or its hidden final suffix could still be unconsumed. Exact source/hash remains in the same-name directory. QA then personally ran `/private/tmp/pcw-ci-collapsed-samecount-independent-green-02.log`, one case passed/four previous cases filtered; its new exact source/hash/diff is also preserved. Backend's `/private/tmp/pcw-ci-submission-samecount-green-01.log` separately passed all five dedicated cases.

The tightened current author input now binds plain paint to the complete sanitized body or a complete InputDraft that loses no leading/trailing data. A 64-character repeated tail is not full-body evidence. Opaque same-count caption plus Space/cursor movement cannot acknowledge Space. For the explicitly tested terminated-multiline synthetic codec only, N is an upper bound for every original body prefix; N+1 after the actual Body-before-Space FIFO write cannot be produced by a body prefix. Backspace must restore N. The Space probe is not itself a prior whole-body consumption receipt.

QA independently parsed the actual producer trace `/private/tmp/pcw-ci-native-prefix-witness-01.log`: 67 pre-Space consume events all had <=68000 bytes and <=2000 lines, followed by exactly one consumed Space at 68001/2001, BS at 68000/2000, and CR with submitted=true. Exactly one submit occurs. The compact independent parse report is `/private/tmp/pcw-ci-native-prefix-independent-review-01.json` and binds the source log hash. The actual complete-body exact-once assertion remains in the real CLI test; this log analysis does not replace it.

Remaining capability decision: this synthetic native caption codec has actual evidence, but an arbitrary real CLI version's opaque display semantics are unknown. Production must not infer a negotiated/verified codec from a caption string. Root/backend are deciding the default unknown capability gate. Until that policy is in the frozen source, the five-case GREEN is not a general opaque-body production acceptance.

The integration corrections are fixture-only: `a1a7b7a8` converts the older CLI prompt-file/stdin reader from kernel canonical echo to actual raw/no-echo native drawing after real reads. QA read its source and `/private/tmp/pcw-ci-submit-cli-glue-02.log` (five passed): complete bytes are written to got.txt only on received CR, argv is independently recorded, failure exits after five seconds; original file/stdin and no-body-in-argv assertions remain. Four existing real S42 task fixtures likewise require actual complete multi-line native body frames with consumed-key echo rather than unsupported opaque captions; `/private/tmp/pcw-ci-submit-s42-02.log` passed all four with the original observer, generation, receipt, and CAS assertions intact. The first workspace failures remain preserved. The final workspace, lint and exact-source remote CI still belong to their separate gates.

## Final runtime capability gate — frozen production bcb4c293

Root's capability decision is now implemented: CoreConfig.verified_collapsed_paste_codec defaults to false. QA's exact-source search finds no environment/CLI/RPC/Serde mapping, and screen contents cannot turn it on. Explicit true assignments are confined to declared synthetic test setup. Thus production does not use an unknown real CLI version's folded display as a negotiated content-confirmation channel.

On default unknown collapsed input the runtime reports Unsubmitted before emitting Space, holds Foreign::Conflicted, and protects the original unknown input from the next queued job. Neither late N+1 paint nor a later Empty display can authorize replacement. An explicit authorized raw clear followed by fresh Empty may resume the next job. The public Core test identifies that raw clear as a synthetic action, not new proof of RPC identity permissions.

The fifth test now explicitly enables the trusted synthetic codec, so its rejection of unsupported same-count/nonterminated opaque paint cannot be masked by the default-false gate. Backend executed all six dedicated cases successfully in `/private/tmp/pcw-ci-native-codec-independent-01.log`; QA personally executed the earlier first-four and same-count RED/GREEN and independently read the final six-case source/results. No redundant Cargo run was added merely to relabel execution attribution. `/private/tmp/pcw-ci-native-codec-lib-01.log` independently records the backend's 297 original/core/physical tests passed with zero failures.

This closes the reviewed default capability/receipt contract and explicitly preserves the supported synthetic codec positive proof. It does not close live-provider confirmation for opaque display modes: M4's actual trusted content witness/version verification remains required by the full delivery scope. The final all-workspace/lint and root's new actual two-platform CI still remain separate gates. These new results supersede the earlier pending-capability paragraph while retaining all original failure evidence and partial acceptance history.
