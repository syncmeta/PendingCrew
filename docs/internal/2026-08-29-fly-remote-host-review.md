# Fly 远程主机接入 —— 现状复核与推荐架构（父 crew Todo #44）

**日期**：2026-08-29 ｜ **基线**：`main@d7ce988` ｜ **性质**：复核 + 架构建议，**不是实现计划**
**目标（人类原话转述）**：把 Fly 当**与本机平行的远程主机**，手机也能连；终端体验类似 SSH，但协议可另定。

> 按 `docs/internal/README.md` 的约定：这份写完就冻结，不随代码更新。读的时候请按
> 「2026 年 8 月 29 日，基于 main@d7ce988 是这么看的」来读。

---

## 0. 一句话结论

**Fly 接入不需要新协议，也不许重建刚被删掉的那两层。它是「前后端分离」那条 session 协议的第三种传输。**

今天真正卡住 #44 的不是 Fly，是**那条协议还没有任何一条真实的字节流传输**——P4（真进程分家）
未合 main。而且协议今天**扛不住真实字节流**（§2 A-2，读代码得出）。所以 #44 现阶段最有价值的动作，
是把「远程」这条约束钉进 P4 的验收，而不是另起一条管子。

---

## 1. 现状（逐条带证据）

### 1.1 Fly 在代码里只有两处，而且都是「形状」不是「能力」

| 位置 | 内容 |
|---|---|
| `Sources/Models/CrewSummary.swift:120-138` | `RuntimeLocation` 三档 `local_host` / `peer_device` / `fly_machine`，只带图标和短标签 |
| `Sources/Models/Machine.swift` | `kind: "computer" \| "fly"`、`flyMachineId: String?` |
| `Sources/Stores/CrewStore.swift:187-197` | `refreshMachines()` **硬编码恒一台本机**，`flyMachineId: nil`。它自己的注释写着「#63 第二期删掉跨端遥控整层之后**恒只有本机一台**」 |
| `Sources/Support/MachineGrouping.swift` | 已能按 N 台机器分组、已排好 fly 的次序、有单测 |
| `Sources/Stores/LocalCrewStore.swift:255-256` | 本地 crew **恒** `runtimeLocation: "local_host"` |

**结论**：侧栏按机器分组那一层是**现成且好用的**，`fly` 那一档已经预留。它下面**没有任何东西**
能产出第二台机器——`refreshMachines()` 返回的数组长度是常量 1。

### 1.2 两层已被**有意**删除，`#44` 不得复活其中任何一层

**（a）#63 第二期（2026-08-26）：跨端遥控 + 登录整层。**
人类原话（`docs/tech-debt.md:27` 那条下方引用，一个字没改）：

> 跨端遥控，端掉。以后前后端解耦时重新做

删掉的包括：`EdgeBackend`、`CrewRealtimeClient`、`CrewRelayAgent`、`serverLink` 那条链、
crew 详情页的「接入 PendingBot」、`AppModel` 凭据层 + Keychain。
`Sources/Services/PendingCrewBackend.swift:5-8` 现在写着：只剩 `LocalBackend`，**iOS 上
`AppModel.backend` 恒 nil**。全仓 `#if false` 零命中——是真删，不是注释掉。

**（b）#78（0.1.20，`bf07de8`，2026-08-28）：Workspace 同步层。**
11 个源文件 + 5 个测试：`SyncEngine`（planUp/executeUp）、`WorkspaceGit`、`WorkspaceManifest`
（TOML manifest）、`WorkspaceRepoLayout`、`WorkspaceRepoService`、`ProjectSyncService`、
`MachineRegistration`、`SyncReceipt`，外加 `WorkspaceSetupSheet` / `WorkspaceSyncView` 两个页面
和 `WorkspaceSyncStore`。`CHANGELOG.md:23` 记：「删除已经停用的跨机器 Workspace 同步层」。

> **这两层恰好覆盖了「远程主机」最容易被重新发明的两个方向：云中继、和应用层文件同步。
> #44 两个都不许走。** 这不是我的偏好，是上面两条已经落地的决定。

**（c）配套的方向决定**：2026-08-24 人类定「PendingCrew 不登录到任何地方」。所以 Fly 接入
**不得引入账号体系**。

### 1.3 `2026-08-10` 那份 iOS 通道设计**已经作废，别照它做**

`docs/internal/2026-08-10-pendingcrew-ios-driving-channel-design.md` 定案走「路线 A：relay 上的
结构化消息」，整份挂在「本地 crew 在服务端有一条 conversation」这个依赖上（它自己在最前面用
一整节声明了这个依赖当时**是空的**）。

那条依赖连同 relay 一起在 #63 第二期删掉了；2026-08-24 又定了不登录。**所以它的定案对今天不成立。**
它里面仍然值钱的是两段：§2「什么样的通道不会再造一次空管子」的判据，和「这是第三次面对同一个坑」
那段教训——**先把上层做完，才发现底下是空的**。#44 要靠这两段自我约束。

（同一份 spec 的 §「边界」一节里，`Todo #44 手机直连 fly 机器` 已经被点名过一次，当时被归到
「本地 crew 镜像上服务端」那条瓶颈下面。那条瓶颈现在随 relay 一起没了，所以 **#44 的前提整个换了**。）

### 1.4 唯一活着、且形状正确的地基：backend-split 的 session 协议（P0–P3 已在 main）

`docs/internal/2026-08-19-backend-split-design.md` 那条线，P0–P3 已经落在 main 上：
`ProcessRole`、`SessionHost`、`AgentSessionCore`、`TerminalMirrorView`、`SessionProtocol`
（长度前缀分帧 + codec）、`RemoteSessionBackend`、`TerminalSnapshotEncoder`、
`SessionStateReconciler`、`SessionReconnectPolicy`。

**它当初就是照着「以后要跑在网上」设计的**，两处白纸黑字：

- 设计 §4.1 选 UDS 的第三条理由：「**可延伸**：同一套分帧换个传输就能跑在网络上 —— 手机遥控是
  已知的下游消费者，我们不为它写代码，但也不给它砌墙。XPC 换不了。」
- `Sources/Mac/LocalRunner/InProcessTransport.swift:4`：「P4 只需把这里换成 socket，上面的
  codec / server / `RemoteSessionBackend` 不变。」

协议里已经定好、且**正好是 Fly 需要的**那几条：后台单向权威 + `stateSeq` 跳号拉全量（§4.3）、
attach 靠快照恢复而**不重放中断期字节**（§4.5）、能力靠 `hello.capabilities` 协商而不是版本号
（§4.4）、背压溢出走 resync（§5.4）。

### 1.5 但 P4 还没合 main（读代码）

全仓 grep 无 `UnixSocketTransport` / `socketpair` / `SMAppService`；`PENDINGCREW_BACKEND` 只在
`ProcessRole.swift:22` 出现一次（判定用），没有任何一处真的启动 daemon。
31 号 crew「常驻后台·前后端分离」挂着 P4 的四个 session（`31-9`~`31-12`），在飞行中。

---

## 2. 差距（按「离 Fly 有多远」分三类）

### A 类 —— 卡住整条线的（不解决，Fly 一行都不用写）

**A-1 · 没有任何真实字节流传输。** P4 未合 main。阻塞全部后续。

**A-2 · 协议今天扛不住真实字节流（读代码，白纸黑字，非推断）。**
两个 endpoint 的收包路径都假设「**一次投递 == 正好一整帧**」：

- `RemoteSessionBackend.swift:567-568`（server 侧）：`guard let message = try? codec.decodeApp(data) else { return }`
- `RemoteSessionBackend.swift:855-859`（client 侧）：`SessionFrameDecoder.decodeAll(data)` 且要求 `frames.count == 1`，随后同样 `try? codec.decodeDaemon(data) else { return }`
- `SessionProtocol.swift:469` / `:486` → `:503-509` `exactlyOneFrame()`：帧数不等于 1 就 `throw`

也就是说：**半帧到达 → 丢；两帧粘在一起 → 两帧都丢。不断连、不报错、不落日志。**
而正确的增量缓冲 `SessionFrameDecoder`（带 `buffer`、能处理半帧）就在 `SessionProtocol.swift:81-107`，
**只是没接到 endpoint 的收包路径上**。

`InProcessTransport` 每次投递恰好是一整帧（`sendFromApp` 直接把整个 `Data` 交给对端回调），
**所以这条今天不可能被任何现有测试照出来**。UDS 上偶尔会踩；WAN + TLS 上必然拆包粘包。
**这是 Fly 线上的头号静默失效候选**，而且症状形状正是本项目吃过大亏的那类（没有报错，就是收不到）。

**A-3 · endpoint 绑死具体传输类型。** 两个 endpoint 的 init 签名是
`init(transport: InProcessTransport, ...)`（`:479` / `:763`），不是 `SessionTransport` 协议——
`SessionTransport` 协议定义在 `InProcessTransport.swift:6` 但 endpoint 没用它。换传输必须改这两个 init。

> A-2 / A-3 都落在 31 号 crew 正在飞行的 P4 文件里。**它们应当成为 P4 的验收条件，而不是由本 crew
> 平行改一遍**——那正好是设计文档 §6 反复警告的「双头」。已用 `contact` 提给 `31-1`。

### B 类 —— Fly 专有

**B-1 · 远端机器上 claude / codex 的订阅登录态从哪来。（本条最硬，要人拍板）**
设计 §8.4 把「登录态悄悄降级、然后被误诊成后端问题」列为本项目**吃过大亏**的病，并因此把
「daemon 拉起的 session 能真正读到订阅登录态」写成 P5 不可省略的验收项。
Fly machine 是 Linux 容器：**没有 macOS 钥匙串、没有用户 TCC、没有交互式登录窗口。**
这不是工程细节，是产品决策——见 §6。

**B-2 · 没有机器名册，也没有机器身份。** `DeviceIdentity` 只有一个安装级 UUID（无密钥对），
注释还指向已删的 `POST /v1/machines/register-self`。`Machine` 模型的注释同样描述的是已删的
`GET /v1/machines`。要连远端主机，先得有「我认识哪几台、怎么证明我是我」。

**B-3 · 账本归属。** 白板 / Todo / 审批都是编排者进程写的本地 JSON（`LocalWhiteboardStore` /
`LocalTodoStore` / `LocalApprovalStore`）。第二台机器上的 crew，账本在第二台机器上。
**要么每台机器各自持有、viewer 聚合读；要么它就会长成同步层——而同步层刚被删（#78）。**

**B-4 · 供给（provisioning）为零。** 全仓没有 flyctl / Machines API 的任何一行；
远端镜像里要装什么（agent CLI、git、语言栈、字体/locale）没定过。

**B-5 · 远端工作目录里的代码怎么来。** #78 删掉的正是「应用层帮你同步代码」这件事。
**远端主机上的代码应当由 agent 自己 `git clone` / `pull` 拿到**——app 只搬字节流，不搬文件。
这条要写死，否则 `SyncEngine` 会以另一个名字重新长出来。

### C 类 —— 手机专有

**C-1 · iOS 上今天没有任何数据源。** `PendingCrewBackend.swift:8`：iOS 上 `AppModel.backend` 恒 nil。
把 Mac 的视图编译进 iOS target，得到的是恒空的漂亮界面——那正是「第三次空管子」。

**C-2 · 手机连 Fly 比手机连本机简单一个量级。** Fly machine 有公网地址；本机 Mac 在 NAT 后面。
**所以排期上必须先做「手机连 Fly」**，把「手机连本机」推到最后单独解（并且它可能根本不该由
#44 解决）。

---

## 3. 推荐架构

**一台机器 = 一个 `SessionHost` + 一条传输；viewer（Mac 窗口 / 手机）对哪台机器都说同一套协议。**

```
        ┌───────────── viewer ─────────────┐
        │  Mac 窗口 / iPhone               │
        │  RemoteSessionBackend × N 连接    │   ← 一个实现，连几台机器就是几条连接
        └───┬───────────────────────┬──────┘
            │ UDS（同机）            │ TLS 长连接（跨网）
            │ 同一套分帧             │ 同一套分帧
  ┌─────────┴──────────┐   ┌────────┴─────────────────┐
  │ 本机 SessionHost    │   │ Fly machine SessionHost   │
  │ （PendingCrew       │   │ （同一份编排逻辑，跑 Linux） │
  │   --daemon, macOS） │   │                           │
  │  claude / codex ×N  │   │  claude / codex ×N        │
  │  本机账本(JSON)      │   │  该机账本(JSON)            │
  └────────────────────┘   └───────────────────────────┘
```

### 四条纪律（每条都对着一个已经踩过的坑）

1. **不新造协议。** Fly 用的就是 §4 那套分帧 + capabilities 协商。
   「体验像 SSH」是**产品目标**，不是协议约束——终端字节走 kind=1、重连靠快照不靠字节重放，
   这两条已经定好了，正好就是 SSH 体验的关键。**借体验，不借协议。**
2. **不重建云中继。** viewer 直连机器。没有账号、没有服务端 conversation、没有服务端裁决。
   （对应 #63 与 2026-08-24 的方向决定。）
3. **不重建文件同步。** 远端工作目录里的代码由 agent 自己用 git 拿。app 只搬字节流。
   （对应 #78。）
4. **账本随机器走。** 每台机器是自己那批 crew 账本的**唯一写者**；viewer 聚合读，不双写、不合并。
   这是 §4.3「后台单向权威」的自然外推，也是不让同步层复活的结构性保证。

### 传输与身份（实现期定型，方向先钉）

- 传输：**TLS 长连接**（裸 TLS 或 WebSocket over TLS，实现时二选一），承载**完全相同**的
  `[u32 length][u8 kind][payload]` 分帧。
- 身份：**设备密钥对**（ed25519 / age，私钥存设备 Keychain，`synchronizable=false`），
  远端机器持 recipient 白名单。**不引入账号、不引入服务端签发。**
- 心跳/重连沿用 `SessionReconnectPolicy`；WAN 上的超时数值可能要放宽，但那是调参不是改设计。

---

## 4. 可执行阶段

| 阶段 | 做什么 | 完成判据 | 归属 |
|---|---|---|---|
| **P4′**（前置，**不归本 crew**） | P4 合 main；并把 **A-2 / A-3** 纳入验收 | 有一条真 socket 传输；**拆包/粘包测试为红→绿**；endpoint 面向 `SessionTransport` 而非具体类 | 31 号 crew |
| **F1** 传输上网 | 在 P4 的传输抽象上加 TLS 长连接 + 设备密钥对认证 | 同机 loopback TLS 跑通全部 P4 剧本；密钥不对 = 明确拒连（不是静默） | 本 crew |
| **F2** 机器成为复数 | 本地机器名册（可容 N 台，落盘）；crew 归属机器；`refreshMachines()` 不再是常量 | 侧栏出现第二台机器分组（`MachineGrouping` 已就绪）；断线的机器显式标注，**不静默空着** | 本 crew |
| **F3** Fly 供给 | 镜像（agent CLI + git + 运行时）、Machines API 拉起/销毁、地址下发 | 从 app 里起一台 Fly machine，`--daemon-status` 等价物能问出实况 | 本 crew |
| **F4** 远端凭据 | 按 §6 第 1 条人类拍板的方案落地 | **起一个远端 session，让它跑一次真需要登录态的动作并成功**（「进程起来了」不算过，照抄 §8.4 的判据） | 本 crew，**等拍板** |
| **F5** 手机连 Fly | iOS viewer：`RemoteSessionBackend` + 终端镜像 + 群聊/审批面 | 手机上看到并驾驶一个跑在 Fly 上的 session；**每一栏要么有真数据要么显式报错，不许空态**（对着「三次空管子」那条教训） | 本 crew |
| **F6** 手机连本机 | NAT 穿透 / 中转 | —— | **建议移出 #44 单独立项** |

排期纪律：**F1 → F2 → F3 串行**（每一阶段都是下一阶段的地基）；F4 与 F3 可并行（等拍板）；
F5 必须在 F1–F4 全绿之后才开工——否则又是一根空管子。

---

## 5. 现在（P4 未合 main 时）能做的最小地基

**能做的、且不与任何人抢文件的，只有下面三件；其余都要等 P4。**

1. ✅ **本文档**——把上面这些事实冻结下来，免得下一个人从已删的 relay/edge 架构重新推一遍。
2. ✅ **让代码停止描述一套已被删除的云架构**：`Machine.swift` 与 `DeviceIdentity.swift` 的
   注释仍在讲 `GET /v1/machines`、`POST /v1/machines/register-self`、`pendingbot.machine` 表、
   RLS、device-grant 登录——这些端点和表在 #63 第二期之后**都不存在了**。改成本地事实 +
   写明「第二台机器将从哪来」。**只改注释，零行为变化。**
3. ✅ **把 A-2 / A-3 入 tech-debt 账**，并提给 31 号 crew 作 P4 验收条件。

**明确不做的**：不建机器名册、不建传输、不碰 iOS——在没有传输之前，这些每一样都会变成一根
空管子，而这份文档整节 §1.3 就是为了不让它第四次发生。

---

## 6. 待人类拍板

1. **远端 agent 的订阅登录态怎么给**（最硬的一条，F4 卡在这）。三个选项：
   - **A. 远端只用 API key**（`ANTHROPIC_API_KEY` / OpenAI key 注入容器）——最简单、可自动化；
     代价是**远端那台不吃订阅额度，按量另外花钱**，且与本机的额度感知/降配那套逻辑分家。
   - **B. 把本机已登录的凭据材料投送到远端容器**（一次性、加密投递）——远端吃同一份订阅；
     代价是凭据离开本机钥匙串，且各家 CLI 是否允许多机并发使用需要逐个核官方条款。
   - **C. 远端做一次交互式登录**（device-code 流，人在手机/浏览器点一次）——最贴近"另一台你自己的电脑"；
     代价是每台新机器要人点一次，且要能把那个 URL 从容器里递到人眼前。
   - **我的倾向：C，退而求其次 A。** C 最符合「Fly 是与本机平行的一台你自己的主机」这个目标，
     也不把凭据搬出钥匙串；A 作为 C 跑不通时的兜底。**B 在核清各家条款之前不建议做。**
2. **Fly 账号与成本谁出、跑多大规格、闲置是否自动停机。** 这决定 F3 的形状（常驻 vs 按需拉起）。
3. **「手机连本机 Mac」（F6）是否留在 #44 里。** 我建议移出——它是 NAT 穿透问题，与 Fly 无关，
   混在一起会让 #44 永远完不成。

## 7. 参考

- `docs/internal/2026-08-19-backend-split-design.md` —— §3 架构 / §4 协议 / §8.4 登录态 / §9 分期
- `docs/internal/2026-08-10-pendingcrew-ios-driving-channel-design.md` —— **结论已作废**，
  只读 §2 判据与「三次空管子」那段
- `docs/tech-debt.md` —— #63 第二期那条（含人类原话与删除名单的修正）
- `CHANGELOG.md` 0.1.20 —— #78 删除 Workspace 同步层
