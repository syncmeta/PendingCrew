# M2 documents, view folders and channel work references

This checkpoint implements the full Rust module on the isolated candidate branch. It does not claim main integration, installed daily-data migration, real-model use, or whole-program completion.

## Authority and public flows

Schema 10 migrates documents, immutable creation witnesses, collaboration history, independent crew/view folder scopes and channel work references in the normal guarded schema transaction. The organization book remains the only #/W/H lifecycle authority. Folder parents and channel references never change work parentage, ownership or lifecycle.

Documents support Human and actual same-crew agent collaborative create/read/list/update/patch/soft-delete, revision CAS, retained original content audit, and immutable UUID creation witnesses across later edits. Read metadata is separate from body; default MCP body reads are bounded UTF-8 chunks, and patches send exact byte spans rather than full replacement content.

Messages/Todo/Documents/Progress each have independent classifications. Folder deletion promotes children and unclassifies direct content without deleting it. Current actor visibility filters refs on every read and write; manually authored shared folder names do not infer hidden channel information. Structured foreign references block source-crew purge until explicitly removed.

Channel Todo lists the actual channel's linked global #/W/H, with live source status and bounded labels. Reference operations require actual current channel visibility, scope revision CAS and active write eligibility. Group creation atomically creates the same authoritative requirement or plan and its channel reference. Confirming an old UUID after explicit unlink returns the current unlinked fact and never re-links. Cursor pagination binds the channel, reference revision and organization event high-water; changed facts produce finite conflicts. Closed H details are available through the ordinary read-only public-H ask.get contract.

New attached-agent ordinary document/folder/channel reads and writes cannot borrow default Human identity; credible agents use existing MCP identity paths. New requirement/group-create and Human-only write guards retain the existing local unattached Human trust model. This is not OS isolation. Resource/ref changes do not wake models; the existing fresh-requirement follow-up remains a guarded atomic effect of genuine requirement creation.

## Preserved independent evidence

- Doc/folder actual Unix socket/MCP: 5 passed, `/private/tmp/pcw-document-folder-workflow-02.log`. Includes immutable retries after edits, UTF-8 chunks and patch CAS, four views, compact receipts, expired cursor, current membership filtering, attached subscriber/read/write refusal, public foreign-topic leave, and purge refusal/explicit foreign-DM unclassification. Dynamic DM membership change is explicitly a Store-layer fact fixture, not a public DM-leave claim.
- Channel actual Unix socket/MCP: 4 passed, `/private/tmp/pcw-channel-work-workflow-01.log`. Includes two groups sharing one source lifecycle, public H blocking/answering, unlink then original-UUID confirmation without any duplicate allocator/event/wake, actual channel visibility, both cursor revisions, attached ordinary bypass refusal, foreign-source purge refusal/explicit unlink and SQL-trigger whole-creation rollback. The later public closed-H detail assertions are included in the combined gate rather than retroactively claimed by this initial log.
- Complete real CLI method flow: `/private/tmp/pcw-m2-cli-final-02.log`, 1 passed; actual document create/get/edit/retry/delete, classification create/assign/rename/delete, channel link/list/unlink and preserved object reads. Original fixture failure remains in 01 (missing object correctly returned existing ACL refusal rather than fixture's assumed NotFound).
- Tool contract: catalog 12 passed `/private/tmp/pcw-channel-work-catalog-02.log`; surface 2 passed `/private/tmp/pcw-m2-surface-final-01.log`; actual bilingual snapshots and RPC schema regenerated.
- True-v9 staged schema rollback/document witness tests: `/private/tmp/pcw-resources-store-staged-01.log`, 2 passed. Runtime old-binary migration/backup/import/restore acceptance requires the final normal-build witness below.

## Final combined and candidate boundaries

Combined workspace, strict clippy/fmt, normal-default debug build provenance, old-v9 normal-daemon migration and typed/native client evidence are recorded after their actual terminal results. Intermediate all-features test binaries are not default normal candidates. The final build must capture all frozen tracked Rust inputs before Cargo, actual toolchain/target/profile/features, command/exit/log and immutable pre-sign binary bytes.

### Final Rust gate

- Combined workspace original log `/private/tmp/pcw-m2-workspace-final-01.log`: 2536 passed, 1 legacy historical-fixture assertion failed, 8 ignored real-environment tests. The sole failure assumed true-v9 fixtures were unsupported. The corrected fixture now exercises actual versions 4–9, proves v9 has organization authority but no v10 resource tables, and rejects historical version 10; targeted GREEN `/private/tmp/pcw-m2-historical-v9-final-01.log`. This composes to **2537 distinct passed / zero unresolved / 8 ignored**, without rerunning already-passed unrelated targets or rewriting the original log.
- Strict workspace clippy, all targets/features, warnings denied: `/private/tmp/pcw-m2-clippy-final-04.log` PASS. Earlier lint logs remain. New conflict revisions use boxed internal storage without a JSON contract change; the reference Tx change has an explicit grouped input; no blanket lint suppressions.
- After these bounded refactors: actual channel 4 + document/folder 5 PASS `/private/tmp/pcw-m2-guard-wire-final-01.log`; includes closed-H ordinary detail and attached refusal. Proto 44 PASS `/private/tmp/pcw-m2-proto-final-01.log`; committed RPC schema match 1 PASS `/private/tmp/pcw-m2-schema-final-01.log`; fmt PASS `/private/tmp/pcw-m2-fmt-final-01.log`.

Normal candidate provenance and cross-version/typed GUI runtime evidence remain subsequent explicit layers, not inferred from these tests.
