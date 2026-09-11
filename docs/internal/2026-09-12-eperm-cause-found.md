# EPERM 故障：载体找到了，是 `com.apple.provenance`，罩的是目录

- 基准提交：`be98b9c`
- 现场时刻：2026-09-12 00:50–00:55，**故障正在发生时逐项量的**
- 前情：`docs/internal/2026-09-08-eperm-live-capture.md`（四次现场，成因未定）

---

## 一句话

**`~/Library/Application Support/PendingCrew*` 整棵树被 `com.apple.provenance` 罩住了。
在这个目录里，读被拒、写/stat/列目录/删除照常。范围是目录，不是文件，也不是进程。**

## 前一份记录被推翻/收窄的两条

| 旧记录说 | 这次实测 |
|---|---|
| 「跟着 inode 走、**每个进程**不一样」 | **不是每个进程**：全新的 `env -i /bin/sh` 得到一模一样的 EPERM |
| 「跟着 inode 走」 | **载体是目录**：在那个目录里**新建**一个文件，写得进、**读不回** |

## 逐项读数

### 允许 / 拒绝（与旧记录一致，这次再确认一遍）

| 操作 | 结果 |
|---|---|
| `stat` | ✅ |
| `getdirentries`（列目录，2691 项） | ✅ |
| `open(O_WRONLY|O_CREAT|O_TRUNC)`（新建写入） | ✅ |
| `unlink` | ✅ |
| **`open(O_RDONLY)`** | ❌ EPERM |
| **读扩展属性** | ❌ EPERM |

### 范围

| 路径 | 读 |
|---|---|
| `Application Support/PendingCrew/` 下全部 164 个 json | ❌ **全部** |
| `Application Support/PendingCrew/local-crews.json` 等上一层文件 | ❌ |
| `Application Support/PendingCrew-backup-20260912-001349/`（我今晚 `cp -Rp` 出来的副本） | ❌ |
| `Application Support/` 下另外 18 个 app 的目录（Claude / Codex / Cursor / Adobe …） | ✅ **全部正常** |
| 仓库里的文件 | ✅ |

### 决定性那一刀

在 `whiteboards/` 里新建 `.eperm-probe-<pid>.txt`：

```
① 写入   ✅ 成功
② 读回   ❌ EPERM          ← 新生儿一落地就被罩住 ⇒ 载体是目录
③ 属性   com.apple.provenance  11
④ 删除   ✅ 成功
```

同一把尺子在 `Application Support/Codex/` 上：写得进、**读得回**。尺子是活的。

### app 自己不受影响

daemon 此刻打开着 22 个该目录下的文件，群聊收发一切正常 —— 本轮所有 `post_to_crew`
（都是写）全部成功。**产品功能没坏，坏的是「从 app 之外读它的数据」。**

### 我的进程谱系（本来以为能解释，结果不能）

```
zsh ← claude ← PendingCrew(daemon) ← PendingCrew(GUI) ← launchd
```

**我是 PendingCrew 的后代，却照样被拒。** 所以「TCC 按 app 身份放行、我不是那个 app」
这个解释**不成立** —— 至少不是那么简单。这一条我没解释掉，明写在这儿。

## 成因：一个有证据支撑的假设，**不是结论**

`com.apple.provenance` 是 macOS 用来标记「这份文件归哪个 app」的扩展属性，
App Management / TCC 那一套据此判断「别的进程能不能碰另一个 app 的数据」。
读数与这个机制的形状对得上（读被拒、写放行、按目录传染、只罩这一个 app）。

**但我没有证实它**：
- 没抓到系统日志里那条拒绝（`log show` 近 3 分钟无命中，可能要更高权限）。
- **解释不了为什么同属 PendingCrew 进程树的我会被拒**。
- **解释不了为什么它是间歇的** —— 今晚 00:22 之前我反复读过这些文件，都成功。
  00:11 装了 0.1.34、00:24 后台换代，两者都在窗口之前，但这只是**时间上挨着，不是因果**。

## 下一次撞上要抓的

1. `sudo log stream --predicate 'subsystem == "com.apple.TCC"'` 开着，再触发一次读。
2. 量 `com.apple.provenance` 的值在**正常时刻**是什么（现在读不了 xattr，正常时读得了）。
3. 试 `xattr -d com.apple.provenance` 能不能解除（**别在真数据上试，先在探针文件上**）。

## 边界

- 以上全部是 2026-09-12 00:50–00:55 这一个窗口内量的。**没有非故障时刻的对照读数**
  （那正是这份记录之前一直缺的东西，这次仍然缺）。
- 「app 自己不受影响」是从「群聊照常收发 + daemon 有 22 个 fd 开着」推的，
  **我没有让 app 当场新开一个文件读一次**。已打开的 fd 不受后来的限制影响，
  所以这条推论比它看起来弱。
