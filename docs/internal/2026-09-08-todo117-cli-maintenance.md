# Todo #117：本机 CLI 检测与人工维护

## 交付行为

侧栏订阅额度的 Claude/Codex 两行旁各有版本入口。首次显示及每 10 分钟自动运行 `--version`；没装、输出无法解析、命令失败均显示警示，旧值明确标成上次成功检测。展开显示启动入口、最终二进制路径、检测时间、维护结果和完整命令日志路径。

只有人点“检查更新并升级…”并确认后运行该入口的 `update`。Claude 可留空、stable、latest 或完整数字版本；doctor 单独手动执行。Codex 原生安装可枚举本机同平台保留版本，验证候选二进制自报版本后原子替换 `current` 符号链接，并复验实际启动入口。`update` 返回 0 但复验失败不会报成功，版本没变不会宣称已最新。

## 升级前“没有 session 在跑”的判据

1. 按 runner 区分跨进程 flock。PendingCrew 从 `CrewSessionRunner.start` 开始持共享锁，直到该 run 退出；包括启动中、空闲、干活中和等审批，不按 isBusy 判断，也不按当前 crew 过滤。viewer/daemon 使用同一个本机锁目录。
2. 维护必须先取得独占锁，再通过本机 `ps -axo pid=,command= -ww` 查进程；发现同类 CLI 存活就拒绝，包含外部终端和 CLI 探针。列表读取失败、非 0、空或格式异常同样拒绝。
3. 独占锁一直持有到命令进程组退出、复验完成。PendingCrew 新 session 启动期间无法越过这把锁。命令无 shell 拼接，stdout/stderr 写入完整日志，显示末尾 32 KiB；15 秒版本探针、10 秒进程探针、30 秒 doctor、300 秒升级超时均报错，终止整个命令进程组。
4. 确认后再查实际路径和版本，和人确认时看到的安装不一致就拒绝执行。确认文案明确解释 Unix 语义：旧进程不会因符号链接切换当场崩，但新启动的用新版，可能两版并存，所以要先全停。

## 原始 400 故障

`CodexProtocol.sessionHealth` 原来要求 `codexErrorInfo` 是字符串，而且只认额度/登录两类。新版要求错误（含对象型 error info）落成 nil，回合结束后显示空闲。

现在新版要求进入 `cliVersionIncompatible`，保留原始错误并指向侧栏版本管理；其它终局回合错误进入 `turnFailed`，仍重试的普通错误不误报终局。沿现有 health 链显示异常并发群。`turn/start` RPC 失败也翻 health。成功回合清除已恢复的异常、重新武装群告警；失败回合不拿旧 assistant 正文冒充本轮完成。

## 版本解析复用

开工先查到两类既有实现：发版脚本的 `version_gt` / `asc-highest-build.py.version_key`，以及 Swift 的 `LocalCodingAgentExecutable.versionComponents`（node 目录排序）。本次接上已有 Swift 数字分段解析并收紧坏值处理，Claude/Codex 仅剥各自输出外壳；node 排序和 Codex 保留版本排序复用同一个数字解析器，没有新增第二份 Swift 数字解析。发版脚本的多段 build 号比较保留原状，未把 CLI 格式规则塞进发版链路。

## 测试与证据

- 先在原实现上运行 `testOutdatedCLIIsVisibleEvenWithObjectErrorInfo`：1 个测试、3 条断言失败，留全日志 `.test-archive/todo117-red-test.log` 和 `.test-archive/todo117-red.xcresult`。初次缓存权限失败另留 `.test-archive/todo117-red.log`，不算功能造红。
- 注入式覆盖两家版本格式、坏值、空闲外部进程、维护/启动互斥、进程扫描失败、升级失败、确认后安装变化、返回 0 但复验坏、未知安装拒绝、同平台回滚、候选实际版本、Claude 参数、无害命令大输出/非 0/子进程超时。全部升级命令输出为替身；回滚只改测试临时目录。
- XcodeGen 使用仓库要求的 2.46.0 缓存生成，新增 Swift 文件和 pbxproj 一起提交；没有更改本机 XcodeGen 2.45.4。
- 全量 macOS 测试完成情况与确切 SHA 另随群交付附完整日志。

## 边界、假设与可能的错误答案

- 维护只支持本用户默认原生布局：Claude `~/.local/share/claude/versions/<version>`，Codex `~/.codex/packages/standalone/current` 指向 `releases/<version>-<platform>/bin/codex`。brew、npm、自定义目录或其它平台只做可解析的本机检测，不替其包管理器升级；iOS、Windows、Linux、远端主机和跨机 CLI 不覆盖。
- 自动检测的是 PendingCrew 路径解析器会选择的本机 CLI，不查上游最新版本，不推断任意模型的最低兼容版本。多个安装并存时不会列出所有副本；PATH 登录 shell 结果进程内缓存，改变 PATH 后需重启应用/后台。终端 alias/function 可能与 PendingCrew 的实际启动路径不同；界面展示路径便于核对。
- 数字版本限三段，无 prerelease/build suffix；格式变化或混杂警告会显示检测失败，不能据此断言“未安装”。Codex 回滚列表只列相同平台后缀、存在可执行文件的保留版本，不保证旧版仍兼容当前模型、配置或服务端。
- 锁只约束采用这版 PendingCrew 的启动入口，不能阻止外部终端、CLI 自身自动更新器、旧版 PendingCrew 或其它管理工具在扫描之后启动/替换二进制。进程扫描匹配执行位置和常见 node 包路径；改名的 CLI、自定义 wrapper、含空格的非标准执行路径、进程隐藏/伪装可能漏检，同名其它程序或短命探针可能保守误拦。没有把一次扫描说成全机永久无竞态。
- 更新程序主动脱离进程组的后代不在终止组范围；超时说明安装可能部分完成，必须重新检测，未声称能事务回滚整个下载/安装过程。命令日志本机保留，当前没有自动清理 UI。
- 没有真的运行 `codex update` / `claude update`，没有更换人类本机 CLI、做 GUI 自动化或安装新 app；doctor 的真实 TTY 要求/输出尚未真人验证，若要求终端会明确失败/超时，不能把此项标成实际健康检查通过。
- 源码/自动测试证明不等于已装产品或群消息实机验收；仍需人在正常使用中验证版本入口布局、确认提示、错误群告警，以及在合适维护窗口验证真实升级。此报告不声称这些已完成。
