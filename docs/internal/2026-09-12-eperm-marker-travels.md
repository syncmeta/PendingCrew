# EPERM 续：拒绝跟着**文件**走，而且发生在权限层之上

基准提交：`6786d13`（main）。本文只记 2026-09-12 21:2x–21:3x 这一次故障窗口内
亲手量到的读数，和它们各自的对照。**没有**用到 sudo，没有 GUI。

## 0. 结论

1. **标记跟着文件走，不跟着目录、也不跟着 inode 出生地。**
   `clonefile(2)` 按路径克隆、不 `open()` 源文件、产出**新 inode**。
   克隆一个坏文件到 `/private/tmp` 下 → **克隆体照样 EPERM**；
   在同一个目录里克隆一个好文件 → **克隆体可读**。
2. **拒绝发生在 VFS 权限判定之上。**
   坏文件的 `ATTR_CMN_USERACCESS` 返回 **6 = `R_OK|W_OK`** ——
   文件系统认为调用者有读权限，然后 `open()` 仍然 EPERM。
   所以这不是权限位/ACL/属主的问题，是一个 MACF/sandbox 策略钩子。

## 1. 量法（可复核）

全部用 ctypes 直呼 libc，脚本在当时的 scratchpad 里，没进仓库：

- `clonefile(src, dst, 0)` 与 `clonefile(src, dst, CLONE_NOFOLLOW)`
- `open(path, O_RDONLY)` 判可读
- `getattrlist` 取 `ATTR_CMN_USERACCESS` / `ATTR_CMN_CRTIME`
- `getxattr(..., XATTR_NOFOLLOW)` 取 `com.apple.provenance` 的**值**
- `acl_get_link_np` / `st_flags` / `mode` / `uid`

## 2. 四行主表（同一趟）

| 文件 | open | crtime | provenance 值 | USERACCESS |
|---|---|---|---|---|
| 坏·源（whiteboards/…approvals.json） | EPERM | 09-11 17:53 | `010200bec7230769fff17c` | 6 |
| 坏·源的克隆（落在 /private/tmp） | **EPERM** | 09-11 17:53 | 同上 | 6 |
| 好·刚建（同一个 /private/tmp 目录） | 可读 | 09-12 21:29 | **同上** | 6 |
| 好·刚建的克隆 | **可读** | 09-12 21:29 | 同上 | 6 |

第二行是本文的全部价值：**一个我自己建的、躺在 /private/tmp 下的文件，
每一项可见属性都和它旁边那个可读的邻居一样，就是打不开。**
它比数据目录好用得多 —— 可丢弃、归我所有、跟 TCC 保护位置完全无关。

## 3. 这一趟排除掉的（每条都有对照，不是"看起来不像"）

- **出生时间**：仓库里 09-01 出生的文件可读；把一个刚建的可读文件用
  `setattrlist(ATTR_CMN_CRTIME)` 改到 30 天前，**仍然可读**。
- **provenance 的值**：可读的和读不了的是**同一个值**
  （`010200bec7230769fff17c`）。此前"排除 provenance"比的是**有没有**这个
  xattr，那是个便宜的代理量；这次比的是值，结论同向但支撑硬了一档。
- **mode / uid / gid / st_flags / ACL**：三个文件逐项一致，ACL 全都没有。
- **`clonefile` 本身有害**：好文件的克隆可读，所以不是克隆这个动作把文件弄坏的。

## 4. 与既有结论的关系

- 早先记的"跟着 inode 的出生地走"**不成立**，本文第 1 条推翻它。
- 早先那条**唯一有判别力的对照**（同一个二进制：daemon 能读、claude 起的
  helper 不能）仍然站着。它说策略也认**读的那个进程**。
  两条合起来：策略同时看**读者**和**被读的文件**，
  像是"这个进程不许读带某种来源标记的文件"。
- 剩下的一步没变：故障窗口内 `sudo scripts/capture-eperm-fsusage.sh`，
  看是哪个策略在拒。那一步要人。

## 5. 边界

- 本文所有读数取自**一次**故障窗口（2026-09-12 21:2x 起，当时仍开着）。
  非故障时刻**没有**量过克隆行为，所以"好文件的克隆可读"这一行是
  同窗口内的横向对照，不是跨窗口对照。
- `ATTR_CMN_DOCUMENT_ID` / `ATTR_CMN_GEN_COUNT` 在这台机器上返回
  `EINVAL(22)`，**没有比过** —— 它们不在排除名单里。
- 第 4 条里"策略同时看读者和文件"是**推论**，不是观测：
  本文没有在同一时刻拿 daemon 去读那个 /private/tmp 下的克隆体。
  那个实验能做，且会直接证伪或坐实它。
