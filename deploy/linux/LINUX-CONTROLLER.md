# Single Linux host-package controller (source preparation only)

This replaces the existing manual `crewd-linux-current.yml` recipe. It does not dispatch a run or qualify a Linux package/service. The exact frozen backend now contains B enrollment, default-off native APNs and the complete Codex input/history/profile module, Store20/API3/schema141. All five original source-side build/packager/installer recipes must remain byte-identical between C and S. The recorder, provenance verifier and managed service launcher are unchanged from Bbb033; the already accepted APNs root-admin installer remains intact, with only its explicit release/install/rollback format qualification and the packager format updated from19 to20 for this necessary writer barrier. No installation or downgrade compatibility path is added. No historical eight-case/testkit suite, historical reuse gate, new automatic trigger, provider invocation, deployment or publication is part of this job.

## Two immutable inputs

Root first integrates the independently accepted complete backend and complete Mac/iOS consumer with the then-current UI, then freezes the full source SHA **S**. Do not run an intermediate B-only build. Separately freeze/review controller commit **C**, containing this workflow and `scripts/dev/build-crewd-host-release.py`. The workflow is dispatched on a ref pointing exactly to C, with inputs `controller_sha=C` and `source_sha=S`. It explicitly checks out C into `controller/` and S into `source/`, disables checkout credential persistence, then checks actual controller HEAD == input C == GITHUB_SHA and actual source HEAD == input S. Each is a clean ordinary-file full tracked checkout with committed-blob byte equality, modes, hashes and source inventory recorded before and after.

The admitted source and controller Rust trees must both equal the exact frozen Codex/Store20 tree `2c258829cd4fa8b9af4c446bc81124ee18a90ddf`. Five source-side original witness/packager/installer files must also match their reviewed controller bytes. If a necessary Rust or recipe revision intervenes, this controller refuses rather than quietly building another capability set; review its exact update once before the single dispatch. Controller source does not have to be copied into S for its binaries to carry S honestly. A new C is not labelled B20c or S. Workflow/run ID, attempt, both full SHAs, controller recipe hashes and all input hashes are retained together.

After both complete SHAs and the single-run budget are separately approved, the normal future command is:

```sh
gh workflow run crewd-linux-current.yml --repo syncmeta/PendingCrew --ref CONTROLLER_REF -f controller_sha=FULL_C -f source_sha=FULL_S
```

This is an operation example, not an executed command. `CONTROLLER_REF` must resolve exactly to FULL_C at dispatch; mismatch refuses. No automatic repeat, second run, matrix, concurrency cancellation or workflow chaining is added.

## One bounded production job

Ubuntu24.04, one job, `timeout-minutes:30`, repository read-only permissions, existing toolchain/cache actions and source-locked Rust1.94.0. Estimated8–15min with dependency cache, not a guarantee. One locked fetch is followed by the existing `record-crewd-build.py` prepare/build and this sole Cargo command, cwd `source/crewd`:

```sh
cargo build --offline --locked --release -p crewd --message-format=json
```

The existing source-preflight invalidates owning-workspace artifacts while preserving dependency caches. Original `write-codex-first-provenance.py` checks the full frozen Rust inventory/digest, actual fresh production graph, raw compiler JSON and artifact identity, x86_64 Linux, Release/default features=[], no testkit/test profile or unexpected workspace features. Compiler override/buildtag environment injection is refused. Actual build-script env must contain S's12-character tag and dirty=false. Only the normal ELF's pure `schema` runs: full JSON == checked141, both issuer methods, exact delivery read and all eight Codex completion methods present, stderr empty, ELF hash unchanged. No serve, DB/migration, model, runtime smoke, account/status probe, old stable tests or installer execution occurs.

After those checks, project the original witness into B's existing packager proof fields, retaining its hash and controller SHA. Call the original `package-host-release.py` once to produce `install/host.tar.gz`, `install/manifest.json`, and `install/install.py`. Its source-qualified HTTPS URLs are only planned immutable Release locations `.../releases/download/host-S/...`. This does not publish those objects. Publishing the public repository source, running Actions, making these URLs downloadable, installing a root-managed service, binding/opening49381 and enrolling real clients are distinct authorization gates.

One Actions artifact contains exactly one installation tar and its two entries, original normal witness/ELF/source snapshot/graph/log, both complete source inventories and their equality, schema output, build proof, result, and byte manifest. The witness's original presign ELF is retained as evidence; there is no second install tar or relabelled582 package. On failure, the same single artifact step preserves available partial evidence, with no completed result or runtime qualification claim. An unqualified partial artifact is not an accepted release. Job timeout/cancellation may prevent even evidence upload; do not promise a terminal artifact after forced cancellation. Logs/metadata are preserved without retrying a failed job.

## Remaining real prerequisites

The old582 Linux package is not eligible. Existing VPS Debian user CLI lookup proved only no executable in that observed non-login PATH; it did not prove logged-out state. The user must install/login both providers themselves and satisfy exact Codex0.163.0/Claude2.1.301 metadata checks before any PCW install write. This is not model access qualification. Do not rewrite natural HOME/PATH/SHELL/CODEX_HOME, copy credentials, install/login on their behalf or downgrade automatically.

The old f0 B-client WIP proof and all earlier B18/controller evidence retain their historical labels. Subsequent client/source acceptance is recorded separately by root; this controller does not infer real Keychain/network/GUI qualification from any source test. The final selected S must contain the accepted matching Mac/iOS consumer and then-current UI. Source preparation cannot claim that combined package or real deployment has already passed. No public endpoint, systemd install/start, firewall rule, real-provider call or previous record mutation is authorized by this local controller preparation. Root selects the final service/data/public endpoint/client acceptance window after these dependencies and a successful accurate package exist.

The controller extension is static-only qualification; backend source and affected local synthetic protocols have their separate evidence. No Linux job has run. Local verification for this preparation is static only: Python AST, workflow YAML/typed shape, embedded shell/heredoc syntax, closed command/trigger/artifact scope, and Git diff/Rust-tree identity. It does not claim actual Actions/Release ELF/package/runtime/systemd/client acceptance. All prior B20c source/synthetic evidence retains its original label.
