# 「账本读不出来」当场抓住了 —— 现场读数与成因

2026-09-08 19:44，机长（captain-0a5860fb）在故障当口量的。
**这是 `docs/internal/2026-09-08-ledger-read-failure-diagnosis.md` 里那个
「没能复现、EPERM 到底谁发的仍然不知道」的现场。**

## 一、实测到的（都在故障当口）

```
读 ~/Library/Application Support/PendingCrew/ 下任意文件  → errno=1 EPERM
  · <crew>.todos.json      EPERM
  · <crew>.json (白板)      EPERM
  · local-crews.json        EPERM
  · whiteboards/diagnostics/read-failures.log  EPERM   ← 痕迹文件自己也读不了

对照组（同一进程、同一时刻）
  · ~/.zshrc                         OK
  · <仓库>/CHANGELOG.md               OK

元数据全部放行：
  · listdir(whiteboards) → 能，2628 个条目
  · stat(todos.json)     → uid=501（我自己）mode=0644 size=204047
```

**形状：元数据放行、内容拦截，且只针对 PendingCrew 的数据目录。**
不是权限位（0644、属主是自己），不是文件缺失（stat 有 size），
不是整个 home 不可读（对照组 OK）。

## 二、成因（进程时间线对上了）

```
/Applications/PendingCrew.app/Contents/MacOS/PendingCrew  被替换于  19:04:01
新 GUI      pid 12177   起于 19:04:03      ← Sparkle 更新后重启了界面
daemon      pid 15229   起于 09-07 22:02   ← **早于替换 22 小时，没有被重启**
我这条链：  bash → claude(23126, 08:43) → daemon(15229)
```

**Sparkle 自动更新换掉了磁盘上的可执行文件，重启了 GUI，但没有重启 daemon。**
daemon 仍在运行那份**已经被替换掉的**二进制。macOS 对「运行中进程的可执行
文件已与磁盘不符」会作废其 TCC 授权 —— 于是**整条 daemon 谱系**（daemon 自己、
它的 agent 子进程、以及那些 `--mcp-serve` helper）读不了 app 自己的数据目录。

新 GUI 跑的是新二进制、授权是新的，所以它照常读写（`crew-sessions.json`
mtime 19:44 就是它写的）。

**这解释了那个 worker 留下的问题**：他写过
> 痕迹文件里的 `argv=` 一句话就能回答「是 agent 谱系被拒还是 app 自己也读不动」
> —— 这两个是不同的病。

**答案：是 agent 谱系被拒，app 自己没事。**

也解释了间歇性：**每次 app 自动更新之后、daemon 重启之前，这个窗口里所有
agent 都是哑的。** 之前那 11 次里有 2 次落在「休眠唤醒那一秒」，可能是同族
（唤醒后 code signature 复核）——**这条没验，别当结论。**

## 三、后果：消息在丢，而且一度是静默的

本 crew 白板 mtime 停在 **19:13:10**，而机长在 19:13–19:44 之间发了多条
`post_to_crew`，**每一条的回执都是「已发到 crew 群聊白板」**。

19:44 再发一条探针时，回执变成了诚实的失败：
> ERROR: 没能写进 crew 群聊白板 …… 原始记录已原地保留、本次一个字都没写
> …… 这条消息没有发出去，请当作未送达处理

**所以「已发到」这个回执在这段窗口里至少有一部分是假的。**
（哪几条真丢、哪几条只是没更新 mtime，本次没能逐条区分 —— 这一栏没做到。）

## 四、修法与遗留

**当场修法：完全退出 PendingCrew 再打开**（重启 daemon）。它会中断所有 session，
但那正是拿回授权的唯一途径。

**该修的产品缺陷（按要紧程度）**：
1. **自动更新后 daemon 必须重启**，或者至少要检测到「我的二进制已被换掉」并
   大声报出来。现在它安静地变哑。
2. **`post_to_crew` 在这段窗口里回过「已发到」而消息没落盘。** fail-loud 那条
   路径显然存在（19:44 那次就报对了），但更早那几次没报 —— 要查清是哪条分支
   吞了它。
3. **痕迹文件放在被保护的目录里，故障时自己也读不出来。** 它解决了「播报被吞」，
   没解决「查的人读不到痕迹」。考虑挪到不受 TCC 保护的位置（如 `~/Library/Logs/`）。

## 五、边界

- 「macOS 因可执行文件被替换而作废 TCC 授权」是**从时间线和现象推出来的**，
  没有直接观测到系统的授权撤销事件。要坐实需要看 TCC 日志或造一次实验
  （替换二进制但不重启进程，再读）。
- 19:13–19:44 之间**哪几条消息真的丢了**，没有逐条核对。
- 那 2/11 落在休眠唤醒的旧线索与本次是否同族，**未验**。
