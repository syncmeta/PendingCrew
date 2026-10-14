# Linux ordinary-user installation/update/rollback unit (source-review freeze)

This module prepares an inspectable development distribution and tests only an explicit synthetic owner-private prefix. It does not install a real system service, start a login item, alter daily data, use a real model or publish a release. Actual Linux runner acceptance is still pending.

## Runtime and switching contract

- Normal UID/euid, canonical owned0700 prefix and private0600 locks. Aliases, escape, wide permissions and mixed-elevation calls refuse. First installation with no release needs no process scan.
- Managed launcher takes an inherited shared runtime flock before resolving current, preserves that lease across exec and throughout actual daemon lifetime. The wrapper→daemon fixture does not prove all genuine SDK-agent descendant inheritance; those paths remain separately dependent on Core/SDK policy. Writer-exclusive runtime and installation locks cover every install/rollback; concurrent CAS cannot both commit.
- Relevant same-UID private-prefix binary PID/birth/executable observations fail closed on unreadable or changed identity. Unrelated other-UID system processes cannot access the0700 prefix and do not veto personal installation. A one-time scan is not permanent quiescence. Host-root/elevated and raw-launch bypasses are outside managed-launcher guarantees; Core remains the database/process authority.
- Retain complete immutable releases and provenance. A durable request UUID binds action, source/binary/full inventory and prior activation. Preparation, atomic activation, actual pointer/history readback distinguish absent, prepared unknown, committed current/history; never blindly repeat an unknown switch. Payload mismatch refuses.
- Installer never opens or modifies databases and never invents DDL. Rollback switches the binary pointer only. Old11 must reject a13 database before touching it; the new Core's formal restore CLI handles retained backups, followed by explicit pointer choice and actual restart.

## Fixed complete acceptance flow

Original Linux11 baseline is accepted canonical tar from run38500948870/artifact11866465185, source7b1849d97a813a1655ebafb66068a04a614e8787, tar SHA2568a9cc7395f66c1fd60283c1aeceb26ef762b7e8205ac04ad496feb28c7742c94 and binaryc5a65cc9472fa92bdea9bc60a80f049a869045cc57706feffb19416dd5e54cda. Its original loose hidden-file upload RED remains; only the normative complete tar is used, without reconstruction or new-build labeling. New13 package must have a different actual source/binary, normal default release and full fresh owning-workspace proof.

The explicit defaultfalse `linux_install_upgrade` dispatch is mutually exclusive with the other four opt-in scopes, preserving normal push/main/PR/default full checks. It verifies original API run/artifact identity, strict canonical tar/checker proof, builds the actual new release with existing runner cache and packaged recovery, then runs the whole ordinary nonroot flow: install11 → live managed lease rejects switch → wrapper exits while its actual daemon retains lease → stop → CAS/prepared failure/unknown readback → real11→13 upgrade → two concurrent writers admit one CAS winner → pointer rollback leaves DB untouched/old11 refuses13 → authoritative new Core restore11 → old11 reopen with original request identity → restore13/restart/integrity. Models and remote listeners remain off.

Controlled related unreadable/live/birth-change/other-UID/private-mode/symlink negatives are included separately. They establish the guard behavior, not actual forced PID reuse. Actual ordinary Linux process FD inheritance, wrapper-child survival, lock refusal and full migration require the real remote run. All source, old/new archives, raw build/config/graph logs, data/backup/replaced files and request outcomes are retained in private workflow artifacts.

## Author checks retained (not actual Linux acceptance)

- `/private/tmp/pcw-linux-installer-source-controls-onguwmkz`: controlled filesystem/process metadata guards passed, actual PID reuse explicitly false.
- `/private/tmp/pcw-linux-install-workflow-source-b80ckg8k`: actual YAML parsed, all32 selector combinations and default push/PR behavior verified, every shell step syntax checked.
- `/private/tmp/pcw-linux-install-canonical-baseline-control-01.log`: original canonical tar/full hidden Rust inputs/source/default-release/ELF/log/binary checker passed without executing a Linux binary on Mac. No loose proof repaired.

This is a complete source-review unit ready for independent quality review, not a passed installation milestone. Actual runner failures go directly to the author and preserve their logs; stable M5/Core/TLS/business acceptance is reused.

## Actual first run and read-only verification closure

Run38533350050, exact5f, terminal failure: original run/artifact identity and canonical11 tar/full proof passed; the new-release recipe refused before compilation because its checkout was dirty. No install/lease/upgrade/restore phase ran. Complete original log `/private/tmp/pcw-linux-install-5f3f7595-run38533350050-failed.log` and artifact11878604282 remain unchanged. That runner did not save its Git status, so the original dirty-path listing cannot be recovered or invented.

The preceding reader dynamically imports the source consumer. A real source-backed reproduction in a private Git fixture, with Linux-default `sys.pycache_prefix=None`, shows the unchanged5f reader strictly verifies the package but creates untracked `scripts/dev/__pycache__/`. Mac system Python's external cache default hid this platform difference during the earlier local control. This is the reproduced causal explanation, distinguished from an original-run status observation. `/private/tmp/pcw-linux-reader-clean-whole-control-01/result.json` preserves actual old5f/new source bytes, commands, before/after statuses and RED/GREEN.

The checker now disables bytecode before dynamic imports; the isolated job also disables source bytecode writing globally. Strict clean/fresh/input guards remain. Before/after baseline verification and reader controls record actual checkout status outside the checkout, and any builder clean refusal prints its actual status. The next actual Linux run reproduces old-reader RED and fixed-reader GREEN in separate private source fixtures, then builds a new exact-source normal release and executes the still-unverified whole lifecycle. No ignore rule, cleanup or existing proof relabeling bypasses the guard.

The permanent source-backed reader control is `scripts/dev/test-crewd-linux-reader-clean.py`; final local execution `/private/tmp/pcw-linux-reader-clean-whole-control-02/result.json` preserves both exact old5f and new checker/consumer/builder sources and confirms RED/GREEN. The subsequent real Linux run additionally tests a deliberately modified tracked source: the clean guard must refuse with actual status before creating any build output or invoking Cargo. This negative is not claimed from the Mac control. No source or cache is removed to make the owning checkout clean.
