# PendingCrew host installation

This directory prepares the one-command Linux installer and one device-enrollment link. It has not published a Linux release or installed a permanent VPS service. The retained R2 package cannot be substituted for this source.

A release owner assembles a reviewed **Linux ELF** with its clean default-build proof using `package-host-release.py`. Supply immutable HTTPS artifact and manifest URLs containing the full source commit. The output contains `host.tar.gz`, its SHA256-bound `manifest.json`, and an `install.py` entry with the manifest URL/hash embedded. Assembly performs no download, provider call, installation or publication. Publish only after the separately authorized release gates.

The eventual user entry is a single immutable installer URL, for example this command template (the URL must be the actual reviewed published entry):

```sh
curl -fsSL '<immutable install.py URL>' | sudo /usr/bin/python3 -I - install --user '<existing login user>' --advertise-host '<reachable host>' --listen-host '<explicit local bind IP>' --port 49381
```

The ordinary selected user must first install and log in to both providers themselves. This candidate accepts exactly Codex0.163.0 and Claude2.1.301. Missing executables, an unsupported version, or unsuccessful login status stops before PendingCrew prefix/data/unit writes and prints the official installation/login instructions. No provider installation, login, model request, credential import, downgrade or HOME/PATH/SHELL/CODEX_HOME assignment is performed. Successful version/login metadata is not proof of model access.

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
