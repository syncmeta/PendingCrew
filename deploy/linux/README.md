# Linux 后台安装

macOS 客户端与 Linux 后台统一发布在 [0.2.2 预览版（半成品）](https://github.com/syncmeta/PendingCrew/releases/tag/v0.2.2)。在 Linux 主机执行：

```sh
curl -fsSL https://github.com/syncmeta/PendingCrew/releases/download/v0.2.2/install.sh | sudo sh
```

按提示选择已有普通账号、Mac/iPhone 可达的主机地址、本机监听 IP、端口，以及仅 Codex 或 Codex + Claude。安装成功后得到一个设备配对链接，在 PendingCrew 客户端打开并确认即可。root 只负责安装，后台始终使用选定的普通账号运行。

## 安装前准备

- 当前程序包仅支持 **Linux amd64（x86_64）、glibc 2.39 或更新版本**；不适用于 musl/Alpine、arm64 或较旧 glibc。
- 需要 curl、Python 3.9+、运行中的 systemd，以及 sudo/root 安装权限。
- 先以将要运行后台的普通账号安装并登录 Codex；选择同时使用 Claude 时还需要 Claude Code **2.1.301**。脚本检测现有工具，缺少时给出指引并停止，由你自行安装、登录后再运行。不会自动安装或降级工具、复制 root 凭据或运行模型任务。

官方步骤：[Codex 安装](https://learn.chatgpt.com/docs/codex/cli)、[Codex 无浏览器登录](https://learn.chatgpt.com/docs/auth)、[Claude Code 安装](https://code.claude.com/docs/en/setup)、[Claude 登录](https://code.claude.com/docs/en/authentication)。

如果通过公网连接，需要填可达的公网地址、合适的监听 IP，并自行配置防火墙。`127.0.0.1` 只能通过你已配置的隧道连接；脚本不会替你开端口或建立隧道。工具登录成功也不代表模型额度或全部功能已验。

## 日常管理

```sh
sudo pendingcrew status
sudo pendingcrew pair
sudo journalctl -u pendingcrew.service -n 100
```

`pair` 为新设备生成一个短期、一次性配对链接。不要把它放进公开日志。每台设备拥有独立授权。若操作结果未知，按提示查询原操作记录；不要重复安装或重复生成邀请。

## 下载文件

| 文件 | 用途 |
| --- | --- |
| `install.sh` | 推荐的一条命令安装入口 |
| `pendingcrew-0.2.2-linux-amd64.tar.gz` | Linux 后台及管理工具 |
| `pendingcrew-0.2.2-linux-amd64.manifest.json` | 压缩包摘要、平台与完整组件来源 |
| `pendingcrew-0.2.2-linux-amd64.install.py` | 由入口自动下载、核验的底层安装器，无需手动调用 |
| `SHA256SUMS` | 下载文件的 SHA-256 校验值 |

全部文件在同一个 Release 中。旧长文件名资产保留，避免旧链接失效；推荐使用上面的新入口。脚本依次核验 installer、manifest 和压缩包摘要。下载这些文件不等于离线安装；默认安装仍从固定版本的 HTTPS 地址取得其依赖文件。

本次修正安装入口和发行包装，复用原 `ccfd1bc58dc68a0b6572c489d46aaf11ec507cef` 的 Linux Core 与构建证明，组件来源分别记在 manifest 和包内 `release.json`；没有重新编译或重标 Core。新入口已做独立隔离测试，未用它重装 Joy，也不宣称新入口已完成新的真实主机安装。

## 无交互安装与已有安装

自动化调用可下载 `install.sh` 后使用 `sudo sh install.sh --non-interactive --user <普通账号> --advertise-host <可达地址> --listen-host <监听IP> --port 49381`。仅 Codex 时加 `--disable-claude`；loopback 还需明确提供 `--accept-loopback-tunnel`。交互参数读取 `/dev/tty`，不与下载脚本的标准输入混用。

新入口拒绝覆盖已存在或尚未完成的安装。升级、保留原连接及结果未知恢复沿用下面的管理流程；先查询状态并正常停止任务，不以重装覆盖问题。旧版本若没有 `pendingcrew` 短命令，仍可用 `sudo /usr/bin/python3 -I /opt/pendingcrew/current/bin/pendingcrew-host.py <command>` 管理。

---

以下为发布维护与高级恢复说明。

## Reusing an unchanged reviewed Core

A release owner can assemble new Python deployment components around an already reviewed Linux Core without rebuilding Rust. `package-host-release.py --source <package commit> --core-source <original Core commit> ...` records `coreSourceCommit` in both the artifact descriptor and `release.json`. The existing `sourceCommit` identifies the package's frozen deployment source and immutable release directory. The original ELF and `build-proof.json` remain byte-for-byte unchanged and retain the original compiled commit; they are never relabelled as a new build.

For different component commits, the assembler requires identical committed `crewd` trees, checks the actual Rust source bytes/types/modes against their identical Git blobs, and checks the assembler/installer/wrapper bytes against the package commit. A source change or changed original proof prevents assembly. The installer checks the manifest/release component commit agreement and the binary proof against that original Core commit, while retaining its existing digest, tar, ELF, API and store checks. Without `--core-source`, the original same-source behavior and metadata remain unchanged; older packages that omit `coreSourceCommit` use `sourceCommit` for the Core too. This assembly is not a new Rust build, upload, deployment or provider test.

## Persistent service and a Codex-only host

The current installer supports an explicit `install --disable-claude` choice. It checks and enables Codex only; Claude need not be installed or authenticated. The default still checks both tools. Supplying `--claude` together with `--disable-claude` is rejected. The original installation record retains this choice, and `resume-installation` uses that record rather than rediscovering an omitted provider. The systemd wrapper passes Core its existing `--disable-claude` flag. It removes only inherited `CREWD_CODEX` and `CREWD_CLAUDE` from the child environment, retaining the real service user's other environment and the genuine socket-activation descriptors.

These options are included in the current installer. They do not remove the ordinary-user requirement. If only root is logged in to Codex, choose an ordinary login account and sign in to Codex yourself as that account first. The installer never creates an account, imports root credentials, or migrates vendor session history. Moving existing Core data into the selected user's private home requires a separately approved stopped-data operation, with a retained original copy and the complete database/TLS identity data kept together. Do not copy a live database or open `/root` permissions to make it reachable. Old vendor sessions belonging to root are not promised to resume under another account.

To retain an existing host connection, obtain the original `remote.status` from its still-live original management shell, or the identical `host.status` through an existing installed root management entry. Save that JSON result in an existing root-owned0700 directory as an owned0600 file. Do this before stopping the original Core. The file contains the existing public identity and listener status, not a private key or a new transaction receipt.

Pass `install --preserve-connection-from /root/private/original-status.json` with the same explicit `--listen-host` and `--port`, the selected ordinary `--user`, and the existing private `--data-dir`. Add `--disable-claude` for Codex-only operation. The bare `--preserve-connection` flag is not supported. Input must have the original complete identity, configuration and endpoint, with enabled/listening true, no failure and the exact requested bind. Missing, unreadable, incorrectly protected or invalid input stops installation before provider checks or writes. The data directory and its service-user0600 database must already exist.

The original status is saved in the existing installation operation record. After service activation, and on an explicit continuation of that original installation, `host.status` must match its identity, certificate, database ID, pin, configuration revision and endpoint exactly. A mismatch, failed listener or unknown result stops; it never reconfigures or issues a new invitation. `host.grants` is current readback only, not proof of an old complete grant list. Without this explicit input, a new installation retains its original configuration/CAS and single-link flow. Never use a new pairing link to conceal an unexpected identity change.

A normal installed host uses systemd socket activation and its root-only typed admin socket, so Core management does not depend on an SSH parent shell. Normal administrative commands can be run from a new root terminal:

```sh
sudo pendingcrew status
sudo journalctl -u pendingcrew.service -n 100
# Request the bounded normal stop and require its stopped result.
sudo pendingcrew stop
# An explicit later service start restores connectivity, not model tasks.
sudo systemctl start pendingcrew.socket pendingcrew.service
```

`Restart=on-failure` keeps the Core service available; it is not permission to resume a session or send old pending/Unknown work. Normal service restart and App reopen do not create a model task. A fresh explicit Human task or an explicit permitted session action is still required. Safe stop must complete before replacement; an unknown stop result is not permission to force-kill or erase data. A loopback deployment still needs its client-side SSH tunnel or other deliberately configured transport; making Core persistent does not make a Mac tunnel persistent or expose a public port.

The installer verifies the immutable package and offline schema, refuses active runtime or changed activation UUID, then activates root-owned code under `/opt/pendingcrew`. Its background Core runs as that same ordinary login user; data defaults to their real `~/.local/share/PendingCrew` and remains private0700. Existing data is retained. The service shares the installed-prefix flock; Core retains its original exclusive `crewd.lock`. There is no duplicate data lease. The systemd service receives only the explicitly named root-created listener FD. Its admin pathname is `/run/pendingcrew/admin.sock`, root0700 parent/root0600 socket. Ordinary work remains the original mandatory-client-certificate TLS service.

The advertised address must actually reach the configured TLS listener. The default `--listen-host 127.0.0.1` is usable only through an existing reachable tunnel; putting a public hostname in the link does not open a port. For direct access, deliberately select the appropriate bind IP and configure host routing/firewall access. The installer does not create a tunnel, change a firewall or certify public reachability.

Installation ends by printing one short-lived `pendingcrew://enroll/v1/...` link. It is a sensitive one-time link, not a permanent administrator credential. Deliver it deliberately to the intended device; do not put it in process argv, logs or cloud directories. The device explicitly confirms the host identity, creates its own local private key, and performs the signed challenge/commit protocol. Each device receives a separate revocable grant. A directory entry or copied link never copies another device's authority. The link is distinct from the host's `tls://host:port` transport descriptor.

The installer saves original installation/configuration/enrollment UUIDs in root-private records before each mutation. If a response is lost, retain the operation record and query its original result; do not repeat installation or create another token automatically. Normal management uses the same installed CLI:

```sh
sudo pendingcrew status
sudo pendingcrew grants
sudo pendingcrew installation --request-id '<original install UUID>'
sudo pendingcrew configuration --operation-file '<original root0600 configure record>'
sudo pendingcrew recover --operation-file '<original root0600 enrollment record>'
```

`resume-installation --request-id <original install UUID>` explicitly continues only an already-current original activation: it rechecks the same user/tool prerequisites, repairs that original unit/binding if quiescent, starts the service, and queries any existing configuration/enrollment operation records. It does not extract/activate again or resend unknown writes. If no phase record existed, it may perform that phase once with UUIDs already stored in the original installation record. A notFound witness after a saved mutation record stops continuation.

`configuration` returns the original witnessed result, not fresh status presented as proof of a prior write. `recover` only looks up the original enrollment creation and can reproduce its same saved link; notFound does not authorize replay. If the link has expired, an operator may deliberately create a new link for a new device using `pair --operation-file '<new root-private file>' --host '<host>' --port 49381`. Existing operation files are never overwritten. Revoke uses the current original grant ID and revision (`revoke --grant-id ... --expected-revision ... --note ...`); if its response is uncertain, inspect that same grant instead of automatically resending the CAS.

`stop` requests the original bounded safePause lifecycle and stops socket activation only after its stopped receipt. It never force-kills a run or erases data. An unknown stop result is reported as unknown. Restart is an explicit operator service action; it is not task resume. Code rollback is also explicit: `rollback --request-id '<target original install UUID>' --expected-current '<current activation UUID>' --operation-id '<new rollback UUID>'`. Both the service and socket activation must already be stopped; exclusive prefix locks must be free; installation user/data binding, immutable release hashes, stopped SQLite application identity and writer format must match. Pending WAL/journal or a different format prevents rollback. Rollback neither recovers a database nor starts a service/task. No automatic rollback follows an uncertain activation.

Mac hosts use the existing App-owned Local Human management connection to issue the same link. This Linux root socket does not create a second Mac management role, grant ordinary same-UID callers Human authority, or authorize paired devices/Agents to issue links.

Source tests use private synthetic package headers, existing root-peer/remote-workflow seams and loopback TLS. They do not qualify actual Linux systemd deployment, public networking, signed distribution or provider inference; those retain their separate action and evidence gates.

A complete received management request may finish its single transaction after the caller disconnects while its original root process/birth and managed-lineage floor still qualify. EOF does not retract a received request or roll back an accepted write. A lost reply is Unknown and uses the original result lookup; it is never automatically resent. Explicit logical connection-scope closure is checked, but the single-request reader does not promise concurrent EOF observation while awaiting a database operation.
