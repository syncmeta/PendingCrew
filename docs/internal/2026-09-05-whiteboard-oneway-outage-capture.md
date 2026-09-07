# 群聊「单向断开」——第一次抓到现场（2026-09-05 11:02–11:10+）

前三次都是事后查，查不出来，因为它会自愈。这一次是**挂着探针在断的当场量的**，
本文只记读数与判据，不记推测之外的任何结论。

三类分开写：**实测**（我跑了、亲眼看到输出）/ **读代码读到的** / **推的**。
本文除最后一节外全部是**实测**。

## 现场基本盘

- 时间：**11:02:53 断 → 11:10:48 恢复**（探针翻转行，约 8 分钟）
- 另一 crew（31 常驻后台）的探针：**11:02:17 断**，早 36 秒；两边同窗、不同秒
- 受害面：`~/Library/Application Support/` 下 **128 个目录里 23 个**读不了内容（105 个可读）
- 二进制普查（按 `comm` 数，不按命令行 grep）：`/Applications` **9** 个进程，
  `/tmp/pendingcrew-pkg` **0** 个

## 一、四种读法，同一个 errno —— 拦截在内核，不在 agent 层

| 读法 | 结果 |
|---|---|
| agent 自己的文件读工具 | `EPERM: operation not permitted, open '…/local-crews.json'` |
| `head -c 1` | `Operation not permitted` |
| `cat` | `Operation not permitted` |
| `python3 open()` | **`errno 1` (EPERM)** |
| `openat(dir_fd=…)` | **`errno 1`** |

`python3` 拿到的是**裸 errno 1**，不是任何分类器的字符串。
（判据来自：`Operation not permitted` = EPERM(1)，`Permission denied` = EACCES(13)；
POSIX 权限 / ACL 给的是后者。）

## 二、写能过、元数据能过，**只拦内容读**

- `echo probe > $SUP/.probe/x.tmp` → **成功**
- 紧接着 `head -c 1` 读**那个刚建好的文件** → **EPERM**
- `stat` → OK（112600, `-rw-r--r--`）；`listdir` → OK；`open(dir, O_RDONLY)` → OK

## 三、内核一条日志都没记（而尺子已证明会红）

- 断的当场 `log show --last 3m`：那棵子树 **0 条**
- 加 `--info --debug` 重查：仍然 **0 条**
- **尺子红过**：读 TCC.db → 日志当场出现
  `System Policy: head(19385) deny(1) file-read-data …`，命令行报的是**逐字相同**的
  `Operation not permitted`
- 机器上**没有任何 EDR / 第三方 Endpoint Security**（`systemextensionsctl list` 只有一个
  sing-box 网络扩展；无 Defender/Falcon/Santa 等进程）

## 四、被拒集合：23 项，而且不是一张固定名单

```
DENY  PendingCrew  PendingCrew-backup-0.1.15-…  PendingCrew-backup-0.1.16-…
      PendingCrew-databackup-…  PendingCrew-databackup-postupdate-0.1.20-…
      PendingCrew-installer  PendingCrew-rescue-…  PendingCrew-whiteboard-backups
      PendingBot-databackup-…  PendingNet
      com.pendingname.pendingbot  com.pendingname.pendingbot.revenuecat
      Microsoft Defender Shim  NetLogo  VirtualBuddy  VirtualInstallationService
      WorkBuddy  ZCode  com.tencent.imamac  com.tencent.workbuddy.mac
      sbtally  taobao  yuque-desktop
ok    Claude  Codex  Notion  Figma  Signal  Postman  Google  Microsoft  Adobe … （100+）
```

- 同一层在 **10:57 全绿时扫过一遍，当时 128/128 可读**；恢复后（11:12）用**同一行代码**再扫，**0 个 DENY**。三次扫描是同一行、一个字没改：
  `for d in */; do f=$(find "$d" -maxdepth 2 -type f -size +0 | head -1); … head -c 1 "$f"`
  —— **`-type f`，目录进不来**，所以不是「拿目录当文件读」那个假象（父机长自己的探针踩的正是那个，已撤回）
- **新建的目录也被拒**：`mkdir ZZTESTNeutral` + 写文件 → 写成功、**读 EPERM**
- 「同一目录里换没读过的文件」→ Claude / Codex / Notion 各挑 5 个**全部 ok**，
  PendingCrew 挑 5 个**全部 DENY** —— 所以不是「按单个文件缓存放行」

### 这把尺子的红样本与它的洞（补做，之前欠着）

- **会红**：把同一条判定喂给一个确知被拒的**普通文件** `com.apple.TCC/TCC.db`
  （`-rw-r--r-- 180224`）→ 判定输出 `DENY`。✅
- **洞（自曝）**：那次扫描**没有自带红样本** —— `find` 连
  `com.apple.TCC` 目录都列不动（`bfs: error: … Operation not permitted`），
  `$f` 为空就被 `continue` 跳过了。**唯一已知会红的那一项，被扫描器自己漏掉。**
  所以「23」这个数是可信的，但那一趟扫描当时并没有自证会红，红是事后补的。

## 五、被数据打掉的候选（都曾是我们倾向的，写下来免得再走一遍）

| 候选 | 打掉它的读数 |
|---|---|
| **按 crew 分隔** | 断的是**父 crew 之外的另一个 crew**（本 crew），且**对照组 31 的探针同一秒也在断** —— 两个不同 crew、两个不同时刻起的长驻进程，同断同持续 |
| **同 bundle id 两份签名** | 断的那一刻 `/tmp/pendingcrew-pkg` 进程数 **0**，只有 `/Applications` 9 个 |
| **agent 权限层 / 分类器** | `python3 open()` 拿到裸 `errno 1` |
| **目录名前缀（`PendingCrew*`）** | 中性名的新目录 `ZZTESTNeutral` 一样被拒 |
| **`com.apple.provenance`** | `PendingCrew` 与 `Claude` 两个目录的 provenance **逐字节相同**（`010200BEC7230769FFF17C`），一个拒一个通；`PendingNet` 与 `Codex` 同为 `010200D75B37FB5E7FC5F0`，一拒一通 |
| **ACL / flags / dataless** | `ls -leO@`：无 ACL、flags 栏 `-`、`local-crews.json` **一个 xattr 都没有**，照样被拒 |
| **按单个文件缓存放行** | 见第四节最后一条 |

## 六、只影响 agent 进程谱系，**不影响 app 自己**

断线期间只用元数据（`stat` 是通的）采样两次，间隔 30 秒：

```
11:10:04  crew-sessions.json  距今 2 秒
11:10:34  crew-sessions.json  距今 2 秒     ← app 每 ~30s 在写
```

**PendingCrew 自己在正常读写那棵子树，同一时刻我们这些 claude 谱系的进程读不了它。**

## 七、产品侧的实际后果（实测）

断线期间 `post_to_crew` 回执：

> ERROR: 没能写进 crew 群聊白板 —— 白板文件读不出来且归档失败（未能打开文件
> “local-21533801-….json”，因为你没有查看它的权限。），原始记录已原地保留、
> 本次一个字都没写 …… 这条消息没有发出去，请当作未送达处理。

**没有丢数据、也没有误报成功**（这一点是好的）。但同一时刻 `report_to_parent`
**可以送达** —— 与 8-27 那次受害者报的形状完全一致：**向上能通、本群不能通**。

## 八、推的（标清楚，未观测到直接证据）

两个探针都是**长驻 `sh`，先绿后红**。seatbelt profile 在 `exec` 时定死、不会中途变严，
**中途会消失的是 sandbox extension**。所以倾向：这些进程原本持有对那批路径的
extension，**被吊销、过一阵又发回**。这与另外量到的
`tccd: Failed to match existing code requirement for subject com.pendingname.pendingcrew`
（12h 内 **14** 次、横跨 **7** 个 service；同一 service 不同时刻答案不同）接得上 ——
身份验不过 → 授权失效 → extension 掉。

**但吊销事件本身没有被直接观测到。这一节是推的，不要当结论用。**

### 按父机长的建议换尺子重算，结果是**否定**

他提出：`deny file-read-data` 不是这一类拒绝的痕迹，那 14 条 `Failed to match
existing code requirement` 才是，要求拿它跟发作窗重新对齐。**照做了，对不上**：

- 14 条的时刻：00:32(×3) 00:52(×2) 00:53 01:17(×3) 03:17(×2) 08:45(×2) 09:20
- 本次发作窗：**11:02:53–11:10:48**
- 11:00–11:15 内 tccd 提到 `com.pendingname.pendingcrew` 的只有 11:00:23 一组
  正常 `AUTHREQ`（sandboxd 为新起的 claude pid 24331 做预检），**零条 failmatch**

换了他指定的尺子重算，结论不变：**这 14 条跟发作窗对不上。**

## 复现/继续观察

探针：`scripts/diag/whiteboard-access-probe.sh <tag> [间隔秒]`，日志
`/tmp/pc-access-probe/<tag>.log`。状态一翻转自动抓全现场；**恢复的那一刻也记** ——
「自愈」的宽度仍然是本案唯一还没量到的东西。

## 九、三方对表：三个 crew、三条谱系、同一个窗

第一次三边同时挂着探针，所以这组时刻是量出来的，不是拼出来的。

```
31-1（常驻后台）  断 11:02:17   半恢复 11:10:45   全恢复 11:10:51   （8 分 34 秒）
1-1 （本 crew）   断 11:02:53                     恢复   11:10:48
4-1 （父 crew）   断 ~11:02:0x                    恢复   11:11:16
```

### 9.1 恢复不是原子的 —— 逐对象，不只是逐进程（31-1 量到）

11:10:45 那一拍，有 **6 秒**里 `board`（真白板文件）和 `write`（在 `.probe/` 里建了
再读回）都通了，而 **`local-crews.json` 仍 EPERM**。**同一棵子树里不同文件先后解禁。**

这把粒度从「逐进程重新评估」再推细一层：**逐对象**。

### 9.2 一个还没有模型能同时罩住的矛盾

- **起点散**：11:02:17 / 11:02:53 / ~11:02:0x —— 跨度约 36 秒
- **终点聚**：11:10:45–11:11:16 —— 半恢复到最后一个全恢复挤在 31 秒内，
  其中三个「全通」时刻（11:10:48 / 11:10:51 / 11:11:16）里有两个只差 3 秒

**「每进程独立重新评估」解释得了发散，解释不了收敛为什么这么齐；
「一个全局开关」解释得了收敛，解释不了 36 秒的发散。**

**目前没有单一模型同时罩住两头。这个矛盾本身就是下一步该盯的东西 —— 不挑边。**
（一个候选：如果是逐**对象**重评估，三个进程量的是同一批对象，终点自然比起点收敛；
但这条也只是候选，**没有观测到重评估事件本身**。）

### 9.3 账本上的两条反证（保持反证身份，别被吸回去当机制）

1. **14 条 `Failed to match existing code requirement`**：换用它做时间轴重算，
   **本次发作窗内零条**。→ 「代码要求匹配失败 → 授权失效」**至少不是同步触发器**。
   31-1 已据此撤回他自己那半推测。
2. **`System Policy: deny(1) file-read-data` 那一串**（31-1 抓到，打在 AddressBook /
   CallHistoryDB / CloudDocs 等系统路径上）在 **11:00:23–26**，比三方失败起点都早
   2 分钟以上，**且不打在我们这棵子树上**。

**写故事的时候别把这两条拗回来当机制。** 它们现在的价值正是「此路不通」。

## 十、探针的两层自检（被这次现场逼出来的）

1. **红样本**（管样本）：每轮量一个**确知不可读**的普通文件（`com.apple.TCC/TCC.db`）。
   它变绿 = 尺子坏了，不是世界变了。
   起因见第四节末那个洞：唯一会红的那项被 `find` 列不动、`$f` 取空后 `continue`
   跳过，**从未进入样本集**，于是「全绿」看起来像「已核」。
2. **格数**（管整把尺子有没有接上）：每轮报「本轮实际量了几格 / py 回了几项」，
   对不上即判 BAD。同族真实事故：新测试文件没进 `.xcodeproj`，`-only-testing` 指了个
   不存在的套件，xcodebuild **不报错不警告、报 passed**，而 `Executed 0 tests` ——
   只钉 `0 failures` 和 `skip=3` 的判据罩不住它。

判据本身写进日志头（「每轮应量 10 格、py 应回 3 项」），**不能只活在脚本里**。

---

## 2026-09-06 16:36–16:38Z 又一次现场，这次量到了最硬的一条：**同一文件、同一时刻，别的进程写得动，我读不动**

发作是**自己撞上的**（`post_to_crew` 失败），不是探针报的。回执照 2026-09-06 的纪律 fail-loud：
「白板文件读不出来且归档失败（…你没有查看它的权限）。原始记录已原地保留、本次一个字都没写」——**没有把写失败伪装成成功**，也没有静默丢消息。

### 实测（都用 python 捕 `OSError`，读真 errno，不看 shell 退出码）

**故障面（我这个进程）：**
| 操作 | 结果 |
|---|---|
| `os.stat(白板.json)` | **OK** —— size=1934073, mode=0o100644, uid=501, nlink=1 |
| `os.listdir(whiteboards/)` | **OK** —— 2453 条 |
| `open(白板.json,'rb').read(1)` | **✗ errno=1 EPERM** Operation not permitted |
| `os.open(..., O_RDWR)` | **✗ errno=1 EPERM** |

**不是权限位、不是 EACCES(13)**：mode 0644、属主就是本人，报的是 **EPERM(1)**。

**范围（沿路径边界逐点量，不是「感觉是这一片」）：**
| 路径 | 结果 |
|---|---|
| `…/PendingCrew/whiteboards/*.json`（抽样 12 个，分属 12 个不同 crew） | **12/12 读不动** |
| `…/PendingCrew/whiteboards/*.lock` | 读不动 |
| `…/PendingCrew/local-crews.json`（**上一层**） | 读不动 |
| `…/PendingCrew/daemon.registry.json`（上一层） | 读不动 |
| `~/Library/Application Support/Claude/Preferences`（**别的 app**） | **可读** |
| `~/.claude.json` | **可读** |
| 仓库里的文件 | **可读** |
| `~/Library/Logs/PendingCrew/daemon.log`（**同一个 app 的另一个目录**） | **可读** |

→ **范围恰好是「app 数据根这一棵子树」**，既不是整个 `Application Support`（2026-09-06 早上我曾这么说过、当天已实测撤回），也不是这个 app 的全部文件（它自己的 Logs 目录好好的）。

### 最硬的那一条：**它不是文件坏了，是「我」被拒了**

`open` 被拒的同时，用还能用的 `stat` 连续采样 20 秒：

```
16:37:21  crew-sessions.json mtime=00:37:20 size=26743   目录 2453 条
16:37:25  …                  mtime=00:37:24
16:37:29  …                  mtime=00:37:28
16:37:33  …                  mtime=00:37:32
16:37:37  …                  mtime=00:37:36
16:37:41  …                  mtime=00:37:40   ← 期间 mtime 变了 5 次
→ 同一时刻我仍 EPERM
```

**同一个文件、同一段时间：daemon 每秒都在写，我一个字节都读不到。**

**这一条打掉的假设（不是推的，是这组读数直接排除的）：**
- ✗「文件/目录损坏」——它正被正常写入
- ✗「整台机器的该路径不可达」——另一个进程可达
- ✗「per-crew」——12 个不同 crew 的文件同时中招
- ✗「POSIX 权限位问题」——mode/属主正常，且报 EPERM 不是 EACCES

**剩下的方向（这句是推的）**：**路径作用域的沙盒授权，按进程发放，我这个进程（或它这条进程树）手上那份失效了，而 daemon 手上那份仍然有效。**与既有的「长命进程从绿变红、沙盒 profile 在 exec 时固定、所以只能是 extension 中途消失」一致，并**补上了此前缺的对照组**：同一时刻的另一个进程是好的。

### 下一次该抓什么（写给下一个撞上的人）

1. **别只量自己**：立刻用 `stat` 看同一文件的 mtime 是否仍在推进 —— 这是区分「文件坏了」和「我被拒了」的最便宜的一刀，且 `stat` 在故障期仍可用。
2. **沿路径边界逐点量**，不要笼统说「这一片读不了」：本次的边界精确到 app 数据根，连同一个 app 的 Logs 目录都在界外。
3. **errno 要用 `open()` 捕 `OSError` 读**，不要用 `cmd && echo OK || echo FAIL` —— 退出码不承载 EPERM/EACCES 的区别（2026-09-06 已因此误判过一次）。

### 补正（同一次故障，来自两条独立探针）——**「单向」这个词要改口径**

探针把这一次记全了，并给出**第二个独立来源**，与上面手采的一致（`stat=OK` 而 `head/cat=FAIL`，errno 1）：

```
08:01:49+0800 持续断 | stat=OK head=FAIL cat=FAIL write=FAIL(readback) board=FAIL
                       ctrl_claude=OK ctrl_repo=OK otherapp1=OK otherapp2=OK [红样本正常]
08:02:20+0800 翻转   | 全部 OK
```

**两条要改的：**

1. **`write=FAIL(readback)` —— 写也被拒，不只是读。**所以这次故障对被拒的那条进程是**双向**的。上面「daemon 每秒在写、我读不到」这条读数仍然成立，但它说明的是**「daemon 那条进程没被拒」**，**不是「写通道没事」**。「单向断开」这个沿用了几天的说法，到此为止；准确的说法是 **按进程的、路径作用域的、双向的拒绝**。

2. **恢复时刻钉住了：`08:02:20+0800`（= 00:02:20Z）。**此前只有「7 分钟还没恢复」这样的下界。有了确切终点，才可能去对齐系统日志找触发源。

（本文上一节写的「7.4 小时」是**故障起点到我能读**的跨度上界；那段时间机器休眠过，不等于连续故障 7.4 小时。探针的连续记录才是判断连续性的依据。）
