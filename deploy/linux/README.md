# PendingCrew host installation

The current source improves the installation prerequisite checks described below. It has not replaced the published 0.2.0 installer. Use the exact installation entry supplied with a reviewed release; changing a URL or copying this source does not create a new released package.

The Linux x86_64 installer and macOS installation package share the single [0.2.0 Release](https://github.com/syncmeta/PendingCrew/releases/tag/v0.2.0). Linux provides one installation command and one device-enrollment link. The retained R2 package cannot be substituted for this source.

Distribution version0.2.0 reuses the independently accepted Linux ELF and original build proof from source `fd7a563fd1d69365ab38811c7e5c6718aa4cd66d` (API3/schema149/Store21); it is not a new Rust build or a relabelled compiled source. All three Linux download URLs belong to v0.2.0, with the exact compiled-source SHA retained in the tar and manifest filenames. The older stable [0.1.42](https://github.com/syncmeta/PendingCrew/releases/tag/v0.1.42) and Homebrew installation context remain separate from this release.

The original Linux build and the affected0.2.0 packaging passed independent artifact review. The table below identifies the exact three reviewed assets. The published installer and manifest were actually downloaded and their hashes matched; the complete tar download and actual installation of these0.2.0 URLs have not been verified. Target VPS deployment, permanent service, public port and device pairing remain unaccepted; this Mac/Linux release does not claim complete iOS, Cloud synchronization or APNs delivery.

On a Linux x86_64 host with Python 3 and systemd, replace the existing ordinary login user, reachable address and explicit local bind IP, then run the original entry:

```sh
curl -fsSL 'https://github.com/syncmeta/PendingCrew/releases/download/v0.2.0/PendingCrew-0.2.0-linux-install.py' | sudo /usr/bin/python3 -I - install --user '<existing ordinary login user>' --advertise-host '<reachable host>' --listen-host '<explicit local bind IP>' --port 49381
```

This is a Python entry executed from stdin in isolated mode, not a shell installer. Its embedded HTTPS manifest URL and SHA256 bind the original tar. It still downloads the manifest and tar; copying only the installation script is not an offline installation.

| 0.2.0 Release asset | Bytes | SHA256 |
| --- | ---: | --- |
| [PendingCrew-0.2.0-linux-install.py](https://github.com/syncmeta/PendingCrew/releases/download/v0.2.0/PendingCrew-0.2.0-linux-install.py) | 45811 | `8185463692fdd33f1c0cc742bdaf0acb79a955a3515fcaf81626cb27e2b88ce1` |
| [Linux manifest](https://github.com/syncmeta/PendingCrew/releases/download/v0.2.0/PendingCrew-0.2.0-linux-x86_64-fd7a563fd1d69365ab38811c7e5c6718aa4cd66d.manifest.json) | 399 | `d969559e4ad559c8c33d4a7f3c57d5079673798a6471591c84261b3258c241f6` |
| [Linux x86_64 tar](https://github.com/syncmeta/PendingCrew/releases/download/v0.2.0/PendingCrew-0.2.0-linux-x86_64-fd7a563fd1d69365ab38811c7e5c6718aa4cd66d.tar.gz) | 21526325 | `1afb10f885683dbf1849bc2129e098cdd6fcd4fe16afb56a70fe0860696e9486` |

Before installing, sign in as the existing ordinary Linux user that will run PendingCrew. Install and authenticate both tools yourself using the official [Codex CLI installation](https://learn.chatgpt.com/docs/codex/cli), [Codex headless authentication](https://learn.chatgpt.com/docs/auth), [Claude Code setup](https://code.claude.com/docs/en/setup) and [Claude authentication](https://code.claude.com/docs/en/authentication) instructions:

```sh
# Run these yourself as the intended service user, without sudo.
curl -fsSL https://chatgpt.com/codex/install.sh | sh
codex login --device-auth
curl -fsSL https://claude.ai/install.sh | bash
claude auth login
```

The current source checks both tools and reports their failures together before writing any PendingCrew prefix, data or unit. Codex must identify itself with a version, expose its `app-server` command help, and pass its login-status command. It is not restricted to the historical 0.163.0 string. Claude must still be exactly 2.1.301: the managed-task hook contract in Core requires that version. If your installed Claude differs, review the official specific-version installation instructions and decide explicitly; PendingCrew never installs or downgrades it for you. The published 0.2.0 entry above still has its original exact Codex0.163.0/Claude2.1.301 checks.

These are installation prerequisites, not a full protocol or model-access test. Successful login status does not guarantee subscription, quota, model access, or every managed feature. Tool output and authentication errors are not echoed. No provider installation, login, model request, credential import, automatic upgrade/downgrade or HOME/PATH/SHELL/CODEX_HOME assignment is performed. Root is only the installer administrator; it cannot be selected as the service account, and its credentials are never copied to that account. Optional `--codex /absolute/path` and `--claude /absolute/path` select existing executables; checks run in the same real systemd service-user context.

For future releases, a release owner assembles a reviewed **Linux ELF** with its clean default-build proof using `package-host-release.py`, supplying immutable HTTPS artifact and manifest URLs containing the full source commit. Its three-file output retains the immutable artifact contract above. The installer owns one literal `PROVIDER_POLICY`; the assembler reads that same literal into `providerPrerequisites` in the manifest, and the installer rejects a different policy. This metadata describes installation prerequisites, not a blanket provider qualification. Old published manifests and installers remain unchanged. Assembly alone performs no download, provider call, installation or publication; publish only after the separately authorized release gates.

## Reusing an unchanged reviewed Core

A release owner can assemble new Python deployment components around an already reviewed Linux Core without rebuilding Rust. `package-host-release.py --source <package commit> --core-source <original Core commit> ...` records `coreSourceCommit` in both the artifact descriptor and `release.json`. The existing `sourceCommit` identifies the package's frozen deployment source and immutable release directory. The original ELF and `build-proof.json` remain byte-for-byte unchanged and retain the original compiled commit; they are never relabelled as a new build.

For different component commits, the assembler requires identical committed `crewd` trees, checks the actual Rust source bytes/types/modes against their identical Git blobs, and checks the assembler/installer/wrapper bytes against the package commit. A source change or changed original proof prevents assembly. The installer checks the manifest/release component commit agreement and the binary proof against that original Core commit, while retaining its existing digest, tar, ELF, API and store checks. Without `--core-source`, the original same-source behavior and metadata remain unchanged; older packages that omit `coreSourceCommit` use `sourceCommit` for the Core too. This assembly is not a new Rust build, upload, deployment or provider test.

## Persistent service and a Codex-only host

The current source supports an explicit `install --disable-claude` choice. It checks and enables Codex only; Claude need not be installed or authenticated. The default still checks both tools. Supplying `--claude` together with `--disable-claude` is rejected. The original installation record retains this choice, and `resume-installation` uses that record rather than rediscovering an omitted provider. The systemd wrapper passes Core its existing `--disable-claude` flag. It removes only inherited `CREWD_CODEX` and `CREWD_CLAUDE` from the child environment, retaining the real service user's other environment and the genuine socket-activation descriptors.

These source options require a newly assembled and reviewed installer; the published 0.2.0 entry above does not contain them. They do not remove the ordinary-user requirement. If only root is logged in to Codex, choose an ordinary login account and sign in to Codex yourself as that account first. PendingCrew never creates an account, imports root credentials, or migrates vendor session history. Moving existing Core data into the selected user's private home requires a separately approved stopped-data operation, with a retained original copy and the complete database/TLS identity data kept together. Do not copy a live database or open `/root` permissions to make it reachable. Old vendor sessions belonging to root are not promised to resume under another account.

To retain an existing host connection, obtain the original `remote.status` from its still-live original management shell, or the identical `host.status` through an existing installed root management entry. Save that JSON result in an existing root-owned0700 directory as an owned0600 file. Do this before stopping the original Core. The file contains the existing public identity and listener status, not a private key or a new transaction receipt.

Pass `install --preserve-connection-from /root/private/original-status.json` with the same explicit `--listen-host` and `--port`, the selected ordinary `--user`, and the existing private `--data-dir`. Add `--disable-claude` for Codex-only operation. The bare `--preserve-connection` flag is not supported. Input must have the original complete identity, configuration and endpoint, with enabled/listening true, no failure and the exact requested bind. Missing, unreadable, incorrectly protected or invalid input stops installation before provider checks or writes. The data directory and its service-user0600 database must already exist.

The original status is saved in the existing installation operation record. After service activation, and on an explicit continuation of that original installation, `host.status` must match its identity, certificate, database ID, pin, configuration revision and endpoint exactly. A mismatch, failed listener or unknown result stops; it never reconfigures or issues a new invitation. `host.grants` is current readback only, not proof of an old complete grant list. Without this explicit input, a new installation retains its original configuration/CAS and single-link flow. Never use a new pairing link to conceal an unexpected identity change.

A normal installed host uses systemd socket activation and its root-only typed admin socket, so Core management does not depend on an SSH parent shell. Normal administrative commands can be run from a new root terminal:

```sh
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py status
sudo journalctl -u pendingcrew.service -n 100
# Request the bounded normal stop and require its stopped result.
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py stop
# An explicit later service start restores connectivity, not model tasks.
sudo systemctl start pendingcrew.socket pendingcrew.service
```

`Restart=on-failure` keeps the Core service available; it is not permission to resume a session or send old pending/Unknown work. Normal service restart and App reopen do not create a model task. A fresh explicit Human task or an explicit permitted session action is still required. Safe stop must complete before replacement; an unknown stop result is not permission to force-kill or erase data. A loopback deployment still needs its client-side SSH tunnel or other deliberately configured transport; making Core persistent does not make a Mac tunnel persistent or expose a public port.

The installer verifies the immutable package and offline schema, refuses active runtime or changed activation UUID, then activates root-owned code under `/opt/pendingcrew`. Its background Core runs as that same ordinary login user; data defaults to their real `~/.local/share/PendingCrew` and remains private0700. Existing data is retained. The service shares the installed-prefix flock; Core retains its original exclusive `crewd.lock`. There is no duplicate data lease. The systemd service receives only the explicitly named root-created listener FD. Its admin pathname is `/run/pendingcrew/admin.sock`, root0700 parent/root0600 socket. Ordinary work remains the original mandatory-client-certificate TLS service.

The advertised address must actually reach the configured TLS listener. The default `--listen-host 127.0.0.1` is usable only through an existing reachable tunnel; putting a public hostname in the link does not open a port. For direct access, deliberately select the appropriate bind IP and configure host routing/firewall access. The installer does not create a tunnel, change a firewall or certify public reachability.

Installation ends by printing one short-lived `pendingcrew://enroll/v1/...` link. It is a sensitive one-time link, not a permanent administrator credential. Deliver it deliberately to the intended device; do not put it in process argv, logs or cloud directories. The device explicitly confirms the host identity, creates its own local private key, and performs the signed challenge/commit protocol. Each device receives a separate revocable grant. A directory entry or copied link never copies another device's authority. The link is distinct from the host's `tls://host:port` transport descriptor.

The installer saves original installation/configuration/enrollment UUIDs in root-private records before each mutation. If a response is lost, retain the operation record and query its original result; do not repeat installation or create another token automatically. Normal management uses the same installed CLI:

```sh
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py status
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py grants
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py installation --request-id '<original install UUID>'
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py configuration --operation-file '<original root0600 configure record>'
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py recover --operation-file '<original root0600 enrollment record>'
```

`resume-installation --request-id <original install UUID>` explicitly continues only an already-current original activation: it rechecks the same user/tool prerequisites, repairs that original unit/binding if quiescent, starts the service, and queries any existing configuration/enrollment operation records. It does not extract/activate again or resend unknown writes. If no phase record existed, it may perform that phase once with UUIDs already stored in the original installation record. A notFound witness after a saved mutation record stops continuation.

`configuration` returns the original witnessed result, not fresh status presented as proof of a prior write. `recover` only looks up the original enrollment creation and can reproduce its same saved link; notFound does not authorize replay. If the link has expired, an operator may deliberately create a new link for a new device using `pair --operation-file '<new root-private file>' --host '<host>' --port 49381`. Existing operation files are never overwritten. Revoke uses the current original grant ID and revision (`revoke --grant-id ... --expected-revision ... --note ...`); if its response is uncertain, inspect that same grant instead of automatically resending the CAS.

`stop` requests the original bounded safePause lifecycle and stops socket activation only after its stopped receipt. It never force-kills a run or erases data. An unknown stop result is reported as unknown. Restart is an explicit operator service action; it is not task resume. Code rollback is also explicit: `rollback --request-id '<target original install UUID>' --expected-current '<current activation UUID>' --operation-id '<new rollback UUID>'`. Both the service and socket activation must already be stopped; exclusive prefix locks must be free; installation user/data binding, immutable release hashes, stopped SQLite application identity and writer format must match. Pending WAL/journal or a different format prevents rollback. Rollback neither recovers a database nor starts a service/task. No automatic rollback follows an uncertain activation.

Mac hosts use the existing App-owned Local Human management connection to issue the same link. This Linux root socket does not create a second Mac management role, grant ordinary same-UID callers Human authority, or authorize paired devices/Agents to issue links.

Source tests use private synthetic package headers, existing root-peer/remote-workflow seams and loopback TLS. They do not qualify actual Linux systemd deployment, public networking, signed distribution or provider inference; those retain their separate action and evidence gates.

A complete received management request may finish its single transaction after the caller disconnects while its original root process/birth and managed-lineage floor still qualify. EOF does not retract a received request or roll back an accepted write. A lost reply is Unknown and uses the original result lookup; it is never automatically resent. Explicit logical connection-scope closure is checked, but the single-request reader does not promise concurrent EOF observation while awaiting a database operation.
