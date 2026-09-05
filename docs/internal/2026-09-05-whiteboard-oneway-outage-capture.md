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
