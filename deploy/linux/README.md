# PendingCrew host installation

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

The ordinary selected user must first install and log in to both providers themselves; see the official [Codex CLI](https://developers.openai.com/codex/cli) and [Claude Code setup](https://code.claude.com/docs/en/setup) instructions. This candidate accepts exactly Codex0.163.0 and Claude Code2.1.301, even if an upstream installer offers a newer version. Missing executables, an unsupported version, or unsuccessful login status stops before PendingCrew prefix/data/unit writes and prints the official installation/login instructions. No provider installation, login, model request, credential import, automatic upgrade/downgrade or HOME/PATH/SHELL/CODEX_HOME assignment is performed. Successful version/login metadata is not proof of model access. Optional `--codex /absolute/path` and `--claude /absolute/path` select existing executables; the original preflight still checks their real systemd service-user environment.

For future releases, a release owner assembles a reviewed **Linux ELF** with its clean default-build proof using `package-host-release.py`, supplying immutable HTTPS artifact and manifest URLs containing the full source commit. Its three-file output has the same contract above. Assembly alone performs no download, provider call, installation or publication; publish only after the separately authorized release gates.

The installer verifies the immutable package and offline schema, refuses active runtime or changed activation UUID, then activates root-owned code under `/opt/pendingcrew`. Its background Core runs as that same ordinary login user; data defaults to their real `~/.local/share/PendingCrew` and remains private0700. Existing data is retained. The service shares the installed-prefix flock; Core retains its original exclusive `crewd.lock`. There is no duplicate data lease. The systemd service receives only the explicitly named root-created listener FD. Its admin pathname is `/run/pendingcrew/admin.sock`, root0700 parent/root0600 socket. Ordinary work remains the original mandatory-client-certificate TLS service.

Installation ends by printing one short-lived `pendingcrew://enroll/v1/...` link. It is a sensitive one-time link, not a permanent administrator credential. Deliver it deliberately to the intended device; do not put it in process argv, logs or cloud directories. The device explicitly confirms the host identity, creates its own local private key, and performs the signed challenge/commit protocol. Each device receives a separate revocable grant. A directory entry or copied link never copies another device's authority. The link is distinct from the host's `tls://host:port` transport descriptor.

The installer saves original installation/configuration/enrollment UUIDs in root-private records before each mutation. If a response is lost, retain the operation record and query its original result; do not repeat installation or create another token automatically. Normal management uses the same installed CLI:

```sh
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py status
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py grants
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py installation --request-id '<original install UUID>'
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py configuration --operation-file '<original root0600 configure record>'
sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py recover --operation-file '<original root0600 enrollment record>'
```

`resume-installation --request-id <original install UUID>` explicitly continues only an already-current original activation: it rechecks the same user/provider qualification, repairs that original unit/binding if quiescent, starts the service, and queries any existing configuration/enrollment operation records. It does not extract/activate again or resend unknown writes. If no phase record existed, it may perform that phase once with UUIDs already stored in the original installation record. A notFound witness after a saved mutation record stops continuation.

`configuration` returns the original witnessed result, not fresh status presented as proof of a prior write. `recover` only looks up the original enrollment creation and can reproduce its same saved link; notFound does not authorize replay. If the link has expired, an operator may deliberately create a new link for a new device using `pair --operation-file '<new root-private file>' --host '<host>' --port 49381`. Existing operation files are never overwritten. Revoke uses the current original grant ID and revision (`revoke --grant-id ... --expected-revision ... --note ...`); if its response is uncertain, inspect that same grant instead of automatically resending the CAS.

`stop` requests the original bounded safePause lifecycle and stops socket activation only after its stopped receipt. It never force-kills a run or erases data. An unknown stop result is reported as unknown. Restart is an explicit operator service action; it is not task resume. Code rollback is also explicit: `rollback --request-id '<target original install UUID>' --expected-current '<current activation UUID>' --operation-id '<new rollback UUID>'`. Both the service and socket activation must already be stopped; exclusive prefix locks must be free; installation user/data binding, immutable release hashes, stopped SQLite application identity and writer format must match. Pending WAL/journal or a different format prevents rollback. Rollback neither recovers a database nor starts a service/task. No automatic rollback follows an uncertain activation.

Mac hosts use the existing App-owned Local Human management connection to issue the same link. This Linux root socket does not create a second Mac management role, grant ordinary same-UID callers Human authority, or authorize paired devices/Agents to issue links.

Source tests use private synthetic package headers, existing root-peer/remote-workflow seams and loopback TLS. They do not qualify actual Linux systemd deployment, public networking, signed distribution or provider inference; those retain their separate action and evidence gates.

A complete received management request may finish its single transaction after the caller disconnects while its original root process/birth and managed-lineage floor still qualify. EOF does not retract a received request or roll back an accepted write. A lost reply is Unknown and uses the original result lookup; it is never automatically resent. Explicit logical connection-scope closure is checked, but the single-request reader does not promise concurrent EOF observation while awaiting a database operation.
