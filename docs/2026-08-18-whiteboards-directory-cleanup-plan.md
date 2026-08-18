# `whiteboards/` 目录堆垃圾 —— 盘点与清理方案

> 2026-08-18。**本轮只调查、只出方案，一个文件都没删、没移、没改。**
> 下面每个数字都是当天在 `~/Library/Application Support/PendingCrew/whiteboards/`
> 上实测的（读操作 + 一份到临时目录的拷贝），复核命令都附在各节末尾。

## 0. 一句话结论

1051 个文件里，**必须常驻的约 700 个**（每 crew 的账本 + 还活着的成员的游标），
**本该清没清的约 341 个**（成员早已不存在的 `.cursor` / `.cursor.lock` / `.turn`），
**26 个 `.corrupt-*` 是 8-12 事故的现场证据、不是垃圾** —— 而且逐份验过：
它们**全都是合法 JSON**，内容**全部已经回到 live 文件里**，所以"能不能清"这件事
现在有了确定答案（可以，但该先移进事故归档目录、由人拍板再删）。

真正值钱的不是省那 7.5 MB 磁盘，而是：**主线程每秒把这个目录整个列 12 遍**
（见 §4）。文件数直接乘在那个开销上。

## 1. 现状盘点

```
总计 1051 个文件 / 7.5 MB
├─ 405 × *.lock                （全部 0 字节，只占 inode）
│   ├─ 320 × <crewId>.<sessionId>.cursor.lock   ← 随 session 增长，不回收
│   ├─  28 × <crewId>.todos.lock                ← 每 crew 一个
│   ├─  27 × <crewId>.approvals.lock            ← 每 crew 一个
│   └─  30 × <crewId>.lock / wakeups.lock / agent-sessions.lock
├─ 347 × <crewId>.<sessionId>.cursor            （36–57 B）
├─ 209 × <crewId>.<sessionId>.turn              （约 100 B）
├─  64 × *.json                                 （白板 / approvals / todos / captain-awareness / 全局账本）
└─  26 × *.json.corrupt-<unix毫秒>              （2.1 MB，最大单份 368 KB，**全部是 8-12 那天的**）
```

复核：
```
cd ~/Library/Application\ Support/PendingCrew/whiteboards
ls | wc -l; du -sh .
ls | sed -E 's/^local-[0-9a-f-]+//' | sort | uniq -c | sort -rn | head -20
ls *.lock | grep -c 'cursor\.lock$'
```

## 2. 逐类判定

| 类别 | 数量 | 判定 | 理由 |
|---|---|---|---|
| `<crewId>.json`（白板） | 28 | **常驻** | 群聊历史本体 |
| `<crewId>.approvals.json` / `.todos.json` / `.captain-awareness.json` | 10 / 8 / 10 | **常驻** | 每 crew 一份账本，数量以 crew 数为界 |
| `wakeups.json` / `agent-sessions.json` / `quota.json` / `models.json` / `crew-sessions.json` | 各 1 | **常驻** | 全局单份 |
| `<crewId>.lock` / `.todos.lock` / `.approvals.lock` + 2 个全局 lock | 83 | **常驻** | flock sidecar，每 crew 至多 3 个，**不随 session 增长**。18 个从来没有审批的 crew 也有 `.approvals.lock`，那是 `withFileLock` 用 `O_CREAT` 开锁文件的必然结果，不是泄漏 |
| `<crewId>.<sessionId>.cursor` | 347 → **211 常驻 / 136 可清** | **半数可清** | 见 §3 |
| `<crewId>.<sessionId>.cursor.lock` | 320 → **195 常驻 / 125 可清** | 同上 | |
| `<crewId>.<sessionId>.turn` | 209 → **129 常驻 / 80 可清** | 同上 | |
| `*.json.corrupt-<ts>` | 26 | **事故证据，先归档不删** | 见 §5 |

## 3. 那 341 个孤儿：判定条件是什么

`.cursor`（未读游标）/ `.turn`（回合记账）/ `.cursor.lock` 都是 **per (crewId, sessionId)**，
每起一个 session 就多一组，**从来没有任何一条路径去回收它们**
（全仓 `removeItem` 只有一处碰游标：`WhiteboardCursor.resync` 在"白板是空的"时删掉悬空游标，
与 session 生命周期无关）。

**不能按时间清**：`CrewSessionRunner.restartMember` 复用原 `sessionId`（身份/成员登记/白板
游标都靠它延续），所以一个 session 关掉几周后被重启，它的游标必须还在 —— 否则重启后
会把这几周的群聊当"未读"整份灌回去。

**能按归属清**：判据是 `local-crews.json` 里 `crews[].sessionMembers[].sessionId`。
成员被移除 / crew 被删 → 这个 sessionId 永远不会再回来（id 不复用），它的三个 sidecar 就是死的。

实测（28 个 crew / 211 个 session 成员）：

```
.cursor.lock  320  成员还在 195 / 孤儿 125
.cursor       347  成员还在 211 / 孤儿 136
.turn         209  成员还在 129 / 孤儿  80
```

复核脚本（只读）见本文末尾附录 A。

**`.cursor.lock` 要额外小心**：unlink 一个正被 flock 持有的锁文件，后来者会创建一个**新
inode**，两边就此各锁各的、互斥失效。所以锁文件只在"它对应的 session 已经不是任何 crew
的成员"时清，且只在**没有 session 在跑的时刻**清（见 §6 的"由谁清"）。

## 4. 为什么这事有性能后果（比省磁盘重要得多）

`CrewStore` 在**每一次目录事件**上排空三条控制通道，每条都 `contentsOfDirectory` 把
**整个目录**列一遍：

- `LocalCrewControlStore.drainRenames()`（改名）
- `LocalCrewControlStore.drainAttentions()`（黄点）
- `LocalCrewControlStore.drainCommands()`（机长命令）

目录事件已合流到最多 4 次/秒 → **每秒最多 12 次、每次 1051 个条目的全目录枚举，在主线程上**。
这条**与文件清理是两件事**：清到 700 个只是把常数降三成，设计上的根治是让这三个 drain
不再扫全目录（按后缀 + 指纹门控，或把控制文件挪进独立子目录）。**建议单独立一条，
别和清理混在一起做。**

## 5. `.corrupt-*` 是怎么来的 —— 别当垃圾判死刑

**来历确定**：2026-08-12 的 fd 耗尽事故。GUI app 从 launchd 继承的 `RLIMIT_NOFILE`
软上限只有 256，而这个目录 900+ 文件、本机数十个 PendingCrew 进程，定时唤醒那趟批量读
一次顶穿 → `open()` 返回 `EMFILE` → Foundation 把它包成
`NSFileReadNoPermissionError`（文案是"你没有权限查看此文件"）→ 上层当成"读不出来 = 大概率
坏了"→ 归档 + 从空重建。事故与不变式已经写进 `MultiProcessJSONStore` 的类型注释（④）。

**本轮新验到的两件事**（这决定了它们能不能清）：

1. **26 份归档全部是合法 JSON**，一份都解得开 —— 也就是说 26 次归档**全是误杀**，
   没有一次是真损坏。这是那条不变式的事后佐证。
2. **归档里没有任何 live 文件里没有的内容**：逐份比对消息 id，
   `archive-only = 0`（唯一例外是 `wakeups.json.corrupt-…` 里 1 条早已过期的唤醒）。
   所以历史确实已经找回来了，这 26 份**没有独有数据**。

时间戳全部落在 8-12 18:14–20:38 之间，与事故窗口吻合；之后再没有新增。

## 6. 方案：清什么 / 按什么条件 / 由谁清

### A. `.corrupt-*`（26 份，2.1 MB）—— 先"移出监听目录"，删与不删由人拍板

- **动作**：整体移进 `whiteboards/incidents/2026-08-12/`（保留原文件名与内容）。
- **好处**：一步同时解决两个问题 —— 证据留着，而且它们不再被每秒 12 次的全目录枚举扫到。
- **条件**：移之前对每份跑一次"能解析 + 每条 id 都在 live 文件里"的校验（附录 A 的脚本
  已经跑过一遍，全部通过）；有任何一份不通过就**停下来问人**，那份就是真丢过东西。
- **谁做**：**人**（一次性）。不做成自动清理 —— 事故归档天生是"要人看的东西"。
- **长期**：`MultiProcessJSONStore` 归档时直接写进 `incidents/<日期>/`，别再平铺进被监听的目录。

### B. 孤儿 sidecar（约 341 个）—— 程序清，但只在安全时刻

- **条件**（三个都要满足才删）：
  1. 文件名解析成 `(crewId, sessionId)`；
  2. `(crewId, sessionId)` 不在 `local-crews.json` 的 `sessionMembers` 里
     （crew 不存在也算不在）；
  3. **此刻没有任何 run 在跑**。
- **谁做**：
  - **主路径（根治）**：移除成员 / 删除 crew 的那一处，顺手删掉它的三个 sidecar ——
    谁产生谁负责，不留给扫描器。
  - **兜底**：app 启动时扫一次（那时必然没有 helper 子进程在写这个目录），
    把历史遗留的孤儿收掉。做成 `WhiteboardDirectoryJanitor`，一次扫描、纯判定可单测。
- **不做**：按 mtime 清。session 会被原 id 重启，按时间清会让重启后的第一轮把整段历史
  当未读灌回去。
- **量级**：341 个 → 目录降到约 710 个文件。

### C. 每 crew 的 lock / 账本（约 100 个）—— 不清

数量以 crew 数为界，不随时间增长。删了下次用到还会重建，净收益为零、风险不为零。

### D. 不属于清理、但同因同治的一条

§4 那个"每秒 12 次全目录枚举"。**单独立条**，别塞进清理里。

## 7. 本轮明确没做的事

- 没有删除、移动、改写 `~/Library/Application Support/PendingCrew/` 下的任何文件。
- 为了跑基准，把整个目录**拷贝**了一份到临时目录（只读地量），原目录未动。

---

## 附录 A：复核脚本（全部只读）

判定 `.corrupt-*` 有没有独有内容：

```bash
cd ~/Library/Application\ Support/PendingCrew/whiteboards
python3 - <<'EOF'
import json, glob, os, re
for f in sorted(glob.glob('*.corrupt-*')):
    base = re.sub(r'\.corrupt-\d+$', '', f)
    try: a = json.load(open(f))
    except Exception as e: print("PARSE-FAIL", f, e); continue
    live = json.load(open(base)) if os.path.exists(base) else []
    only = {r.get('id') for r in a if isinstance(r, dict)} - {r.get('id') for r in live if isinstance(r, dict)}
    print(f"{'OK ' if not only else 'KEEP'} {f}  archive={len(a)} live={len(live)} archive-only={len(only)}")
EOF
```

统计孤儿 sidecar：

```bash
cd ~/Library/Application\ Support/PendingCrew
python3 - <<'EOF'
import json, glob, os
crews = json.load(open('local-crews.json'))['crews']
pairs = {(c['id'], m.get('sessionId') or m.get('id'))
         for c in crews for m in (c.get('sessionMembers') or [])}
os.chdir('whiteboards')
for suf in ['.cursor.lock', '.cursor', '.turn']:
    fs = [f for f in glob.glob('*' + suf) if f.endswith(suf)]
    if suf == '.cursor': fs = [f for f in fs if not f.endswith('.cursor.lock')]
    live = dead = 0
    for f in fs:
        base = f[:-len(suf)]
        if '.' not in base: continue
        cid, sid = base.split('.', 1)
        live, dead = (live + 1, dead) if (cid, sid) in pairs else (live, dead + 1)
    print(f"{suf:14s} count={len(fs):4d}  成员还在={live:4d}  孤儿={dead:4d}")
EOF
```
