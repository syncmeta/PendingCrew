# Current SQLite Linux managed-prefix delivery

This unit consumes the accepted immutable Linux candidate compiled at `c8d70482e42b0e9f777df11f9db720a910a91115`: canonical tar SHA256 `ce3f377134f3c652fb50bc1477cbe5d5867ba1ef2ebcac8f98016f3c940196ab`, ELF `72fdab4aeaf1ad256ea488d4aece836f0e2c220de417d49a4113bf75ff597f1a`, 443 Rust inputs/digest `673cd2bff312b747ca71bff29aa0d3abe2ba04872ed68108f8966827f4f99d50`. Controller/fixture source is the separately reviewed workflow commit, never a new compiled source label. No Rust compilation, old baseline, second version, service, model, SDK or daily data belongs here.

`crewd-linux-managed-current.yml` is manual-only. One ordinary Ubuntu job verifies the exact original successful run/artifact/ZIP/tar/proof, installs into a new owner-private prefix, then uses the actual installed launcher. The existing installer retains its original platform/ELF/proc, immutable-release, runtime/data lease, durable request and activation CAS checks. The new source-family entry only permits Store's existing Core WAL validation for the complete exact current normal source; a digest entry alone is not runtime qualification.

The whole flow checks clean start/stop/reopen, live inherited prefix/data leases, failed stale CAS with unchanged data, durable prepared Unknown read only through status, two genuine concurrent writers with one winner, real SIGKILL leaving nonempty Core WAL, offline-reader refusal without changed bytes, copied bad-WAL/future-main no-touch refusals, same-version managed recovery of committed knowledge/view/placement and original UUIDs, zero agent sessions, and normal final shutdown. Knowledge commits use the current ACL and SQLite FULL/WAL transaction contract; this does not claim Core's special no-touch guard for arbitrary corrupt knowledge WAL. Core-only restore remains forbidden for knowledge workspaces. Existing c8 knowledge snapshot/confirmation/epoch and seven-case evidence are reused.

The lifecycle archive uses the existing non-dereferencing archiver and includes its file/hash/link manifest. Socket/FIFO entries are diagnostic metadata, not restorable contents. Failed runs and their original data remain evidence. No source-family, artifact or test result is relabelled after later documentation/controller changes.

## Explicit ordinary-user prefix use

After verifying/extracting the original canonical package, select a fresh canonical private prefix and data directory. Keep the full proof with the binary. Replace the example absolute paths and UUIDs explicitly:

```sh
mkdir -m 700 /absolute/private-prefix /absolute/private-data
python3 -B scripts/dev/install-crewd-linux-candidate.py install --prefix /absolute/private-prefix --package /absolute/crewd-linux-x86_64 --request-id YOUR-NEW-UUID --expected-current none
/absolute/private-prefix/bin/crewd --data-dir /absolute/private-data serve --codex /usr/bin/false --claude /usr/bin/false
python3 -B scripts/dev/install-crewd-linux-candidate.py status --prefix /absolute/private-prefix --request-id YOUR-ORIGINAL-UUID
```

Use the managed `prefix/bin/crewd` for this guarantee. Stop the owned daemon normally before activation changes. A lost response calls for status of the original UUID; never invent a replacement request or delete an Unknown activation. Do not use raw release paths to bypass a refusal. Pending WAL permits only the exact qualified managed `serve` recovery route; offline readers remain refused until recovery completes. This is a private user prefix, not system installation or an automatic background service.

## Mac fixture development boundary

The explicit `--development-macos` consumer substitutes only Linux host/ELF/proc observation edges with strict accepted native Mac proof and native process observation. It runs the same installer transaction/locking/CAS and real managed/Core lifecycle in a new private workspace. Its `compiledRef` remains the actual Mac source, and it is never Linux acceptance. Actual Linux uses the unmodified production installer/checker/proc path with no adapter. The whole source/runtime/archive unit requires independent acceptance before being described as delivered.
