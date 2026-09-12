# EPERM：只有 PendingCrew 自己那个目录读不出来（2026-09-12）

基准提交：`b0d3f18`（写这份时的 main HEAD）。

## 这一趟新拿到的是什么

之前一天里所有读数都是「PendingCrew 数据目录读不出来」，**没有横向对照** ——
分不出「这台机器此刻对谁都这样」和「单挑我们」。这趟补上了对照。

同一个 shell、同一秒、同样的 `head -c1`：

| 目录 | 结果 |
| --- | --- |
| `~/Library/Application Support/Code` | 读得出来 |
| `~/Library/Application Support/Codex` | 读得出来 |
| `~/Library/Application Support/claude` | 读得出来 |
| `~/Library/Application Support/Cursor` | 读得出来 |
| `~/Library/Application Support/Google` | 读得出来 |
| `~/Library/Application Support/PendingCrew` | **全部 EPERM** |

PendingCrew 那一列不是抽样：目录下 2696 个条目，`*.json` 逐个试，**一个都读不出来**。

## 同时排除掉的

- 文件权限位：`-rw-r--r--`，属主 `hey`(uid 501) —— 就是我自己。
- ACL：`ls -le` 目录和文件都没有 ACE。上级 `Application Support` 只有一条
  `group:everyone deny delete`，与读无关。
- 文件没坏：`stat` 拿得到 size=2189；写、列目录、新建、unlink 全部照常。
- 不是沙盒：`/Applications/PendingCrew.app` 是 hardened runtime、有 Team ID
  `M42BKJN82S`，没有 App Sandbox entitlement。
- 责任进程不是「某个不相干的东西」：进程链是
  `PendingCrew.app(launchd) → PendingCrew → claude → zsh → 我`。

## 剩下的形状

被拒的**只有「读文件内容」和「读扩展属性」**（`xattr -l` 同样 EPERM），
而列目录、写、删都放行。这个不对称不是文件系统权限能产生的，
是策略层（TCC 那道「一个 app 的数据目录不给别的进程读」）的形状。

**边界（这份读数没做到的）**：

- 我没能直接看到 TCC 的判决 —— `log show --predicate 'subsystem == "com.apple.TCC"'`
  这 2 分钟里一条都没匹配到，所以「是 TCC」是从形状推的（第三层），不是实测到的。
- 我读不了 `TCC.db`（自己就没有 FDA），所以**也没法证明放行之后就一定好**。
- 「daemon 能读」这条是别的窗口留下的结论，这一趟**没有复核** —— 我这侧看不见它。

## 这条建议我收回（同一天，45 分钟后）

写这份的时候我在这儿写了「要人去系统设置里加完全磁盘访问权限」。**那条是错的，别照做。**

11:26:15 父 crew 那侧自愈，11:26:23 我这侧第一次读通 —— 差 8 秒，**两边都没有做
任何解除动作**。一次「加了权限然后好了」的因果在这种会自愈的故障上本来就没有
证据力，而这次连动作都没做就好了，所以它连巧合都算不上。

完整区间由父 crew 那侧量到：**10:41:56 → 11:26:15，44 分 19 秒**。这是一个
**时间窗**，不是一个待授权的权限状态。

推论也跟着换：它会自己回来，也会自己再来。所以要做的不是「去点开关」，
而是**窗口一开先把要读的东西搬出那棵树**（这次就是这么拿到两本账的，
见 `2026-09-12-my-ledger-snapshot.md`）。

## 这一趟砍掉的一整族猜测

窗口里我**自己新建了一个文件，写完立刻读回来，同样 EPERM**。

拒的是**路径**，不是文件。所以下面这些一次性全部不成立：

- provenance / 扩展属性 / 谁创建的 —— 我自己刚创建的也读不了。
- 属主、权限位、文件损坏 —— 那个文件是我一秒钟前写的。
- 「把数据拷到目录外面再读」这一整类绕法 —— **拷贝本身就要读**。

## 顺带一条该想的产品问题

agent 这侧所有账本都是**直接打开文件**读的。拥有那个目录的 app 自己读得到，
而它拉起来的 agent 读不到。如果把 MCP helper 的读路改成走 daemon（它本来就有
`daemon.sock`，只是现在只跑 session 进程管理），这一整类故障对 agent 侧就不存在了。
没动手，先记在这里 —— 这是要机长/人拍的方向，不是我顺手能改的。
