import Foundation

/// 本地 crew-comms 的 MCP server 逻辑（spec local-first chunk 4）。claude 经
/// `--mcp-config` 把 `pendingcrew-mcp serve` 拉成子进程，session 用 `post_to_crew`
/// 往本地白板发、`read_whiteboard` 读 —— 全部落到与 PendingCrew app 同一份
/// `LocalWhiteboardStore`。MCP spike 已验 auto mode 自动放行（findings 文档）。
///
/// `handleLine` 是纯函数式 dispatch（一行 JSON-RPC → 应答 JSON 字符串 / nil），
/// 不碰进程/stdio —— 这样能编进 PendingCrewTests bundle 单测。stdin/stdout loop
/// 在 `main.swift`。用 `JSONSerialization`（不给每个 RPC 形状写 Codable）。
final class McpServer {
    let store: LocalWhiteboardStore
    let approvals: LocalApprovalStore
    /// crew 元数据控制通道（机长 `rename_crew` → 写待改名，app 侧 `CrewStore`
    /// 排空落地）。与 store/approvals 同 `--dir` —— 离线 helper 唯一能碰的共享文件层。
    let control: LocalCrewControlStore
    let crewId: String
    let sessionId: String
    /// 机长 session 标记（helper `--captain` flag 传入）—— 解锁机长专用工具
    /// `answer_decision`（chunk2 T4）。worker session 看不到也调不动。
    let isCaptain: Bool
    /// 本 session 的显示 label（helper `--label` flag 传入；如「机长」/「Claude Code
    /// · abc123」）。`post_to_crew` 写白板时带上 → agent 看的白板不再裸 uuid。
    /// nil（未传 label / 旧调用）→ 渲染退回 `session:<id>`，保持兼容。
    let sessionLabel: String?
    /// quota.json 所在目录（app 的 `QuotaCenter` 定时写、`get_quota` 工具读）。
    /// 与 store/approvals/control 同 `--dir`。
    let quotaDirectory: URL
    /// **Agent 的**那本 Todo（`TodoLedger.agent`，task #478）：人类派活，机器人经
    /// `respond_todo` 追加回应 + 推进状态；新增条目只有人类能做（app 面板），
    /// MCP 不暴露新增。与 store 同 `--dir`。
    let todos: LocalTodoStore
    /// 机长作战板（人类 Todo #66）。`plan_add` / `plan_update` 这两个**工具**仍然只给
    /// 机长（worker 连工具列表里都看不到）；但 worker 现在能通过 `post_to_crew` 标
    /// `progress` / `blocked` 往板上写 —— 见 `CrewCockpitWritePermission`。与 store 同 `--dir`。
    let plans: CockpitPlanStore
    /// 定时唤醒账本（`LocalWakeupStore`）。helper 这边**只读它一次** —— 判断
    /// 「这条计划是不是已经挂着督办」，好把「顺延」当场拒掉。真正的登记/触发/
    /// 清账仍在 app 侧 runner，这里不写。与 store 同 `--dir`。
    let wakeups: LocalWakeupStore
    /// **人类的**那本 Todo（`TodoLedger.human`，Todo #62）：方向反过来 —— agent 经
    /// `add_human_todo` 提条目请人拍板，人类在 app 里回应。与 store 同 `--dir`。
    let humanTodos: LocalTodoStore
    /// 机长核账的那点持久态（`confirm_todo_sweep` 写）。**必须注入、不能用
    /// `.shared`**：`.shared` 指着真实数据根，单测一跑就把确认写进人的数据目录，
    /// 而用例自己读的是临时目录 —— 看起来像「没落账」，实际是落到别人家去了。
    let sweeps: CaptainTodoSweepStore
    /// Current-turn one-shot continuation promises. Unlike Todo/plan ledgers this
    /// is executable control state and is scoped to this exact session turn.
    let continuations: SessionContinuationStore
    /// 本 session 跑在哪家 runner 上（helper `--agent claude|codex`）。
    /// `set_session_profile` 拿它挑对照哪张模型表；nil（旧调用/没传）→ 两家都对照，
    /// 任一家认得就不吭声（宁可少说，也别对着错的表瞎报）。
    let agentKey: String?
    /// 群聊附件落盘根目录（`post_to_crew(attachments:)`，Todo #48）。默认真实数据
    /// 目录 `Application Support/PendingCrew/attachments/` —— 与人类 composer 发的图
    /// **同一个目录、同一套命名**，所以两边发的图在气泡里长得一样、清理时也是一处。
    /// 不放 crew 工作目录：worktree 被清掉会把图带走，历史气泡就渲染不出来了。
    /// 单测传临时目录（走同一条生产代码路径）。
    let attachmentRoot: URL
    /// agent 侧会话号账本（`LocalAgentSessionStore`）。`list_sessions` 靠它把
    /// 我们自己的 sessionId 翻成 runner 的会话号 —— 有会话号才知道该去哪儿找
    /// 那份成绩单。与 store 同 `--dir`。
    let agentSessions: LocalAgentSessionStore
    /// 产出证据取证面（人类 Todo #107 第三件）。默认真实的 `~/.claude/projects`
    /// 与 `~/.codex/sessions`；单测喂假目录走同一条生产代码路径。
    let outputProbe: SessionOutputProbe

    /// `list_sessions` 的工具描述。抽成常量是为了让单测直接盯住它 ——
    /// 「产出证据这一列在什么情况下说不出话」必须写在这里，机长读到
    /// 「看不出来」时才不会把它当成「没干活」再犯一次同样的病。
    static let listSessionsToolDescription = """
        （机长专用）点名：列出本 crew 全部 session 成员的实时状态（干活中/空闲/异常/已退出 + 各自任务），        并给每一行附一列**产出证据**。派活前先点名——有空闲的合适成员就 @ 它接手,别急着 start_session         起新人;有异常的（未登录/额度）先处置或上报。
        产出证据这一列回答的不是「它显示什么」，而是「它最近真的写出过东西吗、什么时候」——读的是 runner         自己留下的会话成绩单（claude 的 ~/.claude/projects/**/<会话号>.jsonl、codex 的         ~/.codex/sessions/**/rollout-*-<threadId>.jsonl）最近一次写入的时刻。状态是会骗人的：显示「空闲」        而任务书压根没提交过的 session，状态那一列看不出来，产出那一列会明说。
        三种口径，别混：
        · 「最近产出 X 前」= 真写过东西，X 是最后一次写距今多久。显示空闲、产出却停在一小时前 → 多半卡住了，        用 inspect_session 看现场。
        · 「确实没有产出」= 会话号我们记着、该找的地方找过了，一个字都没写过。
        · 「产出看不出来」= **取证面自己不在场**（还没记下会话号 / runner 不是 claude 或 codex / 那两个目录        读不出来）。它**不等于**「没干活」，只是这条路问不出答案；把它当成「卡住了」去判断，就是把表象当        状态的老毛病换个地方再犯一次。codex 要握手成功才有 threadId，所以 codex 成员刚起来的头几秒本来        就是「看不出来」。
        **这一列本身也只是一个证据，不是结论**：它证明的是「一个字没产出」，**不是「卡住了」**。        要跟状态、跟「距今多久」一起读才下得了判断 —— 空闲 + 刚刚产出 = 正常待命；空闲 + 产出停在一小时前         = 值得去看现场；干活中 + 确实没有产出 = 它连第一句话都没提交上去。**把这一列单独拎出来当状态用，        就是把表象当状态的老毛病换了个地方犯。**
        """

    init(store: LocalWhiteboardStore, approvals: LocalApprovalStore, control: LocalCrewControlStore,
         crewId: String, sessionId: String,
         isCaptain: Bool = false, sessionLabel: String? = nil,
         quotaDirectory: URL? = nil, todos: LocalTodoStore? = nil,
         plans: CockpitPlanStore? = nil,
         wakeups: LocalWakeupStore? = nil,
         humanTodos: LocalTodoStore? = nil,
         continuations: SessionContinuationStore? = nil,
         sweeps: CaptainTodoSweepStore? = nil,
         agentKey: String? = nil,
         attachmentRoot: URL? = nil,
         agentSessions: LocalAgentSessionStore? = nil,
         outputProbe: SessionOutputProbe? = nil) {
        self.store = store
        self.approvals = approvals
        self.control = control
        self.crewId = crewId
        self.sessionId = sessionId
        self.isCaptain = isCaptain
        self.sessionLabel = sessionLabel
        self.quotaDirectory = quotaDirectory ?? LocalWhiteboardStore.defaultDirectory
        // **没显式传就跟着注入的白板目录走**，别静默退回真实数据根 ——
        // 跟下面 `attachmentRoot` 那条注释说的是同一个暗线。这条不是假设出来的：
        // 分类落账的第一版用例没注入 `todos`，于是往**真的 app 数据目录**写进了
        // 三条 Todo，而用例自己读 fixture 读到 0 条 —— 看起来像「没落账」，
        // 实际是**落到别人家去了**。驾驶舱这本账现在也走同一条路，同样得跟。
        self.todos = todos ?? LocalTodoStore(directory: quotaDirectory)
        self.plans = plans ?? CockpitPlanStore(directory: quotaDirectory)
        self.wakeups = wakeups ?? LocalWakeupStore(
            directory: quotaDirectory ?? LocalWhiteboardStore.defaultDirectory)
        // 两本账落在同一个 `--dir` 下，只是文件名不同（见 `TodoLedger.fileSuffix`）。
        // 没显式传就照着 agent 那本的目录开一份 human 的 —— 别让调用方漏传一个
        // 就静默退回默认目录（helper 的 `--dir` 不是默认目录）。
        self.humanTodos = humanTodos ?? self.todos.sibling(.human)
        self.continuations = continuations ?? SessionContinuationStore(
            directory: quotaDirectory ?? LocalWhiteboardStore.defaultDirectory)
        self.sweeps = sweeps ?? CaptainTodoSweepStore(
            directory: quotaDirectory ?? LocalWhiteboardStore.defaultDirectory)
        self.agentKey = agentKey
        self.attachmentRoot = attachmentRoot
            ?? Self.defaultAttachmentRoot(whiteboardDirectory: quotaDirectory)
        self.agentSessions = agentSessions
            ?? LocalAgentSessionStore(directory: quotaDirectory ?? LocalWhiteboardStore.defaultDirectory)
        self.outputProbe = outputProbe ?? SessionOutputProbe.onThisMachine()
    }

    /// 没显式传附件根时用哪儿。
    ///
    /// **注入了白板目录就跟着它走** —— helper 的 `--dir` 是 `<数据根>/whiteboards`，
    /// 附件就该落在同一个数据根的 `attachments/` 下。老实现无条件取
    /// `CrewChatAttachmentStore.defaultDirectory`（= 真实数据根），于是
    /// `PENDINGCREW_DATA_DIR` 把整个数据根挪走之后，**附件仍然写回真实数据根** ——
    /// 数据根三条来源里唯一一条谁都不走的暗线（2026-09-05 发包前审计逮到）。
    ///
    /// 没注入（app 进程自己用）才回到真实数据目录：那时附件要和人类 composer 发的图
    /// 同目录同命名，见 `attachmentRoot` 的声明。**别把这半也「修」掉。**
    private static func defaultAttachmentRoot(whiteboardDirectory: URL?) -> URL {
        guard let whiteboardDirectory else { return CrewChatAttachmentStore.defaultDirectory }
        return whiteboardDirectory.deletingLastPathComponent()
            .appendingPathComponent("attachments")
    }

    /// 处理一行 JSON-RPC。返回应答 JSON 字符串；通知（无 id / `notifications/*`）→ nil。
    func handleLine(_ line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty,
              let data = trimmed.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        else { return nil }

        let method = obj["method"] as? String
        let id = obj["id"]

        if let method, method.hasPrefix("notifications/") { return nil }

        switch method {
        case "initialize":
            return result(id: id, [
                "protocolVersion": "2024-11-05",
                "capabilities": ["tools": [String: Any]()],
                "serverInfo": ["name": "pendingcrew", "version": "0.1.0"],
            ])
        case "tools/list":
            var tools: [[String: Any]] = [
                [
                    "name": "post_to_crew",
                    "description": "把关键节点发到 crew 群聊白板（只发要紧的：开始/完成/卡住/交接/重要发现，别倒 IO 日志）。可带 mentions 定向 @ 某个 session/captain/人类，或 reply_to 回复某条（自动 @ 原发送者 —— 是「广播 + 叫醒他」，不是私信他）。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "message": ["type": "string", "description": "发一条时用它。要一次发几条用 `messages`，两个别同时给。"],
                            "messages": [
                                "type": "array",
                                "description": "**一次发多条**，每条一个气泡、各带自己的分类。一次汇报别揉成一条 —— 拆成「进度 / 要你拍板的 / 要问的」几条，各落各的账。\n最多 6 条（不是性能限制：一次十几个气泡只是把一堵墙拆成一排墙）。\n**校验是全有或全无**：任何一条不合法，整批都不发，错误信息会指名是第几条。\n例：`[{\"text\":\"闸门全绿\",\"category\":\"progress\",\"plan\":3},{\"text\":\"这条要你拍板\",\"category\":\"human_todo\"}]`",
                                "items": [
                                    "type": "object",
                                    "properties": [
                                        "text": ["type": "string"],
                                        "category": ["type": "string"],
                                        "plan": ["type": "integer"],
                                        "blocked_by_number": ["type": "integer"],
                                        "blocked_by_ledger": ["type": "string"],
                                        "headline": ["type": "string"],
                                        "todo": ["type": "integer"],
                                        "todo_status": ["type": "string"],
                                        "evidence_commit": ["type": "string"],
                                        "evidence": ["type": "string"],
                                    ],
                                    "required": ["text"],
                                ],
                            ],
                            "headline": ["type": "string", "description": "**一句话说清「结果是什么」** —— 超过 8 行的消息在群里**默认收起**，收起态就只露这一行，人看不看正文全看它。\n\n**写结果，不写动作：**\n✅「闸门全绿，0.1.34 可以发」\n✅「病根是守卫问错了对象，已修，等接线」\n✅「这条要你拍：A 拒收 / B 降级，我倾向 B」\n❌「关于折叠标题的一些进展」（说了等于没说）\n❌「我改了 CrewMessageFold 和 McpServer」（这是过程，不是结果）\n❌ 把整段摘要粘进来（收起态只露一行，多的会被截掉）\n\n长度：约 60 个半角宽 ≈ **30 个汉字**，超了截断加省略号。\n**不给的话界面只能猜**（取正文前 3 段里第一个加粗）—— 猜出来的常常是句子中间某个强调词。真没结论可写，多半说明这条消息本身不该这么长。\n分条发送时它是**每条自己的**（写在 `messages` 里那一条上，顶层给会整批拒）。"],
                            "category": ["type": "string", "description": "这条该落进哪本账（不是「它讲什么」）。落账的：`human_todo`(要人拍板) / `todo_response`(回应派下来的活) / `plan`(要开始做一件事) / `progress`(某条计划推进了，要 `plan` 号) / `blocked`(卡住了，要 `plan` + `blocked_by_number`) / `done`(完成了，要 `plan` 号)。不落账的：`handoff`(交给谁了，只记录、不起进程) / `ack` / `question` / `finding` / `note`。不给 = 不落账。"],
                            "todo": ["type": "integer", "description": "这条对应哪条 Agent Todo 的 #N。**给了就必须同时给 `todo_status`** —— 挂上号却不更新状态，账还是旧的。跟 `category` 正交：一条消息可以既是进度、又对应一条 Todo。"],
                            "todo_status": ["type": "string", "enum": LocalTodoStore.statusOrder, "description": "配合 `todo` 用。翻 `completed` 必须带 `evidence_commit`（会当场解析）或 `evidence`。"],
                            "plan": ["type": "integer", "description": "配合 `progress` / `blocked` / `done` 用：驾驶舱里那条计划的 #N（plan_list 看得到）。\n**这三类会真的写进驾驶舱那本账**：`progress` 追加一条进展（板上那条是「没做」时顺手翻成「进行中」，是「卡住」时**不动** —— 报进度不等于解了卡）；`blocked` 翻卡住并挂上卡点；`done` 翻完成。\n`plan`(新增一条计划) 和 `done`(翻完成) **只有机长能做** —— 板上有哪些条目是机长的编排权（也是防淹），而完成是验收判断、不是自我声明。worker 报 `progress` / `blocked` 照常。"],
                            "blocked_by_number": ["type": "integer", "description": "配合 `blocked` 用：卡在哪条 Todo 的 #N。**「卡住」的意思就是卡在人身上** —— 不指出是哪一条，人看到板也不知道该推什么。"],
                            "blocked_by_ledger": ["type": "string", "enum": ["human", "agent"], "description": "配合 `blocked_by_number` 用：哪一本 Todo 账 —— human（你请人类拍板那本，默认）/ agent（人类派给你那本）。两本各自从 #1 起，裸 #N 有歧义。"],
                            "evidence_commit": ["type": "string", "description": "翻 `done`（计划完成）或 `todo_status: completed` 时的凭据：产出所在的 commit（7–40 位十六进制）。**会当场在本 crew 登记的工作目录里解析**，解析不出来就拒绝，并且**这条消息也不会发出去**。"],
                            "evidence": ["type": "string", "description": "产出不是 commit 时的凭据：一句话写清是什么（跑了哪趟全量、读数多少）。与 `evidence_commit` 二选一。"],
                            "mentions": [
                                "type": "array",
                                "description": "可选定向 @ 列表 —— 要某个具体对象接手/回应时带上；不填=广播给全 crew。@session / @captain 会**收窄可见范围**：只有被点到的 agent 看得到，并把这条投进它的定向信箱（它优先看到）。@human 不收窄 —— 它只是「这条是讲给人听的、别为它叫醒 agent」的标记，消息对全 crew 照常可见。@broadcast 是**显式放宽器**：和 @session/@captain 一起给（如 `[{kind:\"broadcast\"},{kind:\"session\",target_id:\"…\"}]`）= **全组都看得见、但只叫醒被点到的那个**；别人的注入面上那条会标「（发给 XX 的）」，看得见也看得出不是给自己的活。单独给 @broadcast 等于不填。",
                                "items": [
                                    "type": "object",
                                    "properties": [
                                        "kind": ["type": "string", "enum": ["session", "captain", "human", "broadcast"]],
                                        "target_id": ["type": "string", "description": "kind=session 时必填：目标 session 的 id。"],
                                    ],
                                    "required": ["kind"],
                                ],
                            ],
                            "attachments": [
                                "type": "array",
                                "description": "可选：随这条消息一起发到群里的图片/文件，填**本机绝对路径**（`~` 可用）。图片在群聊气泡里直接显示，其它类型显示成文件条。收到的人（包括别的 session）拿到的是可以直接 Read 的绝对路径 —— 所以截图、生成的图表、报告文件都可以这样递过去，不用把路径写在正文里让人自己拼。\n\n**你的原文件不会被搬走**，收进群聊的是一份副本（存在 app 数据目录，随聊天记录长期保留，worktree 清掉也还在）。\n\n收不下的会**逐条**在回执里说明原因（文件不存在 / 是文件夹 / 超过大小上限），不会静默丢；带了附件时正文可以为空（只发图）。",
                                "items": ["type": "string"],
                            ],
                            "crew_status": ["type": "string", "description": "（机长专用）一句话说清**整个机组**现在什么情况 —— 它直接显示在侧栏这个机组那一行，替掉原来那条「最新消息」。\n**和消息同一次动作产生**，所以它永远不会比最新消息更旧；不填就沿用上一次填的（侧栏会把那句话的年龄一起摆出来）。\n侧栏那行大概露得出 40 字，超了照写、回执提醒一句；**超过 200 字拒收**（那不是一句状态，是一篇报告——报告发正文）。\n整批发多条时这是**一次一个**的顶层参数，不是每条一个。"],
                            "reply_to": [
                                "type": "string",
                                "description": "可选：你在回复哪条群聊消息的 id —— 给了会自动 @ 那条的原发送者。**这个自动 @ 不收窄可见范围**：落盘的形状是 `[{kind:\"broadcast\"},{被回复者}]` —— 全组照样看得见全文，只是把被回复的那个现在叫醒。你自己在 mentions 里手打了定向 @（session/captain）时按你选的排他来，不替你放宽。",
                            ],
                        ],
                        "required": ["message"],
                    ],
                ],
                [
                    "name": "directory",
                    "description": "查全机通讯录：本机每个 crew、每个 session 都有一个短号码 —— crew 是整数（`7`），成员是分机（`7-3`），其中 `-1` 恒定是那个 crew 的机长。返回号码 / 名字 / 挂在哪个部门下 / 在干什么 / 在不在线（含已退出）。查到号码后用 contact 联系。query 可选：按号码前缀、名字、关键词过滤（如 \"7\" 看 7 号 crew 整组，\"更新\" 按名字找）。人类不编号 —— 找人仍用 ask。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "query": ["type": "string", "description": "可选过滤：号码前缀 / 名字 / 关键词。不填=全表。"],
                        ],
                    ],
                ],
                [
                    "name": "contact",
                    "description": "按号码联系别的 crew 或别的 session（号码用 directory 查）。语义**等同于你到对方群里发一条消息**：to 只填 crew 号（如 \"7\"）= 在那个群里广播发言（对方机长会被叫醒）；\"7-1\" = 定向 @ 那个 crew 的机长；\"7-3\" = 定向 @ 那个 session。消息在对方群里会署上你的来源 crew 名和号码，你自己群里也会留一行「已联系 …」的回执 —— 跨线联系全部留痕。汇报线（机长的 report_to_parent / message_child_crew）仍是组织纪律的主干，这个是补充通道：找错人不如找对人，但别绕过自己机长去替他做决定。本群的事直接用 post_to_crew，别打给自己。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "to": ["type": "string", "description": "目标号码：\"7\"（整个 crew，广播）/ \"7-1\"（该 crew 机长）/ \"7-3\"（某个 session）。"],
                            "message": ["type": "string", "description": "要说的话。对方群里看到的就是这段，写清你是谁、要什么。"],
                        ],
                        "required": ["to", "message"],
                    ],
                ],
                [
                    "name": "read_whiteboard",
                    "description": "分页读取当前 crew 群聊白板（按时间正序）。默认只返回最近 50 条，不会把长白板全量塞进上下文；有更早内容时，回执会给出下一页 before 游标。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "limit": ["type": "integer", "description": "每页条数，1–200，默认 50。"],
                            "before": ["type": "string", "description": "可选消息 id 游标：返回该消息之前的一页，不重复游标消息。"],
                        ],
                    ],
                ],
                [
                    "name": "search_whiteboard",
                    "description": "搜索当前 crew 的完整群聊白板。与 app 当前群/跨群搜索共用同一匹配核心：空白分隔的词全部命中（可跨正文、发送者、附件元数据、时间字段），中文按 Unicode 归一化后的子串匹配；附件只搜 filename/MIME，不读文件内容；after/before 为含边界 ISO8601；结果默认最新优先、最多 200 条，并返回 crew_id/message_id 供定位。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "query": ["type": "string", "description": "必填；空白分词全部 AND。"],
                            "after": ["type": "string", "description": "可选 ISO8601 下界，包含该时刻。"],
                            "before": ["type": "string", "description": "可选 ISO8601 上界，包含该时刻。"],
                            "limit": ["type": "integer", "description": "结果上限，1–200，默认 50。"],
                        ],
                        "required": ["query"],
                    ],
                ],
                [
                    "name": "ask",
                    "description": "向人类提一个需要拍板的问题。任何需要人判断、决策、方向选择、澄清、授权的，都走这个 —— 不要在纯文本里问。\n\n**它不阻塞。** 问题会记进人类 Todo（「人类的」那本，人翻得到、不回应不会消失），群里同时 @ 到人。你**立刻拿到回执，然后去干别的或者收工** —— 人回应时系统会直接叫醒你（你已经退出的话转给机长转达）。\n\n**`resume_note` 很重要**：写下「答复回来后我要接着做什么」。不阻塞意味着你会去干别的，被叫醒时如果不知道从哪儿接，这件事就等于丢了 —— 那比停在这儿等还糟。半路上问的问题**一定要写**。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "question": ["type": "string", "description": "要人拍板的那件事。把选项和你的倾向写出来——「A / B，我倾向 A，因为 …」比「这个怎么办？」好拍十倍。"],
                            "resume_note": ["type": "string", "description": "答复回来后你要接着做什么（你正做到哪一步、接下来那一步是什么）。会在人回应时**原样**念回给你。"],
                            "fallback_after_minutes": ["type": "number", "description": "可选：等这么多分钟还没人答，就把你叫醒按 `fallback` 说的办。**有时限的决定才填** —— 不填就是一直等人。"],
                            "fallback": ["type": "string", "description": "到点没人答时你打算怎么办（「就按 A 做」）。跟 fallback_after_minutes 一起给才有意义。"],
                        ],
                        "required": ["question"],
                    ],
                ],
                [
                    "name": "get_quota",
                    "description": "查本机 coding-agent 订阅额度（claude / codex 的订阅档位、各限额窗口、重置时刻；Claude 另带 requests/sessions 画像）。开长活前查一下用于规划。数据约 10 分钟刷新；上游不给剩余 token/request 绝对量，工具不会猜。",
                    "inputSchema": ["type": "object", "properties": [String: Any]()],
                ],
                [
                    "name": "schedule_wakeup",
                    "description": "设一个定时唤醒：到点后系统会把一条带你备注的消息注入回你这个 session，把你叫醒继续干。典型用法：额度快耗尽时（get_quota 看重置时刻），把手头活收尾，然后约在额度重置后几分钟唤醒自己接着做。after_minutes 与 at 二选一；note 写清醒来该干什么（醒来时只有这条备注 + 白板可看）。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "after_minutes": ["type": "number", "description": "多少分钟后唤醒（1–1440）。"],
                            "at": ["type": "string", "description": "ISO8601 时刻（如 2026-07-05T04:45:00+08:00）。与 after_minutes 二选一。"],
                            "note": ["type": "string", "description": "唤醒时带回给你的备注：醒来该继续什么、上下文在哪。"],
                        ],
                        "required": ["note"],
                    ],
                ],
                [
                    "name": "continue_work",
                    "description": "为**当前这一轮**登记一次持久、一次性的续跑承诺。只在你准备结束本轮、但仍有明确且无需外部输入就能继续的安全工作时调用；note 写下一轮第一件事。必须作为本轮最后一个工具调用。已完成、明确阻塞、正在等人/外部系统，或只是历史 Todo/plan 仍是 in_progress 时都不要调用。turn 真正 completed 后才会起下一轮；同一承诺最多消费一次，app 重启也不丢。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "note": ["type": "string", "description": "下一轮第一件事与必要上下文，简短且可独立执行。"],
                        ],
                        "required": ["note"],
                    ],
                ],
                [
                    "name": "listen",
                    "description": "开启「群聊收听」：像人开着微信群一样，在一段时间内，群里**没有 @ 任何人**的新消息（以及 @ 你的消息）会像被 @ 一样注入唤醒你。人类未指定对象的消息本来就会默认唤醒机长，不需要 @机长或先开 listen；listen 用来等其它广播动静。典型用法：你刚 post_to_crew 问了个问题在等回复、把活交接出去想盯进展、或想留意一段时间内人类/机长的动向。minutes 是收听时长（1–480，默认 30），到期自动停、不另行通知；senders 可选——只听某些发送者（\"human\"=人类、\"captain\"=机长、或某 session id/其前 6 位）；off=true 立即停止收听。忙时不会打断当前回合，变空闲后会自动补投，不依赖第二条消息。等待期间正常结束你的回合即可——不要空转轮询 read_whiteboard。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "minutes": ["type": "number", "description": "收听时长（分钟，1–480）。默认 30。"],
                            "senders": ["type": "array", "items": ["type": "string"],
                                        "description": "只听这些发送者：\"human\"/\"captain\"/session id（或前 6 位）。不填=全部。"],
                            "off": ["type": "boolean", "description": "true=停止收听。"],
                        ],
                    ],
                ],
                [
                    "name": "respond_todo",
                    "description": "回应本 crew **Agent 那本** Todo 的某个条目（Todo 面板「Agent 的」药丸；就是人类派给你们的活）。⚠️ 两本账别搞混：要**提一件请人类拍板的事**用 add_human_todo，那是「人类的」那本，这个工具动不了它。**追加式**：每次调用追加一条回应，不覆盖旧回应；可同时用 status 推进条目状态（待办 pending → 进行中 in_progress → 完成 completed）。**推不动、卡在人类身上时翻 `blocked_on_human`**（人类 Todo #139）——它会**原地**出现在人类那本 Todo 的列表里、标成黄色，**不要再用 add_human_todo 另开一条**：两条会各自被回应、各自翻牌，从此对不上。人答复之后照常翻回 in_progress / completed。**人类喊停、决定不做的翻 `dropped`**（「已叫停」）——它**不是** completed 的近义词：completed 说「做完了，凭据在这儿」，dropped 说「不做了，是谁决定的、为什么」。翻 dropped **不要凭据**（叫停没有产出），但那句回应必须写清是谁叫停的、理由是什么。把叫停记成完成，会让「完成 N 条」这个数当场变假，而且没有人看得出来。人类加条目时群里会出现「To do +1: #N …」——看到后用这个工具认领/回应，number 填那个 N。每个条目都该尽快有机器人回应；status 只在真有进展时才给（开始做→in_progress，做完验证过→completed）。**翻成 completed 必须带凭据**：`evidence_commit`（会当场解析，解不出来拒绝销号）或 `evidence`（产出不是 commit 时，一句话写清是什么），两个都不给不能销号 —— 这道闸是让「宣布完成」贵一点点，因为一条记成「已完成」而其实没做的账，没有任何人会回来看。领了 Todo 对应的活，落 main 时顺手翻牌——人类 Todo 面板和 task 账是两本账，别只更 task 漏翻 Todo。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "number": ["type": "integer", "description": "条目编号（群消息「To do +1: #N」里的 N）。"],
                            "response": ["type": "string", "description": "回应内容（认领/进展/结果，一两句说清）。"],
                            "status": ["type": "string", "enum": LocalTodoStore.statusOrder,
                                       "description": "可选：把条目状态推进到这个值。不填=只回应不动状态。**翻成 completed 时必须带凭据**（evidence_commit 或 evidence，见下）。"],
                            "evidence_commit": ["type": "string", "description": "销号凭据之一：产出所在的 commit（7–40 位十六进制）。**会当场在本 crew 登记的工作目录里解析**，解析不出来就拒绝销号并告诉你原因，不会「先记下来以后再核」。注意它只证明这个对象存在，不证明它做了这件事。"],
                            "evidence": ["type": "string", "description": "销号凭据之一：产出不是 commit 时用它（一次核对 / 一个结论 / 在哪台真机上验的 / 哪份归档日志）。一句话写清**是什么**，别写「已处理」。"],
                        ],
                        "required": ["number", "response"],
                    ],
                ],
                [
                    "name": "add_human_todo",
                    "description": "往**人类 Todo**（Todo 面板「人类的」那本）加一条。这本账装的是**要人办的事**：需要人拍板、做方向选择、给权限/密码/账号，或只有人才能做的（去某个后台点一下、真机上试一下、看一眼界面对不对）。**别一股脑喊进群聊**——群消息刷过去就漏了；这本账翻得到，没回应不会消失。\n\n**和 ask 的唯一分界是「你等不等」，别用混**：ask 是**阻塞**的，你当场停下来等答复，只用在「不知道答案就没法往下干」的事上；这个是**非阻塞**的，你写完接着干别的活，人类有空再拍。**默认走这条**，只有真干不下去了才 ask。\n\n写法：一条一件事，把**选项和你的建议**写出来——「A / B，我倾向 A，因为 …」比「这个怎么办？」好拍十倍。加完群里会出「人类 To do +1: #N …」。人类回应时群里出「回应 人类 To Do #N：…」并**直接叫醒你**（那时你已经退出了就转给机长转达），所以不用轮询、也不用守着等。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "text": ["type": "string", "description": "要人拍板/要人做的那件事。一条一件，带上选项和你的建议。"],
                            "supersedes": ["type": "integer", "description": "可选：这条**取代**之前的哪一条（填那条的 #N）。填了就等于同时撤回旧的那条，人只会看到新的这条在等他。**范围与 withdraw_human_todo 同一把尺子**：自己提的、还没被撤回的那条；本 crew 机长则是本 crew 的任何一条。N 验不过就整件事都不做（新条目也不会加），你改对了再来。别在正文里写「本条取代 #N」——写在正文里没有任何东西会去执行它。"],
                        ],
                        "required": ["text"],
                    ],
                ],
                [
                    "name": "withdraw_human_todo",
                    "description": "撤回一条人类 Todo（Todo 面板「人类的」那本）。用在**那件事已经不成立了**：版本发出去了、站上线了、人当面答过了、那条线被别的决定取代了。\n\n**这是这本账里最该常用的一个动作，因为只有你判断得了。**一条人类 Todo 最常见的死法不是人不想答，是**世界变了**——而人类判断不了世界变没变（他不知道下游走到哪一版了），能判断的只有当初提的那一方。你不撤，它就一直挂在他账上亮着灯催他，而那件事其实早就没了。\n\n**谁撤得动**：你自己提的那条（按落账时记下的 session 判定，不看显示名）；**你要是本 crew 的机长，本 crew 的任何一条你都撤得动**——包括提出者 session 已经不在了的、和老得根本没记提出者的。机长是常驻角色，session 会消失，所以这本账的清理责任在机长身上：看到一条已经作废却没人撤得掉的，那就是你的活。**只能撤本 crew 的**（父 crew 机长也伸不进子 crew，那会绕过人家自己的机长），**reason 必填**。\n\n撤回**不是删除**：条目留在列表里、原因写在它的时间线上、群里也会出一行「撤回 人类 To Do #N：…」——人有权知道你撤了什么、为什么，也有权追问把它问回来。别拿它清理你不想答的事，那是人的账不是你的；替别人撤之前先确认那件事**真的**不成立了。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "number": ["type": "integer", "description": "要撤回的那条的 #N（人类那本的编号，就是当初回执里给你的那个）。"],
                            "reason": ["type": "string", "description": "为什么它不成立了。一句话说清**变了什么**（「0.1.25 已发，这条问的是要不要发」），别写「已处理」。"],
                        ],
                        "required": ["number", "reason"],
                    ],
                ],
                [
                    "name": "crew_ordering_signals",
                    "description": "（机长专用）取**排序原料**：每个 crew 的三列时间 —— ① 最近有动静（任何人）/ ② 人类自己最后发言 / ③ 人类最后打开。\n\n**它不排序、不打分、不加权**，就是把三列原样给你。人类点名要的是「把各个指标拿出来 还有总机长的群聊信息 作为上下文 让总机长自行判断顺序」—— 判断是你的活。\n\n三列各自的毛病会跟数一起给你（① 量的是 agent 在哪儿忙、② 分辨率很低、③ 刚开始埋点很稀疏），**别单看数**。你自己那个群聊里的话（他说过「这周先搞 XX」之类）是任何指标都算不出来的，那部分本来就在你上下文里，记得一起用。\n\n看完用 arrange_crews 把顺序排下去，并写清理由。",
                    "inputSchema": ["type": "object", "properties": [:]],
                ],
                [
                    "name": "arrange_crews",
                    "description": "（机长专用）把几个 crew **顶到侧栏「总机长」视图的最前面**，并说清为什么。\n\n这个视图的基础序是「最近有动静的在上」，永远算得出来；你排的这份只是**叠在上面的覆盖层**：没排过、排布读不出来、里面的 crew 已经没了 —— 一律退回基础序，界面照常能用。**你的判断可以决定「推荐他先看什么」，但决定不了「这台机器上有什么」。**\n\n`reason` 必填，而且**会显示给人看**（不是日志）：他看到一个不合意的顺序时，得分得清是规则算的还是你排的、为什么。做成黑箱，它第一次排错就会被永久关掉。\n\n`crew_ids` 传空数组 = 撤掉排布，退回纯基础序。crew id 从 directory 或组织树里取。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "crew_ids": ["type": "array", "items": ["type": "string"],
                                         "description": "要顶到最前的 crew id，按你想要的顺序。空数组 = 撤掉排布。没提到的 crew 跟在后面、保持基础序。"],
                            "reason": ["type": "string", "description": "为什么这么排。一句话，写给人看的（「这三个他今天在改，其余按动静排」比「已优化排序」有用一百倍）。"],
                        ],
                        "required": ["crew_ids", "reason"],
                    ],
                ],
                [
                    "name": "set_session_profile",
                    "description": "切换你自己这个 session 的模型/thinking effort（至少给一个）。用于按任务阶段调配：机械收尾活降到轻模型/低 effort 省额度，难题升 effort。claude session 在**你本回合结束后**生效（等价终端里打 /model、/effort —— 斜杠命令只能在终端空闲时执行，所以不是当场切换；生效/失败都会回执到白板，成功还会在终端通知你）。撞额度上限时用它正合适：回合被打断后切换落地，你会被叫醒在新模型上接着跑，不用等重置。codex 没有中途切换通道——会在白板收到说明，新任务请让机长用 start_session 带 model/effort 另起。\n"
                        + catalogHint(agents: agentKey.map { [$0] } ?? ["claude", "codex"]),
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "model": ["type": "string", "description": "模型别名/slug，如 opus/sonnet/haiku。"],
                            "effort": ["type": "string", "description": "thinking effort 档位。"],
                        ],
                    ],
                ],
            ]
            if isCaptain {
                // 机长作战板（人类 Todo #66）—— 与两本 Todo 的关系：Todo 是**别人给的**
                // （`.agent` 人类派活 / `.human` 请人拍板），这一本是**机长自己排的**。
                // 派活 / 收活 / 翻牌这三个动作发生时顺手更一条，是这块板唯一的活法。
                // 空闲核账（驾驶舱计划 #71）。它不是「又一个督办」——
                // `supervise_after_minutes` 那套盯的是**单条计划、按超时**；
                // 这一条盯的是**整本 Todo 账、按空闲**，两者共享纪律不共享代码
                // （理由写在 `CaptainTodoSweep` 的注释里）。
                tools.append([
                    "name": "confirm_todo_sweep",
                    "description": Self.confirmSweepDescription,
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "running": ["type": "array", "items": ["type": "integer"],
                                        "description": "正在跑 / 有人在做的 #N。"],
                            "blocked_on_human": ["type": "array", "items": ["type": "integer"],
                                        "description": "卡在人类那边、你推不动的 #N。"],
                            "queued": ["type": "array", "items": ["type": "integer"],
                                        "description": "还没开始、排着的 #N。"],
                            "note": ["type": "string", "description": "可选：一句话补充（不影响判定）。"],
                        ],
                    ],
                ])
                tools.append([
                    "name": "plan_add",
                    "description": "（机长专用）往**你自己的任务列表**上排一条活。这本账只有你写得动，人类只读——它是你整理出来的作战板，不是 Todo（Todo 是别人给你的）。新条目从「没做」起。派活给 worker、接下一件事、拆出一个阶段时顺手排一条；一条一句话说清做什么，别把整段 brief 塞进来。\n**把活交出去时顺手挂上 supervise_after_minutes**（督办）：到点这条还没有结果，系统会把**你**叫醒（只叫你一个，不进群、不打扰别人）。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "title": ["type": "string", "description": "一句话说清这条活是什么。"],
                            "supervise_after_minutes": ["type": "number", "description": Self.superviseParamDescription],
                        ],
                        "required": ["title"],
                    ],
                ])
                tools.append([
                    "name": "plan_update",
                    "description": "（机长专用）推进任务列表上的一条：追加进度描述 / 翻进度档 / 改标题 / 撤下，一次可以做完几样。\n**四档**：not_started（没做）· in_progress（进行中）· blocked（卡住）· done（完成）——注意跟 Todo 的三档不是一回事。\n**翻成 blocked 必须指明卡在哪条人类 Todo**（blocked_by_number，默认指 human 那本，也就是你请人类拍板的那本）：「卡住」的意思就是**卡在人身上**，不指出是哪一条，人看到板也不知道该推什么。翻成 blocked 时（且仅此一档）会往群里发一条——其余的进度更新**不进群**，这块板存在的意义就是让进度不必靠刷屏传达。\n**把活交出去时顺手挂 supervise_after_minutes**（督办）：到点这条还没有结果，系统只叫醒你一个，不进群。解除督办**只有**把这条翻到 done 或 blocked 一条路——没有「我知道了」这种动作，所以想让它停就得把板更到有结果。\n**什么时候更**：派活、收活、给 Todo 翻牌，这三个动作发生时顺手更一条。板上每条都记着最后更新时间并显示在界面上（「进行中 · 最后更新 3 天前」），久没碰的条目人一眼就看得见——这是你自己装的照妖镜，别让它照出一板子 6 天前。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "number": ["type": "integer", "description": "这条计划的 #N（plan_list 里看得到）。"],
                            "progress": ["type": "string", "description": "进度描述，**追加式**（不覆盖旧的）。"],
                            "status": ["type": "string", "description": "not_started / in_progress / blocked / done。不填=不动状态。"],
                            "blocked_by_number": ["type": "integer", "description": "卡在哪条人类 Todo 的 #N。status=blocked 时必给（除非这条已经卡着且卡点没变）。"],
                            "blocked_by_ledger": ["type": "string", "description": "哪一本 Todo 账：human（你请人类拍板那本，默认）/ agent（人类派给你那本）。两本各自从 #1 起，裸 #N 有歧义，所以要说清是哪本。"],
                            "title": ["type": "string", "description": "改标题（排错了、说法不准时）。"],
                            "drop": ["type": "boolean", "description": "撤下这条（软删，号码保留不复用）。整理板面用。"],
                            "supervise_after_minutes": ["type": "number", "description": Self.superviseParamDescription],
                        ],
                        "required": ["number"],
                    ],
                ])
                tools.append([
                    "name": "plan_list",
                    "description": "（机长专用）读你自己的任务列表：每条的 #N、进度档、卡在哪、以及**多久没更新过**。开工前先看一眼，别把已经排过的活再排一遍。",
                    "inputSchema": ["type": "object", "properties": [:]],
                ])
                tools.append([
                    "name": "rename_crew",
                    "description": "（机长专用）给这个 crew 起名字。用**标签的思想**：一个短概括，不是一句描述。等你搞清楚这个 crew 在做什么再叫，例：「鉴权重构」「深色模式」「语音重连」。别写成长句、别带标点、别加前后缀。搞清楚后改一次就够，别反复改；人类要是已经起了个有意义的名字，就别动。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "name": ["type": "string", "description": "短标签（几个字的概括）。"],
                        ],
                        "required": ["name"],
                    ],
                ])
                tools.append([
                    "name": "raise_attention",
                    "description": "（机长专用，旧会话兼容）记录一条 attention 文案，但 Todo #71 起不再控制侧栏状态指示。需要人类处理、并点亮黄色呼吸指示时，请改用 add_human_todo；那本账可追踪、可回应。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "reason": ["type": "string", "description": "一句话：为什么需要人类注意（待决策的事 / 解决不了的问题）。"],
                        ],
                        "required": ["reason"],
                    ],
                ])
                tools.append([
                    "name": "clear_attention",
                    "description": "（机长专用，旧会话兼容）清除旧 attention 文案；不影响由人类 Todo 控制的黄色呼吸指示。",
                    "inputSchema": ["type": "object", "properties": [String: Any]()],
                ])
                tools.append([
                    "name": "change_workdir",
                    "description": "（机长专用）改这个 crew 的工作目录，并把 agent 侧按路径分家的东西一起带过去（claude 的项目记忆与工具权限）。**目录信任位不搬** —— 那是人对这个目录的授权，不是我们的技术步骤；新目录没被信任时，回执里会带上要人去终端跑的那条命令。仓库搬家、目录改名时用它，别去手改文件。\n\n**不带 confirm 就是预览**：返回会做什么、有什么拦路的，什么都不动。看过没问题再带 confirm:true 调一次才真执行。\n\n几件必须知道的事：\n· **一次做完，没有第二趟** —— 不存在「等谁停了再补一趟」这回事。\n· **不用管 session 的会话记录**：`claude --resume <会话号>` 扫整个 ~/.claude/projects 树、只认会话号，**不按目录找**（`--continue` 才是按当前目录）。所以日志留在旧 slug 下照样续得上，我们不搬它。\n· 新目录必须**已经存在**，不会替你创建。\n· 有**别的成员正在干活**会拒绝执行并点名；你自己（发起的机长）和空闲的成员都不拦路。这条现在只是常识（别把目录从干活的人脚下抽走），不再保护任何文件。\n· 新目录只对**之后新起/重启**的 session 生效；此刻在跑的（包括你自己）还在旧目录里，直到重启。\n· 动手前会把 ~/.claude.json、crew 账本各备份一份；~/.codex/config.toml 一个字节都不碰。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "new_path": ["type": "string", "description": "新工作目录的绝对路径（必须已存在）。"],
                            "crew": ["type": "string", "description": "改哪一个：本 crew 的标签名或 id，也可以是你名下任一子 crew。省略 = 本 crew。只能动自己这棵子树。"],
                            "include_children": ["type": "boolean", "description": "连同子 crew 一起迁。默认 true。"],
                            "confirm": ["type": "boolean", "description": "true = 真执行；省略/false = 只出预览。"],
                        ],
                        "required": ["new_path"],
                    ],
                ])
                tools.append([
                    "name": "start_session",
                    "description": "（机长专用）在当前 crew 里起一个 worker session 去干一件明确的编码任务。brief 写清要干什么。title 可选但强烈建议：一句 ≤18 字、不带项目名的任务概括——它是这个 session 在群聊气泡和成员列表里的显示名（不传就从 brief 兜底截断，可能不够精简）。runner 默认随本 crew（不填即可），可填 \"claude\"/\"codex\" 覆盖。isolation 必填：机长必须根据并行冲突、改动范围和任务关系明确决定；false 使用 crew 共享目录，true 新建独立 worktree，创建失败会直接报错而不会偷偷退回共享目录。model/effort 可选：按任务难度配置；不填沿用对应 runner 默认。派的活对应人类 Todo 条目（「To do +1: #N」）时，把 #N 显式写进 brief，并要求 worker 落 main 时顺手 respond_todo 翻牌——别只更 task 账漏翻 Todo。起完 worker 会自己报到，你在群聊看得到。\n"
                        + catalogHint(agents: ["claude", "codex"]),
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "brief": ["type": "string", "description": "要这个 session 干的明确任务。"],
                            "title": ["type": "string", "description": "可选：≤18 字、无项目名的任务概括，作 session 群聊/成员列表显示名。不填从 brief 兜底。"],
                            "runner": ["type": "string", "enum": ["claude", "codex"]],
                            "isolation": ["type": "boolean", "description": "必填。false=crew 共享目录；true=新建独立 worktree。必须由机长逐次判断。"],
                            "model": ["type": "string", "description": "可选：模型别名/slug（清单见工具描述里的可用模型表）。不填=对应 runner 的默认解析，那条腿指向哪也写在表里。"],
                            "effort": ["type": "string", "description": "可选：thinking effort（档位见工具描述里的可用模型表；codex 逐模型不同）。不填=对应 runner 默认。"],
                        ],
                        "required": ["brief", "isolation"],
                    ],
                ])
                tools.append([
                    "name": "handoff_captain_to_session",
                    "description": "（机长专用）把机长位置交给一个**现有 session 成员**。target_crew_id 省略时仍只操作本 crew；显式填写时只允许自己的直系子 crew，不允许上级、平级、孙 crew，且发起者在执行时仍须是父 crew 当前机长。session_id 必须属于目标 crew 且是稳定 id；app 会按成员表 + agent-sessions 账本核对真实 runner 和可续接会话号，绝不从显示名猜。停旧、续接新机长、持久化与失败回滚复用同一事务，最终成功/失败以群聊系统回执为准。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "target_crew_id": ["type": "string", "description": "可选：要救援的直系子 crew 精确 id；省略=本 crew。"],
                            "session_id": ["type": "string", "description": "目标 crew 现有 agent session 的稳定 id。"],
                        ],
                        "required": ["session_id"],
                    ],
                ])
                tools.append([
                    "name": "create_and_handoff_captain",
                    "description": "（机长专用）新建一个 agent session 并把机长位置交给它。target_crew_id 省略时仍只操作本 crew；显式填写时只允许自己的直系子 crew，不允许上级、平级、孙 crew，且必须明确 runner/model/effort/opening_brief。runner 只接受 claude/codex；本 crew 模式继续允许 model/effort/opening_brief 走原有默认。停旧、起新、持久化与失败回滚复用同一事务，最终成功/失败以群聊系统回执为准。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "target_crew_id": ["type": "string", "description": "可选：要救援的直系子 crew 精确 id；省略=本 crew。"],
                            "runner": ["type": "string", "enum": ["claude", "codex"], "description": "新机长 runner，必须显式选择。"],
                            "model": ["type": "string", "description": "可选模型别名/slug；不填=所选 runner 默认。"],
                            "effort": ["type": "string", "description": "可选 thinking effort；不填=所选 runner 默认。"],
                            "title": ["type": "string", "description": "可选 session 标题；不填=「机长」。"],
                            "opening_brief": ["type": "string", "description": "可选首轮交接任务；不填=仅确认接管并续接白板。"],
                        ],
                        "required": ["runner"],
                    ],
                ])
                tools.append([
                    "name": "inspect_session",
                    "description": "（机长专用）查看某个 session 的终端现场：返回其当前状态（干活中/空闲/异常/已退出）+ 终端最近若干行输出（codex 为 transcript 尾部）。用于 @ 不应时自己诊断：先 list_sessions 拿 session_id，再 inspect 看它卡在哪（模态菜单/等输入/报错）。看完通常接 nudge_session 解卡。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "session_id": ["type": "string", "description": "目标 session 的 id（list_sessions 里那个）。"],
                        ],
                        "required": ["session_id"],
                    ],
                ])
                tools.append([
                    "name": "nudge_session",
                    "description": "（机长专用）向某个 session 的终端发文本或按键，替卡住的它解围。input 填 \"Enter\"/\"Esc\" 发对应按键（选菜单项/退出模态框），填其它文本则作为一条输入发给它（自动回车提交）。先 inspect_session 看清现场再按，别盲按。codex session 无终端：Esc 会打断当前 turn，其余文本作为新 turn 输入。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "session_id": ["type": "string", "description": "目标 session 的 id。"],
                            "input": ["type": "string", "description": "\"Enter\" / \"Esc\" / 要发送的文本。"],
                        ],
                        "required": ["session_id", "input"],
                    ],
                ])
                tools.append([
                    "name": "stop_session",
                    "description": "（机长专用）终止本 crew 某个 session 的进程，操作不可撤销。必须给 reason；系统会先把机长、目标 session 和原因写进群聊白板，再复用人类红色停止按钮的 run.stop() 真正结束子进程。它和 nudge_session 发 Esc 不同：Esc 只打断当前一轮，session 进程仍然存活；stop_session 是把进程真正终止。先用 list_sessions 核对 session_id，不能停止别的 crew 的 session。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "session_id": ["type": "string", "description": "本 crew 目标 session 的 id。"],
                            "reason": ["type": "string", "description": "终止原因；会在停进程前公开写入本 crew 白板。"],
                        ],
                        "required": ["session_id", "reason"],
                    ],
                ])
                tools.append([
                    "name": "list_sessions",
                    "description": Self.listSessionsToolDescription,
                    "inputSchema": ["type": "object", "properties": [String: Any]()],
                ])
                tools.append([
                    "name": "report_to_parent",
                    "description": "（机长专用）向上级（父 crew）汇报：消息会送达所有直系父 crew 的群聊并唤醒父机长。用于:阶段性成果、需要上级拍板/协调资源、本部门被阻塞。汇报要短、带结论——上级不看过程日志。本 crew 没有父（根 crew）时会收到提示。",
                    "inputSchema": [
                        "type": "object",
                        "properties": ["message": ["type": "string", "description": "汇报内容（结论先行）。"]],
                        "required": ["message"],
                    ],
                ])
                tools.append([
                    "name": "message_child_crew",
                    "description": "（机长专用）给某个直系子 crew（下属部门）下达消息：送达其群聊并唤醒子机长。用于:派新任务、调整方向、催进度、要汇报。crew 参数填子 crew 的标签名或 id（不确定有哪些子 crew 时,消息发错会收到现有子 crew 清单）。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "crew": ["type": "string", "description": "目标子 crew 的标签名或 id。"],
                            "message": ["type": "string", "description": "要传达的内容。"],
                        ],
                        "required": ["crew", "message"],
                    ],
                ])
                tools.append([
                    "name": "adopt_crew",
                    "description": "（机长专用）收编：把一个**顶层/与本 crew 无上下级关系**的 crew 挂到本 crew 名下，成为其上级。这是向下建立上下级关系的唯一「抓取」动作——想归拢平行 crew 时，由要当上级的那个 crew 的机长来调它。crew 填目标的标签名或 id（解析不了会回执本机 crew 清单）。有环检测（不能挂进自己的子树），结果回执见两边群聊。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "crew": ["type": "string", "description": "要收编的 crew 的标签名或 id。"],
                        ],
                        "required": ["crew"],
                    ],
                ])
                tools.append([
                    "name": "release_crew",
                    "description": "（机长专用）调整**自己直系子 crew**的挂靠：to 省略 = 把它摘出到顶层（脱离本 crew）；to 填另一个直系子的标签名或 id = 把它转挂到那个子 crew 名下。操作对象只能是自己的直系子——上级控制下级，平级互不控制。结果回执见两边群聊。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "crew": ["type": "string", "description": "要摘出/转挂的直系子 crew 的标签名或 id。"],
                            "to": ["type": "string", "description": "可选：转挂目的地（自己的另一个直系子）。省略 = 摘出到顶层。"],
                        ],
                        "required": ["crew"],
                    ],
                ])
                tools.append([
                    "name": "create_parent_crew",
                    "description": "（机长专用）在本 crew 头上新建一个父 crew：本 crew 自动成为它的子部门，父 crew 继承本 crew 的工作目录/机长类型并自动起父机长。典型用法：想把几个平行 crew 归拢到一个总组织时，先建父，再 report_to_parent 请父机长把其余平级 crew adopt_crew 收编进去——你不能直接动平级 crew。title 可不填（自动取地名，父机长自己改名）。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "title": ["type": "string", "description": "可选：父 crew 的短标签名。不填自动取地名。"],
                        ],
                    ],
                ])
                tools.append([
                    "name": "adopt_parent",
                    "description": "（机长专用）认父：把某个**现有** crew 认作本 crew 的父（自愿挂靠，向上建立汇报线）。crew 填目标的标签名或 id。有环检测（不能挂进自己的子树），结果回执见两边群聊。认完可用 report_to_parent 向它汇报。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "crew": ["type": "string", "description": "要认作父亲的 crew 的标签名或 id。"],
                        ],
                        "required": ["crew"],
                    ],
                ])
                tools.append([
                    "name": "create_child_crew",
                    "description": "（机长专用）以当前 crew 为父，建一个子 crew。两个维度判断该不该拆，满足其一即可：**规模**——一块事大到该独立成组、要有自己的机长和群聊；**噪音**——要和某个对象高频往来大量消息时，哪怕子 crew 只有两个 session，也把高量私聊挪出去，别在主群刷屏。两头都不沾就别滥拆。子 crew 继承本 crew 的工作目录与机长类型，会自动起自己的机长，brief 作为它的开场任务。title 可不填（自动取个地名，子机长之后自己改名）。组织树层数不限。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "brief": ["type": "string", "description": "这个子 crew 要干的事（子机长的开场任务）。"],
                            "title": ["type": "string", "description": "可选：短标签名。不填自动取地名。"],
                        ],
                        "required": ["brief"],
                    ],
                ])
            }
            return result(id: id, ["tools": tools])
        case "tools/call":
            let params = obj["params"] as? [String: Any]
            return handleToolCall(id: id,
                                  name: params?["name"] as? String,
                                  args: params?["arguments"] as? [String: Any] ?? [:])
        default:
            guard id != nil else { return nil }
            return error(id: id, code: -32601, message: "method not found: \(method ?? "nil")")
        }
    }

    private func handleToolCall(id: Any?, name: String?, args: [String: Any]) -> String? {
        switch name {
        case "continue_work":
            let note = ((args["note"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !note.isEmpty else {
                return toolResult(id: id, text: "ERROR: note 不能为空；写清下一轮第一件事。")
            }
            // 「已经有一条」和「没写进去」都会让 arm 返回 false —— 两句话完全不同，
            // 所以把后者单独接出来（`onWriteFailure`）。
            var armFailure: Error?
            guard continuations.arm(crewId: crewId, sessionId: sessionId, note: note,
                                    onWriteFailure: { armFailure = $0 }) else {
                if let armFailure {
                    return toolResult(id: id, text: WriteReceipt.notWritten(
                        what: "续跑承诺", error: armFailure, consequence:
                            "**本轮结束后不会有下一轮** —— 别指望它，要么现在把活做完，"
                            + "要么在群里说清你停在哪。"))
                }
                return toolResult(id: id, text: "本 session 已有一条未消费的续跑承诺；没有重复登记。")
            }
            return toolResult(id: id, text: "已登记本轮一次性续跑；当前 turn 真正结束后执行，最多一次。")
        case "post_to_crew":
            switch CrewMessageBatch.parse(args: args) {
            case let .refuse(why):
                return toolResult(id: id, text: "ERROR: " + why)
            case let .batch(entries):
                // **先全部校验，再逐条执行**：分条之后「一半成功」是新的失败形态，
                // 而它最容易被读成「全成了」。任何一条不合法就整批拒、一条不发。
                for e in entries {
                    if case let .refuse(why) = CrewCategoryRouting.decide(
                        category: e.args["category"] as? String, args: e.args,
                        isCaptain: isCaptain) {
                        return toolResult(id: id, text: "ERROR: 第 \(e.index + 1) 条：" + why)
                    }
                    if let why = cockpitPermissionRefusal(args: e.args) {
                        return toolResult(id: id, text: "ERROR: 第 \(e.index + 1) 条：" + why)
                    }
                    if case let .refuse(why) = CrewMessageTodoLink.decide(args: e.args) {
                        return toolResult(id: id, text: "ERROR: 第 \(e.index + 1) 条：" + why)
                    }
                }
                var sent: [Int] = []
                var failed: [(index: Int, why: String)] = []
                for e in entries {
                    var one = e.args
                    one["message"] = e.text
                    one.removeValue(forKey: "messages")
                    let r = postToCrewOnce(args: one)
                    if r.ok { sent.append(e.index) } else { failed.append((e.index, r.text)) }
                }
                return toolResult(
                    id: id, text: CrewMessageBatch.batchReceipt(sent: sent, failed: failed))
            case .single:
                break
            }
            let once = postToCrewOnce(args: args)
            return toolResult(id: id, text: once.text)
        case "directory":
            // 通讯录（2026-08-11）：纯文件层汇总 —— local-crews.json（号码 + 组织边 +
            // 持久成员）× crew-sessions.json（实时状态）。helper 碰不到 app 内存态，
            // 这两份共享文件就是全部数据源。
            let directory: CrewDirectory
            do {
                directory = try CrewDirectory.load(whiteboardDirectory: sharedDirectory)
            } catch let failure as CrewDirectory.Unavailable {
                // 读不出来时**不许**渲染成空表：「本机还没有登记在案的 crew」
                // 比「查无此号」更像真话，也更危险。
                return toolResult(id: id, text: "ERROR: " + failure.message)
            } catch {
                return toolResult(
                    id: id, text: "ERROR: 通讯录读不出来：\(error.localizedDescription)")
            }
            var text = directory.render(query: args["query"] as? String)
            if let mine = directory.phoneNumber(
                crewId: crewId, sessionId: sessionId, isCaptain: isCaptain) {
                text = "你的号码：\(mine.text)\n" + text
            }
            return toolResult(id: id, text: text)
        case "contact":
            return handleContact(id: id, args: args)
        case "read_whiteboard":
            let all = store.list(crewId: crewId)
            guard !all.isEmpty else { return toolResult(id: id, text: "（白板为空）") }
            let requested = integerArgument(args["limit"]) ?? CrewMessageSearch.defaultLimit
            let limit = min(CrewMessageSearch.maximumLimit, max(1, requested))
            let end: Int
            if let before = (args["before"] as? String)?.trimmingCharacters(
                in: .whitespacesAndNewlines), !before.isEmpty {
                guard let cursorIndex = all.firstIndex(where: { $0.id == before }) else {
                    return toolResult(id: id, text: "ERROR: before 消息游标不存在或已失效")
                }
                end = cursorIndex
            } else {
                end = all.count
            }
            let start = max(0, end - limit)
            let page = Array(all[start..<end])
            guard !page.isEmpty else { return toolResult(id: id, text: "（没有更早消息）") }
            var text = page.map(renderRow).joined(separator: "\n")
            if start > 0, let first = page.first {
                text += "\n\n（显示 \(page.count) 条；还有 \(start) 条更早消息，下一页：read_whiteboard(before=\"\(first.id)\", limit=\(limit))）"
            } else {
                text += "\n\n（显示 \(page.count) 条；已到白板开头）"
            }
            return toolResult(id: id, text: text)
        case "search_whiteboard":
            let query = (args["query"] as? String) ?? ""
            guard !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: query 不能为空")
            }
            let afterValue = (args["after"] as? String)?.trimmingCharacters(
                in: .whitespacesAndNewlines)
            let beforeValue = (args["before"] as? String)?.trimmingCharacters(
                in: .whitespacesAndNewlines)
            let after = afterValue.flatMap(CrewMessageSearch.parseISO)
            let before = beforeValue.flatMap(CrewMessageSearch.parseISO)
            if let afterValue, !afterValue.isEmpty, after == nil {
                return toolResult(id: id, text: "ERROR: after 必须是 ISO8601 时刻")
            }
            if let beforeValue, !beforeValue.isEmpty, before == nil {
                return toolResult(id: id, text: "ERROR: before 必须是 ISO8601 时刻")
            }
            if let after, let before, after > before {
                return toolResult(id: id, text: "ERROR: after 不能晚于 before")
            }
            let requested = integerArgument(args["limit"]) ?? CrewMessageSearch.defaultLimit
            let rows = store.list(crewId: crewId)
            let crewTitle = LocalCrewStore.title(
                ofCrew: crewId, whiteboardDirectory: sharedDirectory) ?? ""
            let documents = rows.map {
                CrewMessageSearchAdapters.local(
                    $0, crewId: crewId, crewTitle: crewTitle)
            }
            let matches = CrewMessageSearch.search(
                documents, query: query, after: after, before: before,
                limit: requested, order: .newestFirst)
            guard !matches.isEmpty else {
                return toolResult(id: id, text: "（当前 crew 没有找到匹配消息）")
            }
            let byID = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })
            let rendered = matches.compactMap { match -> String? in
                guard let row = byID[match.document.messageId] else { return nil }
                let fields = match.matchedFields.map(\.rawValue).sorted().joined(separator: ",")
                let location = crewTitle.isEmpty
                    ? "crew_id=\(crewId)"
                    : "crew_id=\(crewId) crew_title=\(crewTitle)"
                return "\(location) message_id=\(row.id) matched_fields=\(fields)\n\(renderRow(row))"
            }
            return toolResult(
                id: id,
                text: "找到 \(rendered.count) 条（最新优先；时间边界包含；附件仅 filename/MIME）：\n\n"
                    + rendered.joined(separator: "\n\n"))
        case "ask":
            // 驾驶舱计划 #75 ①：**`ask` 不再阻塞。**
            //
            // 旧实现：raise 一条 `kind: "decision"` 待决策 → `awaitReply` 每 0.5s 轮询、
            // 最多 3600 次 = **正好 30 分钟**，人不在就真的停在那儿。那就是人类反复问的
            // 「怎么又停了」的一个主要来源。
            //
            // 新实现：问题**进人类 Todo 那本账**（2026-08-25 才有的账；待审批那套的 spec
            // 是 2026-06-08 —— 它诞生时「agent 请人类拍板」无处可去，所以自造了一套，
            // 账出来之后没人回头拆），立刻返回，agent 接着干别的。人回应时由
            // `HumanTodoWakePlan` 叫醒提问者（退出了回落机长转达），**不静默丢**。
            //
            // 承重点在 `resume_note`：不阻塞之后多了一个新风险，而且**比原来更糟** ——
            // 提完问题去干别的，就再也不回来做那件事了。所以提问时把「答复回来后接着
            // 做什么」一起记下，人回应时原样念回去（`TodoLandingFlow.wakeText`）。
            let question = (args["question"] as? String) ?? ""
            guard !question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: question 不能为空")
            }
            let askResume = (args["resume_note"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            // 旧实现里那条 30 分钟超时（`awaitReply` 到点写一句「自行判断后继续」）
            // 的**存在理由**是「把卡住的 agent 解开」—— 而现在它根本不阻塞，那个理由
            // 消失了。剩下的真需求只有一半：**有些决定有时限**。所以不重建一个全局
            // 定时器，改成让 agent 自己事先说好「等到 X 分钟就按 Y 办」，到点由现成的
            // `LocalWakeupStore` 叫醒它并把 Y 念回去。比原来强：原来是系统替它编一句
            // 「自行判断」，现在是它自己定的。
            let askFallback = (args["fallback"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let askFallbackMinutes: Double?
            switch SupervisionLease.parseMinutes(args["fallback_after_minutes"]) {
            case .none: askFallbackMinutes = nil
            case let .minutes(m): askFallbackMinutes = m
            case let .refused(why): return toolResult(id: id, text: "ERROR: " + why)
            }
            var askWriteFailure: Error?
            guard let askItem = humanTodos.add(
                crewId: crewId, text: question,
                bySessionId: sessionId, bySenderName: sessionLabel,
                resumeNote: (askResume?.isEmpty == false) ? askResume : nil,
                expectsResume: true,
                onWriteFailure: { askWriteFailure = $0 }) else {
                // 没落盘（读不出来 / 漏读 / 落盘失败，白板上有系统警示）。如实说没提上去
                // —— 绝不返回一个根本不存在的 #N 让调用方拿去对外宣布。
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "这个问题", error: askWriteFailure, consequence:
                        "（人类 Todo 这本账这次读不出来 / 漏读 / 落不了盘，原有内容没被动过，"
                        + "群聊白板上有系统警示）。**没有人会看到你在问什么** —— "
                        + "改用 post_to_crew 在群里直接问，或稍后重试。"))
            }
            // 通知半边：群里贴一条并 @ 到能处理的人。机长自己问时不 @ 自己。
            let askMentions: [LocalWhiteboardMention] = isCaptain
                ? [LocalWhiteboardMention(kind: "human", targetId: nil)]
                : [LocalWhiteboardMention(kind: "human", targetId: nil),
                   LocalWhiteboardMention(kind: "captain", targetId: nil)]
            var askReceipt = """
            已记进人类 Todo #\(askItem.number)（「人类的」那本），群里也 @ 了。
            **现在去干别的，别停在这儿等** —— 人回应时会直接叫醒你\
            （你已经退出的话转给机长转达），并把你写下的接续说明念回给你。
            """
            if let minutes = askFallbackMinutes {
                let fires = Date().addingTimeInterval(minutes * 60)
                let note = "人类 Todo #\(askItem.number) 到点仍未答复。你当时说过：\(askFallback?.isEmpty == false ? askFallback! : "（没写到点怎么办 —— 自己判断）")"
                if wakeups.register(LocalWakeupStore.PendingWakeup(
                    id: "ask-fallback:\(crewId):\(askItem.number)", crewId: crewId,
                    sessionId: sessionId, fireAt: ISO8601DateFormatter().string(from: fires),
                    note: note), onIncident: { _ in }) {
                    askReceipt += "\n到点（\(Int(minutes)) 分钟后）没人答的话会叫醒你，并把你说的办法念回来。"
                }
            }
            if askResume?.isEmpty != false {
                askReceipt += "\n⚠️ 你没写 resume_note。被叫醒时你可能不知道从哪儿接 —— "
                    + "下次问的时候把「答复回来后接着做什么」一起写上。"
            }
            do {
                let incident = try store.appendSessionMessageReportingFailure(
                    crewId: crewId, sessionId: sessionId,
                    text: "人类 To do +1: #\(askItem.number) \(question)",
                    category: "question", senderName: sessionLabel,
                    mentions: askMentions,
                    senderKind: isCaptain ? "captain" : "session")
                guard let incident else { return toolResult(id: id, text: askReceipt) }
                return toolResult(id: id, text: "⚠️ \(incident)\n\n\(askReceipt)")
            } catch {
                return toolResult(
                    id: id,
                    text: "⚠️ 群里那条" + WriteReceipt.notWrittenMarker
                        + "（\(error.localizedDescription)），"
                        + "群里没人会看到你在问什么 —— 账已经落上了，需要的话自己去群里补一句。"
                        + "\n\n\(askReceipt)")
            }
        // `answer_decision` 已随决策类一起拆掉（驾驶舱计划 #75 ①）。
        // 它的全部作用是让机长答一条 `kind: "decision"` 待决策、解开发起方的
        // long-poll —— 而 `ask` 不再产生待决策、也不再 long-poll，留着它就是一个
        // **永远找不到目标**的工具，还会在机长的世界观里继续教它去用。
        // 决策现在走人类 Todo：机长要拍板就直接 respond_todo 回那条。
        case "rename_crew":
            // 机长专用（crew-naming）：写一条待改名进控制通道；app 侧 CrewStore
            // 排空落地到 LocalCrewStore.setTitle 并刷新侧栏。不限长度 —— 标签思想
            // 靠描述 + prompt 引导；这里只把名字收成单行（空白/换行折成单空格）+ 拒空。
            guard isCaptain else {
                return toolResult(id: id, text: "ERROR: 仅机长可用")
            }
            let raw = (args["name"] as? String) ?? ""
            let name = raw.split(whereSeparator: \.isWhitespace).joined(separator: " ")
            guard !name.isEmpty else {
                return toolResult(id: id, text: "ERROR: name 不能为空")
            }
            if let failure = control.requestRename(crewId: crewId, name: name) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "改名请求", error: failure, consequence:
 "**侧栏上还是原来那个名字**，别当它已经改了。"))
            }
            return toolResult(id: id, text: "已把 crew 改名为「\(name)」。")
        case "raise_attention":
            // 旧会话兼容：attention 文案仍落盘，但 Todo #71 起不再控制状态点。
            guard isCaptain else {
                return toolResult(id: id, text: "ERROR: 仅机长可用")
            }
            let reason = ((args["reason"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !reason.isEmpty else {
                return toolResult(id: id, text: "ERROR: reason 不能为空 —— 一句话说明为什么需要人类注意。")
            }
            if let failure = control.requestAttention(crewId: crewId, reason: reason) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "attention 文案", error: failure, consequence:
 "什么都没记下。"))
            }
            return toolResult(id: id, text: "已记录兼容 attention 文案：\(reason)。它不点亮状态指示；需要黄色呼吸指示请用 add_human_todo。")
        case "clear_attention":
            guard isCaptain else {
                return toolResult(id: id, text: "ERROR: 仅机长可用")
            }
            if let failure = control.requestClearAttention(crewId: crewId) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "清除请求", error: failure,
                    consequence: "**原来那句 attention 文案还挂着**。"))
            }
            return toolResult(id: id, text: "已清除兼容 attention 文案；人类 Todo 的黄色呼吸指示不受影响。")
        case "start_session":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let brief = ((args["brief"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !brief.isEmpty else { return toolResult(id: id, text: "ERROR: brief 不能为空") }
            // 可选精简 title：单行折叠 + trim；空则 nil，app 侧从 brief 兜底 derive。clamp 归 app。
            let title = (args["title"] as? String)
                .map { $0.split(whereSeparator: \.isWhitespace).joined(separator: " ") }
                .flatMap { $0.isEmpty ? nil : $0 }
            let runner = (args["runner"] as? String).flatMap { ["claude", "codex"].contains($0) ? $0 : nil }
            guard let isolation = args["isolation"] as? Bool else {
                return toolResult(
                    id: id,
                    text: "ERROR: isolation 必填 —— 请明确选择 false（共享 crew 目录）或 true（新建独立 worktree）。")
            }
            // model/effort 透传字符串：**照旧透传，绝不拦**（表不是白名单，见
            // `AgentModelCheck`）。这里只做「单 token、非空」的卫生（防把整句话塞进
            // argv），外加对着模型表说一句提醒 —— 填了表里没有的值不再静默（Todo #36）。
            let model = (args["model"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .flatMap { $0.isEmpty || $0.contains(" ") ? nil : $0 }
            let effort = (args["effort"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
                .flatMap { $0.isEmpty || $0.contains(" ") ? nil : $0 }
            // runner 没填 = 随 crew 默认，这边判不出是哪家 → 两家表都对照，
            // 任一家认得就不吭声（宁可少说，也别对着错的表瞎报）。
            let targets = runner.map { [$0] } ?? ["claude", "codex"]
            // start_session 走**启动参数**那条腿 —— claude 的 `--effort` 与运行时
            // `/effort` 不是一套（传 `auto` 会被静默降级），必须按 .launch 对照。
            let notes = profileAdvisories(model: model, effort: effort, agents: targets,
                                          phase: .launch)
            if let failure = control.enqueueStartSession(
                crewId: crewId, brief: brief, runner: runner,
                isolation: isolation, model: model, effort: effort, title: title) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "起 session 的请求", error: failure, consequence:
                        "**没有人会起来**，这条活也没有排进任何队列。重试，或在群里请人手动开。"))
            }
            let announceIncident = announceProfileAdvisories(
                notes, headline: "start_session（\(title ?? brief)）的参数对不上模型表")
            var text = "已安排起 session：\(title ?? brief)。它起来后会在群聊报到。"
            if !notes.isEmpty {
                text += "\n⚠️ 参数提醒（已照常起，没拦你；同一份提醒已发白板）：\n"
                    + notes.map { "· \($0)" }.joined(separator: "\n")
            }
            if let announceIncident { text += "\n⚠️ \(announceIncident)" }
            return toolResult(id: id, text: text)
        case "handoff_captain_to_session":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let targetCrewId = (args["target_crew_id"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            if targetCrewId?.isEmpty == true {
                return toolResult(id: id, text: "ERROR: target_crew_id 不能为空；省略才表示本 crew")
            }
            let target = ((args["session_id"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty else {
                return toolResult(id: id, text: "ERROR: session_id 不能为空")
            }
            if let failure = control.enqueueCaptainHandoff(
                crewId: crewId, requesterSessionId: sessionId,
                targetCrewId: targetCrewId,
                targetSessionId: target, runner: nil, model: nil, effort: nil,
                title: nil, openingBrief: nil) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "机长交接请求", error: failure, consequence:
 "**机长没换**，一切照旧。"))
            }
            return toolResult(
                id: id,
                text: "机长交接请求已受理；app 会核对成员与真实会话账本并执行停旧/起新/回滚。请以群聊最终回执为准，这里不代表最终成功。")
        case "create_and_handoff_captain":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let targetCrewId = (args["target_crew_id"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            if targetCrewId?.isEmpty == true {
                return toolResult(id: id, text: "ERROR: target_crew_id 不能为空；省略才表示本 crew")
            }
            guard let runner = args["runner"] as? String,
                  runner == "claude" || runner == "codex" else {
                return toolResult(id: id, text: "ERROR: runner 必填，只接受 claude/codex")
            }
            let model = sanitizedProfileToken(args["model"] as? String, lowercased: false)
            let effort = sanitizedProfileToken(args["effort"] as? String, lowercased: true)
            let title = (args["title"] as? String)
                .map { $0.split(whereSeparator: \.isWhitespace).joined(separator: " ") }
                .flatMap { $0.isEmpty ? nil : $0 } ?? "机长"
            let openingBrief = (args["opening_brief"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .flatMap { $0.isEmpty ? nil : $0 }
            if targetCrewId != nil,
               (model == nil || effort == nil || openingBrief == nil) {
                return toolResult(
                    id: id,
                    text: "ERROR: 为直系子 crew 新建机长时，runner/model/effort/opening_brief 都必须明确填写")
            }
            if let failure = control.enqueueCaptainHandoff(
                crewId: crewId, requesterSessionId: sessionId,
                targetCrewId: targetCrewId,
                targetSessionId: nil, runner: runner, model: model, effort: effort,
                title: title, openingBrief: openingBrief) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "新建机长并交接的请求", error: failure,
                    consequence: "**新机长没建、旧机长没停**，一切照旧。"))
            }
            return toolResult(
                id: id,
                text: "新机长交接请求已受理（\(runner)，title=\(title)）；app 会执行真实停旧/起新/持久化与失败回滚。请以群聊最终回执为准，这里不代表最终成功。")
        case "inspect_session":
            // 机长自愈（wake-resilience 层4）：@ 不应时自己看终端现场。命令经
            // 控制通道给 app 执行（helper 碰不到 run/PTY），long-poll 应答文件。
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["session_id"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: session_id 不能为空") }
            let cmd = control.enqueueInspectSession(crewId: crewId, targetSessionId: target)
            if let failure = cmd.failure {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "查看现场的请求", error: failure, consequence:
                        "**app 侧根本收不到这条命令**，再等下去只会等到一句「超时无应答」"
                        + "并把病因指向「app 没在跑」。"))
            }
            return toolResult(id: id, text: awaitCommandResponse(commandId: cmd.id))
        case "nudge_session":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["session_id"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let input = (args["input"] as? String) ?? ""
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: session_id 不能为空") }
            guard !input.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: input 不能为空（\"Enter\"/\"Esc\"/文本）")
            }
            let cmd = control.enqueueNudgeSession(crewId: crewId, targetSessionId: target, input: input)
            if let failure = cmd.failure {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "这次 nudge", error: failure,
                    consequence: "**那个 session 的输入框里什么都没进去。**"))
            }
            return toolResult(id: id, text: awaitCommandResponse(commandId: cmd.id))
        case "stop_session":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["session_id"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let reason = ((args["reason"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: session_id 不能为空") }
            guard !reason.isEmpty else {
                return toolResult(id: id, text: "ERROR: reason 不能为空；终止前必须把原因写进白板")
            }
            let cmd = control.enqueueStopSession(
                crewId: crewId, requesterSessionId: sessionId,
                targetSessionId: target, reason: reason)
            if let failure = cmd.failure {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "终止请求", error: failure, consequence:
 "**那个 session 还在跑**，原因也没落进白板。"))
            }
            return toolResult(id: id, text: awaitCommandResponse(commandId: cmd.id))
        case "change_workdir":
            // 机长专用：改工作目录 + 迁 agent 上下文。规划/执行都在 app 侧（helper 是
            // 离线子进程，读不到 crew store，也看不到在跑的 run），这里只做参数卫生 +
            // long-poll 拿预览或回执。
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let newPath = ((args["new_path"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !newPath.isEmpty else {
                return toolResult(id: id, text: "ERROR: new_path 不能为空（要一个已经存在的目录的绝对路径）")
            }
            let targetHint = (args["crew"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let includeChildren = (args["include_children"] as? Bool) ?? true
            let confirm = (args["confirm"] as? Bool) ?? false
            let cmd = control.enqueueChangeWorkdir(
                crewId: crewId, sessionId: sessionId, targetHint: targetHint,
                path: newPath, includeChildren: includeChildren, confirm: confirm)
            if let failure = cmd.failure {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "改工作目录的请求", error: failure, consequence:
                        "**什么都没迁**，连预览都不会有。"))
            }
            // 迁移要复制整个项目记忆 + 重试写 ~/.claude.json（读—改—写，撞上别的 claude
            // 进程会重试几轮），默认 10 秒不够 —— 放宽 12 倍（按 `commandResponseMaxWaits`
            // 成比例，单测把基数调小后不会被这条拖慢）。
            return toolResult(id: id, text: awaitCommandResponse(
                commandId: cmd.id, maxWaits: commandResponseMaxWaits * 12,
                timeoutHint: "注意：它**可能仍在执行**——迁移回执会照常发进群聊，去群里看那条，别当成没跑过。"))
        case "get_quota":
            let url = quotaDirectory.appendingPathComponent("quota.json")
            guard let data = try? Data(contentsOf: url),
                  let file = try? JSONDecoder().decode(AgentQuotaFile.self, from: data) else {
                return toolResult(id: id, text: "暂无额度数据（PendingCrew 尚未完成首次刷新，稍后再查）。")
            }
            var lines: [String] = []
            for (snap, failure, name) in [(file.claude, file.claudeError, "Claude Code"),
                                          (file.codex, file.codexError, "Codex")] {
                guard let snap else {
                    // 一次都没取到过：说清读不到，别让这一家在输出里凭空消失。
                    if let failure { lines.append("\(name)：\(failure)") }
                    continue
                }
                let windows = snap.windows.map { w in
                    "\(w.label) 已用 \(w.usedPercent)%" + (w.resetsAt.map { "（\($0) 重置）" } ?? "")
                }.joined(separator: "；")
                lines.append("\(name)：订阅档位 \(snap.subscriptionPlanDescription)；\(windows)（数据时间 \(snap.fetchedAt)）")
                for activity in snap.activities ?? [] {
                    var counts: [String] = []
                    if let requests = activity.requests { counts.append("\(requests) requests") }
                    if let sessions = activity.sessions { counts.append("\(sessions) sessions") }
                    if !counts.isEmpty { lines.append(
                        "  \(name) \(activity.periodLabel)：\(counts.joined(separator: " · "))") }
                }
                // 失败/翻篇/陈旧都要明说 —— 否则「读到的时刻」看着永远是刚刚，人和
                // session 会把一个早就过期的百分比当现状拿去做规划（Todo #33）。
                if let failure {
                    lines.append("  \(name) 本轮\(failure)，上面是上一轮的旧值")
                }
                if snap.isPastReset() {
                    lines.append("  \(name) 这些窗口的重置时刻都已过去，百分比不是现状")
                }
                if let stale = snap.stalenessNote() {
                    lines.append("  \(name) \(stale)")
                }
            }
            if lines.isEmpty { lines = ["两家 agent 均未取到额度数据。"] }
            lines.append("注：档位提供量级背景，但上游不给剩余 token/request 绝对量，所以这里说不出绝对剩余。额度将尽时先收尾、再用 schedule_wakeup 约重置后继续。")
            return toolResult(id: id, text: lines.joined(separator: "\n"))
        case "schedule_wakeup":
            let note = ((args["note"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !note.isEmpty else { return toolResult(id: id, text: "ERROR: note 不能为空 —— 醒来的你只有这条备注可依靠。") }
            let fireAt: Date
            if let mins = args["after_minutes"] as? Double, mins >= 1, mins <= 1440 {
                fireAt = Date().addingTimeInterval(mins * 60)
            } else if let atRaw = args["at"] as? String,
                      let parsed = Self.parseISO(atRaw), parsed > Date() {
                fireAt = parsed
            } else {
                return toolResult(id: id, text: "ERROR: 需要 after_minutes（1–1440）或未来的 at（ISO8601）之一。")
            }
            let iso = ISO8601DateFormatter().string(from: fireAt)
            if let failure = control.enqueueScheduleWakeup(
                crewId: crewId, sessionId: sessionId, fireAt: iso, note: note) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "这次定时唤醒", error: failure, consequence:
                        "**到点不会有人叫你** —— 别按「醒来接着干」规划，把手上的活收尾到可交接状态。"))
            }
            return toolResult(id: id, text: "已设定时唤醒：\(iso)。到点会把你的备注注入回本 session；若届时你已退出，会落到群聊白板由机长接手。")
        case "listen":
            // 群聊收听（#465）：写一条 listen 命令进控制通道，app 侧 CrewSessionRunner
            // 登记后把收听期内的广播消息直投注入本 session。到期/off 都是 app 侧语义，
            // 这里只做参数卫生 + 计算截止时刻。
            if (args["off"] as? Bool) == true {
                if let failure = control.enqueueListen(crewId: crewId, sessionId: sessionId,
                                                       until: nil, senders: nil, off: true) {
                    return toolResult(id: id, text: WriteReceipt.notWritten(
                        what: "停止收听的请求", error: failure, consequence:
 "**收听还开着**，到期才会自己停。"))
                }
                return toolResult(id: id, text: "已停止收听群聊广播。普通 session 之后只有 @ 你的消息会唤醒你；人类未指定对象的消息仍会默认唤醒机长（白板每轮注入照旧）。")
            }
            let mins = (args["minutes"] as? Double) ?? 30
            guard mins >= 1, mins <= 480 else {
                return toolResult(id: id, text: "ERROR: minutes 需在 1–480 之间。")
            }
            let untilDate = Date().addingTimeInterval(mins * 60)
            let until = ISO8601DateFormatter().string(from: untilDate)
            let senders = (args["senders"] as? [Any])?
                .compactMap { ($0 as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
            if let failure = control.enqueueListen(
                crewId: crewId, sessionId: sessionId, until: until,
                senders: (senders?.isEmpty ?? true) ? nil : senders, off: false) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "开启收听的请求", error: failure, consequence:
                        "**没有在听** —— 就这么结束回合的话，群里的动静不会叫醒你。"))
            }
            let who = (senders?.isEmpty ?? true) ? "全部成员" : senders!.joined(separator: "、")
            return toolResult(id: id, text: "已开启群聊收听至 \(until)（听：\(who)）。期间无定向 @ 的新消息和 @ 你的消息会注入唤醒你；到期自动停。现在正常结束你的回合等消息即可，不要空转轮询。")
        case "confirm_todo_sweep":
            // 空闲核账的收尾（驾驶舱计划 #71）。**判定全在 `CaptainTodoSweep.validate`**，
            // 这里只负责取真账、把拒绝原样说清楚、以及落一条确认。
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let sweepOpen = Set(todos.list(crewId: crewId)
                .filter { !$0.isDeleted && !$0.isSettled }
                .map(\.number))
            let sweepResult = CaptainTodoSweep.validate(
                running: Self.intArray(args["running"]),
                blockedOnHuman: Self.intArray(args["blocked_on_human"]),
                queued: Self.intArray(args["queued"]),
                open: sweepOpen)
            if let refusal = sweepResult.refusal {
                return toolResult(id: id, text: "ERROR: 这份账跟真账本对不上，没有记下。\n" + refusal.summary)
            }
            guard let sweepConfirmation = sweepResult.confirmation else {
                return toolResult(id: id, text: "ERROR: 确认没能生成（内部状态异常），没有记下。")
            }
            if let failure = sweeps.recordConfirmation(crewId: crewId, sweepConfirmation) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "这次核账确认", error: failure, consequence:
                        "**提醒还会再来** —— 它读的是磁盘上那条确认，而那条没写上。重试一次。"))
            }
            return toolResult(id: id, text: sweepOpen.isEmpty
                ? "记下了：这本账一条未完成都没有。空闲时不会再提醒你，直到有新条目进来。"
                : "记下了：\(sweepOpen.count) 条未完成已逐条归桶。空闲时不再提醒你——**除非冒出没被这次覆盖过的新条目**（那时只会点名新的那几条）。")

        case "plan_add":
            // 机长作战板（人类 Todo #66）。门禁与其余机长工具同一道 `guard`。
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let planTitle = ((args["title"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !planTitle.isEmpty else {
                return toolResult(id: id, text: "ERROR: title 不能为空 —— 一句话说清这条活是什么。")
            }
            // 督办参数**先验后写**：不合法就在动账本之前拒掉，绝不留下一条
            // 「排上了但督办没挂上」的半截状态 —— 那正是机长会以为自己盯着、
            // 其实没人盯的形态。
            let addLease: Double?
            switch SupervisionLease.parseMinutes(args["supervise_after_minutes"]) {
            case .none: addLease = nil
            case let .minutes(m): addLease = m
            case let .refused(why): return toolResult(id: id, text: "ERROR: " + why)
            }
            guard let planned = plans.add(crewId: crewId, title: planTitle,
                                          bySessionId: sessionId, byName: sessionLabel) else {
                // nil ≠「没排」这么轻描淡写：读不出来（本次写已拒）或落盘失败，两种都在这儿。
                return toolResult(id: id, text: "ERROR: 这条计划" + WriteReceipt.notWrittenMarker
                    + " —— 任务列表这次读不出来或写不进去（群聊白板上有一条系统警示说明是哪种事故）。"
                    + "**板上没有这条**，别拿一个不存在的 #N 去对外宣布。")
            }
            var addLines = ["已排上 计划 #\(planned.number)：\(planned.title)（没做）。"]
            if let addLease {
                addLines.append(attachSupervisionLease(planNumber: planned.number, minutes: addLease))
            }
            return toolResult(id: id, text: addLines.joined(separator: "\n"))
        case "plan_update":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let planNumber = (args["number"] as? Int) ?? (args["number"] as? Double).map(Int.init)
            guard let planNumber, planNumber >= 1 else {
                return toolResult(id: id, text: "ERROR: number 需为正整数（plan_list 里的 #N）。")
            }
            // 同 plan_add：督办参数先验后写。而且**已经挂着督办的计划不许再挂**
            // —— 重挂就是「顺延」，那正是这套机制不给的动作（见 SupervisionLease）。
            let updateLease: Double?
            switch SupervisionLease.parseMinutes(args["supervise_after_minutes"]) {
            case .none: updateLease = nil
            case let .minutes(m): updateLease = m
            case let .refused(why): return toolResult(id: id, text: "ERROR: " + why)
            }
            if updateLease != nil,
               let existing = wakeups.list().first(where: {
                   $0.id == SupervisionLease.id(crewId: crewId, planNumber: planNumber)
               }) {
                return toolResult(id: id, text: """
                ERROR: 计划 #\(planNumber) 已经挂着督办（下次 \(existing.fireAt)）。督办不能顺延、也不能重挂 —— 这里没有「我知道了 / 已查看 / 顺延」这种动作，因为看一眼不算有结果。
                解除只有一条路：把 #\(planNumber) 翻到 done（完成）或 blocked（卡住）。
                """)
            }
            if (args["drop"] as? Bool) == true {
                guard plans.drop(crewId: crewId, number: planNumber) else {
                    return toolResult(id: id, text: "ERROR: 计划 #\(planNumber) 没撤下"
                        + WriteReceipt.notWrittenMarker
                        + "（找不到这条，或任务列表读不出来/写不进去 —— 后两者白板上有警示）。\n"
                        + planRows())
                }
                return toolResult(id: id, text: "已撤下 计划 #\(planNumber)（号码保留、不复用）。")
            }
            let planProgress = (args["progress"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .flatMap { $0.isEmpty ? nil : $0 }
            let planStatusRaw = (args["status"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
                .flatMap { $0.isEmpty ? nil : $0 }
            let planNewTitle = (args["title"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .flatMap { $0.isEmpty ? nil : $0 }
            // 引用**连账本一起收**：两本 Todo 各自从 #1 起，裸 #N 有歧义（群里那行
            // 都被迫加「人类」二字才分得清）。默认 human —— 机长的活卡住，绝大多数
            // 情况就是卡在「请人类拍板」那本上。
            let blockerLedger: String
            switch CrewCockpitLanding.blockerLedger(args["blocked_by_ledger"]) {
            case let .refused(why): return toolResult(id: id, text: "ERROR: " + why)
            case let .ok(value): blockerLedger = value
            }
            let blockerNumber = (args["blocked_by_number"] as? Int) ?? (args["blocked_by_number"] as? Double).map(Int.init)
            let blocker = blockerNumber.map { CockpitPlanBlocker(ledger: blockerLedger, number: $0) }
            let wasBlocked = plans.item(crewId: crewId, number: planNumber)
                .flatMap { CockpitPlan.status($0.status) } == .blocked
            switch plans.update(crewId: crewId, number: planNumber,
                                progress: planProgress, statusRaw: planStatusRaw,
                                blocker: blocker, title: planNewTitle,
                                bySessionId: sessionId, byName: sessionLabel) {
            case let .failure(failure):
                let tail = failure == .notFound ? "\n" + planRows() : ""
                // `.notWritten` 的 summary 自带那个记号（见 `UpdateFailure.summary`）；
                // 其余几种是「压根没动账」，不该冒充成写失败。
                return toolResult(id: id, text: "ERROR: " + failure.summary + tail)
            case let .success(item):
                let now = Date()
                var lines = ["计划 #\(item.number)：\(item.title) → "
                             + CockpitPlan.statusLine(statusRaw: item.status,
                                                      updated: Self.iso.date(from: item.updatedAt), now: now)]
                if let b = item.blockedBy {
                    lines.append(CockpitPlan.blockerLine(b, state: blockerState(b)))
                }
                // **只有翻成「卡住」才进群**（而且只在这一次翻的时候）：卡住 = 卡在人
                // 身上，那是群里唯一该出现的一档。其余进度更新一律不进群 —— 这块板
                // 存在的意义就是让进度不必靠刷屏传达，每推一步发一条等于原地退回去。
                if let updateLease {
                    lines.append(attachSupervisionLease(planNumber: item.number, minutes: updateLease))
                }
                if CockpitPlan.status(item.status) == .blocked, !wasBlocked {
                    let where_ = item.blockedBy.map { "，" + CockpitPlan.blockerLine($0, state: blockerState($0)) } ?? ""
                    store.appendSessionMessage(
                        crewId: crewId, sessionId: sessionId,
                        text: "计划 #\(item.number)「\(item.title)」卡住了\(where_)。",
                        senderName: sessionLabel,
                        mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)],
                        senderKind: isCaptain ? "captain" : "session")
                    lines.append("（已往群里发了一条 —— 卡住是唯一进群的那一档。）")
                }
                return toolResult(id: id, text: lines.joined(separator: "\n"))
            }
        case "plan_list":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            return toolResult(id: id, text: planRows())
        case "respond_todo":
            // 人类 Todo 的机器人回应（task #478）：追加式回应 + 可选状态推进。
            // number 收 Int/Double 两种形状（JSON 数字经 JSONSerialization 可能是
            // 任一种）。找不到 #N 时把当前列表带在错误里 —— agent 不用另一个工具
            // 就能自纠。
            let number = (args["number"] as? Int) ?? (args["number"] as? Double).map(Int.init)
            let response = ((args["response"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard let number, number >= 1 else {
                return toolResult(id: id, text: "ERROR: number 需为正整数（群消息「To do +1: #N」里的 N）。")
            }
            guard !response.isEmpty else {
                return toolResult(id: id, text: "ERROR: response 不能为空。")
            }
            let status = (args["status"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
                .flatMap { $0.isEmpty ? nil : $0 }
            if let status, !LocalTodoStore.validStatuses.contains(status) {
                return toolResult(id: id, text: "ERROR: status 只能是 \(LocalTodoStore.statusListText)。")
            }
            // 销号凭据（Todo #102 第四刀）：**只在翻成 completed 时要求，且先验后动账。**
            // 2026-09-07 挖出的那笔假账带着一个根本不存在的 hash 挂了 191 小时 ——
            // 病根不是「没带凭据」，是**那个凭据从来没有被解析过**。
            var evidenceLine: String?
            if status == "completed" {
                switch judgeCompletionEvidence(args: args) {
                case .commitResolved(let sha):
                    // 只说指针解析成功，**绝不说「已验证该修复」** —— 我们能验的只有
                    // 指针，不是内容。这道闸拦的是凭空捏造的 hash，不是指错的 hash。
                    evidenceLine = "凭据 `\(sha)` 解析成功（只证明这个对象存在，不证明它做了这件事）"
                case .prose(let text):
                    evidenceLine = "凭据：\(text)"
                case .malformedCommit(let raw):
                    return toolResult(id: id, text: "ERROR: `evidence_commit` 给的 \(raw) 不像一个 commit（要 7–40 位十六进制）。"
                                      + "**这条没有销号。**写错了就改对；产出本来不是 commit 的，用 evidence 写清是什么。")
                case .commitNotFound(let sha):
                    return toolResult(id: id, text: "ERROR: 凭据 `\(sha)` **在该 crew 的仓库里不存在** —— 问题在凭据本身。"
                                      + "**这条没有销号。**核对一下你要引的是哪个 commit；"
                                      + "如果这件事的产出本来就不是 commit（一次核对 / 一个结论 / 一次真机验收），用 evidence 写清是什么。")
                case .cannotVerify(let sha, let why):
                    return toolResult(id: id, text: "ERROR: 我**验不了**凭据 `\(sha)`：\(why) —— 问题在环境，不在你给的东西。"
                                      + "**这条没有销号，也没有把它记下来等以后再核**（「先记下来」正是那笔假账的形状）。"
                                      + "请改用 evidence 写清凭据是什么（可以把这个 sha 写在里面）。")
                case .missing:
                    return toolResult(id: id, text: "ERROR: 翻成「完成」要带凭据 —— `evidence_commit`（会当场解析）"
                                      + "或 `evidence`（一句话说清产出是什么：哪次核对、什么结论、在哪台真机上验的）。"
                                      + "**两个都不给不能销号。**这道闸是让「宣布完成」这个动作贵一点点："
                                      + "今晚有一条账记着「已修好」外加一个 hash，那个 hash 不存在，账挂了 191 小时。")
                }
            }
            // 凭据跟着回应落在条目时间线上 —— 不落进去，人翻这条 Todo 时仍然只看到
            // 一句「做完了」，那正是要治的东西。
            let responseText = evidenceLine.map { "\(response)\n\n\($0)" } ?? response
            // 「找不到 #N」和「写不进去」在这儿必须分得开：前者是号写错了（多半是
            // 指着**人类那本**的 #N 调了这个只写 agent 那本的工具 —— 两本账号码会撞），
            // 后者是账本这一刻落不了盘。两种情况 agent 该做的事完全不同。
            var respondWriteFailure: Error?
            guard let updated = todos.respond(
                crewId: crewId, number: number, sessionId: sessionId,
                senderName: sessionLabel, text: responseText,
                newStatus: status,
                onWriteFailure: { respondWriteFailure = $0 }) else {
                if let respondWriteFailure {
                    return toolResult(id: id, text: WriteReceipt.notWritten(
                        what: "这条回应", error: respondWriteFailure, consequence:
                            "**Todo #\(number) 上没有它**，状态也没动。重试一次。"))
                }
                let rows = todos.list(crewId: crewId).map {
                    "#\($0.number) [\(LocalTodoItem.statusLabel($0.status))] \($0.text)"
                }
                return toolResult(id: id, text: "ERROR: 没能回应 Todo #\(number)（找不到这条，"
                                  + "或 Todo 列表文件读不出来 —— 后者群聊白板上会有一条系统警示）。"
                                  + "**这是 agent 那本账**；你要回的如果是人类那本（群里那行「To do +1」），"
                                  + "两本号码会撞，核对一下。当前列表：\n"
                                  + (rows.isEmpty ? "（空）" : rows.joined(separator: "\n")))
            }
            return toolResult(id: id, text: "已回应 Todo #\(number)（状态：\(LocalTodoItem.statusLabel(updated.status))）。")
        case "add_human_todo":
            // 人类 Todo 的新增（Todo #62 ④）：**这本账只有 agent 能加**，方向与
            // respond_todo 那本正好相反。三步一条都不能少：
            //   1. 落账（记下 `createdBySessionId` —— 人类回应时靠它知道叫醒谁）
            //   2. 群里发一行「人类 To do +1: #N …」（人类原话「跟新建 todo 一样」）
            //   3. 回执如实
            // 没落盘就别去群里宣布（#577 的教训）—— 顺序是先落账再宣布。
            let todoText = ((args["text"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !todoText.isEmpty else {
                return toolResult(id: id, text: "ERROR: text 不能为空 —— 写清要人拍板/要人做的那件事。")
            }
            // supersede（Todo #102 第二刀）：**先验目标，再落任何账**。
            //
            // 做成显式参数而不是去正文里认「本条取代 #N」：那是一把措辞一变就静默
            // 失效的尺子，而且失效的样子跟正常一模一样。做成参数就有一个确定的 N，
            // **有了 N 就必须解引用** —— 验不过宁可整件事不做，也不留下「新的加了、
            // 旧的还挂着」这种半截状态（人会同时看到两条问同一件事）。
            let supersedes = (args["supersedes"] as? Int) ?? (args["supersedes"] as? NSNumber)?.intValue
            if let target = supersedes {
                let obstacle = LocalTodoStore.withdrawObstacle(
                    item: humanTodos.item(crewId: crewId, number: target), sessionId: sessionId,
                    isCaptain: isCaptain)
                switch obstacle {
                case .none:
                    break
                case .notFound:
                    return toolResult(id: id, text: "ERROR: supersedes 指的人类 Todo #\(target) 不在这本账上（号写错了、或人类已经删掉了）。"
                                      + "**新条目也没有加** —— 号改对了再来，或者去掉 supersedes 单纯新增。")
                case .notYours(let owner):
                    let who = owner.map { "「\($0)」" } ?? "（账上没记提出者，老条目）"
                    return toolResult(id: id, text: "ERROR: 人类 Todo #\(target) 不是你提的，提出者是 \(who) —— **你只能取代自己提的**。"
                                      + "**新条目也没有加。**要么去掉 supersedes 单纯新增；"
                                      + "要么让提出者自己撤，提出者已经不在了就找本 crew 的机长（机长撤得动本 crew 的任何一条）。")
                case .alreadyWithdrawn:
                    return toolResult(id: id, text: "ERROR: 人类 Todo #\(target) 已经撤回过了，不用再取代它。"
                                      + "**新条目也没有加** —— 去掉 supersedes 再来。")
                case .some(let other):
                    return toolResult(id: id, text: "ERROR: 人类 Todo #\(target) 现在动不了（\(other)）。**新条目也没有加。**")
                }
            }
            // 顺序、措辞、失败回执全部走共享剧本 `TodoLandingFlow` —— 四个调用点
            // 只有这一份，别在这儿自己拼字符串（#577 那一族靠的就是「文案和顺序
            // 只有一份」）。这条路上没有第三步：agent 加完不用叫醒谁，人类在
            // app 里看得到，所以 `.announced` 就是它的终点。
            var addWriteFailure: Error?
            guard let added = humanTodos.add(crewId: crewId, text: todoText,
                                             bySessionId: sessionId,
                                             bySenderName: sessionLabel,
                                             onWriteFailure: { addWriteFailure = $0 }) else {
                return toolResult(id: id, text: TodoLandingFlow.notPersistedReceipt(
                    ledger: .human, action: .added,
                    detail: addWriteFailure?.localizedDescription))
            }
            let announce = TodoLedger.human.newItemAnnouncement(
                number: added.number, text: todoText)
            var reached = TodoLandingFlow.Step.persisted
            var detail: String?
            do {
                // 群里那行带 `@human` 标记：这条是讲给人听的，别为它叫醒 agent
                // （human 不收窄可见范围，队友照样看得见 —— 2026-08-23 修过）。
                let incident = try store.appendSessionMessageReportingFailure(
                    crewId: crewId, sessionId: sessionId,
                    text: announce, category: "question",
                    senderName: sessionLabel,
                    mentions: TodoLandingFlow.mentions(.added).map(LocalWhiteboardMention.init),
                    inReplyTo: nil,
                    senderKind: isCaptain ? "captain" : "session")
                if let incident {
                    detail = incident
                } else {
                    reached = TodoLandingFlow.terminal(.added)
                }
            } catch {
                // 账已经落了，只是没宣布 —— 如实说，别让 agent 以为整件事没成。
                detail = Self.writeFailureReceipt(error)
            }
            var addReceipt = TodoLandingFlow.receipt(
                ledger: .human, action: .added, number: added.number,
                reached: reached, detail: detail)
            // 新的落好了才去撤旧的：反过来的话，撤成功但新增失败 = 那件事从人的
            // 账上整个消失了。宁可两条并存一瞬，不可一条都不剩。
            if let target = supersedes {
                addReceipt += "\n" + supersedeOldOne(target: target, replacedBy: added.number)
            }
            return toolResult(id: id, text: addReceipt)
        case "withdraw_human_todo":
            // 撤回（Todo #102）。顺序与 add_human_todo 一样：先落账再宣布，回执走
            // 同一个 `TodoLandingFlow` 出口。**这里多一件事**：撤不动的几种原因要
            // 分别说清楚 —— 「不是你提的」和「没这条」在 agent 那边该做完全不同的
            // 反应，压成一句「失败」它只会瞎重试。
            guard let number = (args["number"] as? Int)
                    ?? (args["number"] as? NSNumber)?.intValue else {
                return toolResult(id: id, text: "ERROR: number 必填 —— 填要撤回的那条人类 Todo 的 #N。")
            }
            let reason = ((args["reason"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let outcome = humanTodos.withdraw(crewId: crewId, number: number,
                                              sessionId: sessionId,
                                              senderName: sessionLabel, reason: reason,
                                              isCaptain: isCaptain)
            let withdrawn: LocalTodoItem
            switch outcome {
            case .withdrawn(let item):
                withdrawn = item
            case .reasonRequired:
                return toolResult(id: id, text: "ERROR: reason 不能为空 —— 撤回是把一件事从人的待办里拿走，"
                                  + "没有原因就是让它静默消失。一句话说清**变了什么**。")
            case .notFound:
                let live = humanTodos.list(crewId: crewId)
                    .filter { $0.withdrawnAt == nil }
                    .map { "#\($0.number) \($0.text.prefix(40))" }
                return toolResult(id: id, text: "ERROR: 人类 Todo #\(number) 不在这本账上（号写错了、或人类已经删掉了）。"
                                  + "**什么都没改。**当前还没撤的条目：\n"
                                  + (live.isEmpty ? "（空）" : live.joined(separator: "\n")))
            case .notYours(let owner):
                let who = owner.map { "「\($0)」" } ?? "（账上没记提出者，老条目）"
                return toolResult(id: id, text: "ERROR: 人类 Todo #\(number) 不是你提的，提出者是 \(who) —— **你只能撤自己提的**。"
                                  + "什么都没改。真该撤的话：提出者还在就让他自己撤；"
                                  + "**提出者已经不在了（或这本来就是条没记提出者的老条目）就找本 crew 的机长** —— "
                                  + "机长撤得动本 crew 的任何一条。别让它就这么挂在人的账上亮灯。")
            case .alreadyWithdrawn:
                return toolResult(id: id, text: "人类 Todo #\(number) 早就撤过了，这次没有重复动账，群里也不再发第二行。")
            case .ledgerUnavailable:
                return toolResult(id: id, text: TodoLandingFlow.notPersistedReceipt(
                    ledger: .human, action: .withdrawn))
            case .notWritten(let why):
                return toolResult(id: id, text: TodoLandingFlow.notPersistedReceipt(
                    ledger: .human, action: .withdrawn, detail: why))
            }
            let announced = announceWithdrawal(number: withdrawn.number, reason: reason)
            return toolResult(id: id, text: TodoLandingFlow.receipt(
                ledger: .human, action: .withdrawn, number: withdrawn.number,
                reached: announced.reached, detail: announced.detail))
        case "crew_ordering_signals":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let signalRows = LocalCrewStore.orgTreeLines(
                whiteboardDirectory: store.resolvedDirectory)
            var seenCrewIds = Set<String>()
            var rows: [CrewOrderingSignals.Row] = []
            // ③ 走磁盘镜像 —— helper 是另一个进程，读不到 app 的 UserDefaults。
            // **nil（读不出来）和空字典（确实一条没有）必须分开**：把前者当后者，
            // 就是把「我看不出来」报成「他确实没打开过」。
            let viewedMirror = CrewViewedStore.loadMirror(
                dataRoot: store.resolvedDirectory.deletingLastPathComponent())
            var anyOpenedRecorded = false
            let viewed = viewedMirror ?? [:]
            for row in signalRows where !seenCrewIds.contains(row.id) {
                seenCrewIds.insert(row.id)
                let messages = store.list(crewId: row.id)
                let lastAny = messages.last.flatMap { CrewTimestamp.parse($0.createdAt) }
                let lastHuman = messages.last { $0.senderKind == "user" }
                    .flatMap { CrewTimestamp.parse($0.createdAt) }
                let opened = viewed[row.id]
                if opened != nil { anyOpenedRecorded = true }
                rows.append(CrewOrderingSignals.Row(
                    crewId: row.id, title: row.title,
                    lastAnyMessageAt: lastAny, lastHumanMessageAt: lastHuman,
                    lastOpenedAt: opened))
            }
            let openedState: CrewOrderingSignals.OpenedColumn =
                viewedMirror == nil ? .unreadable
                    : (anyOpenedRecorded ? .hasData : .emptySoFar)
            return toolResult(id: id, text: CrewOrderingSignals.render(
                rows: rows, now: Date(), openedColumn: openedState))
        case "arrange_crews":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let arrangeReason = ((args["reason"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            guard !arrangeReason.isEmpty else {
                return toolResult(id: id, text: "ERROR: reason 不能为空 —— 它是显示给人看的。"
                                  + "人看到一个不合意的顺序时得分得清是规则算的还是你排的；没有理由的排布就是个黑箱，"
                                  + "第一次排错就会被永久关掉。")
            }
            let ids = ((args["crew_ids"] as? [Any]) ?? []).compactMap { $0 as? String }
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .filter { !$0.isEmpty }
            let arrangementURL = CrewArrangementStore.fileURL(
                whiteboardDirectory: store.resolvedDirectory)
            if ids.isEmpty {
                guard CrewArrangementStore.clear(at: arrangementURL) else {
                    return toolResult(id: id, text: "ERROR: 排布没能撤掉（文件删不了）"
                        + WriteReceipt.notWrittenMarker + "。**侧栏还是原来那个顺序。**")
                }
                _ = try? store.appendSessionMessageReportingFailure(
                    crewId: crewId, sessionId: sessionId,
                    text: "撤掉了侧栏排布，回到「按最近活动排」：\(arrangeReason)",
                    category: "progress", senderName: sessionLabel,
                    mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)],
                    inReplyTo: nil, senderKind: isCaptain ? "captain" : "session")
                return toolResult(id: id, text: "已撤掉排布，「总机长」视图回到纯基础序（最近有动静的在上）。")
            }
            let arrangement = CrewArrangement(
                crewIds: ids, reason: arrangeReason,
                bySessionId: sessionId, bySenderName: sessionLabel,
                createdAt: ISO8601DateFormatter().string(from: Date()))
            guard CrewArrangementStore.save(arrangement, to: arrangementURL) else {
                return toolResult(id: id, text: "ERROR: 排布" + WriteReceipt.notWrittenMarker
                    + "（磁盘写失败）。**侧栏还是原来那个顺序**，别当它已经生效。")
            }
            // 排完往群里说一声 —— 这既是「看得见是谁排的」的一半，也是让 app 那边
            // 立刻重读的那个 tick（排布文件不在被监听的白板目录里）。
            _ = try? store.appendSessionMessageReportingFailure(
                crewId: crewId, sessionId: sessionId,
                text: "把 \(ids.count) 个 crew 顶到了侧栏最前：\(arrangeReason)",
                category: "progress", senderName: sessionLabel,
                mentions: [LocalWhiteboardMention(kind: "human", targetId: nil)],
                inReplyTo: nil, senderKind: isCaptain ? "captain" : "session")
            return toolResult(id: id, text: "已排好 \(ids.count) 个 crew（理由会显示在侧栏顶上）。"
                              + "没提到的 crew 跟在后面、保持基础序；里面已经不存在的 id 会被忽略，不会让任何一行消失。")
        case "set_session_profile":
            let model = (args["model"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
                .flatMap { $0.isEmpty || $0.contains(" ") ? nil : $0 }
            let effort = (args["effort"] as? String)
                .map { $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() }
                .flatMap { $0.isEmpty || $0.contains(" ") ? nil : $0 }
            guard model != nil || effort != nil else {
                return toolResult(id: id, text: "ERROR: model / effort 至少给一个（单 token，别整句话）。")
            }
            // 对着本 session 那家的模型表说一句 —— **不拦**，只是别让「填错值」
            // 一路静默到 CLI 回显才暴露（Todo #36）。
            // set_session_profile 走**运行时**那条腿（claude 敲 /effort 斜杠命令）。
            let notes = profileAdvisories(model: model, effort: effort,
                                          agents: agentKey.map { [$0] } ?? ["claude", "codex"],
                                          phase: .runtime)
            let announceIncident = announceProfileAdvisories(
                notes, headline: "set_session_profile 的参数对不上模型表")
            if let failure = control.enqueueSetProfile(
                crewId: crewId, sessionId: sessionId, model: model, effort: effort) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "切换请求", error: failure, consequence:
                        "**本回合结束后不会切** —— 你还在原来的模型/effort 上，别按已经切了来规划。"))
            }
            let parts = [model.map { "模型→\($0)" }, effort.map { "effort→\($0)" }].compactMap { $0 }
            // 回执如实：**这里只是排队，还没切**。claude 的 /model /effort 是终端斜杠
            // 命令，你正在跑回合时写进去只会被排进消息队列、永远不当命令执行（#544
            // 的根因就是老回执谎称「立即生效」，机长信了，继续用旧模型跑到撞上限）。
            var receipt = """
                已排队切换：\(parts.joined(separator: "、"))。现在还没生效 —— claude 的 /model /effort \
                只能在终端空闲时执行，所以会在你**本回合结束后**才注入并核对回显。
                结果（成功或失败）都会回执到群聊白板，成功时你还会在终端收到一条通知。\
                别假定下一次工具调用已经在新模型上跑。
                撞额度上限时用它是对的：本回合被打断后切换就会落地，你会被叫醒在新模型上接着跑，不用等额度重置。
                codex 无中途切换通道（会在白板收到说明）。
                """
            if !notes.isEmpty {
                receipt += "\n⚠️ 参数提醒（已照常排队，没拦你；同一份提醒已发白板）：\n"
                    + notes.map { "· \($0)" }.joined(separator: "\n")
            }
            if let announceIncident { receipt += "\n⚠️ \(announceIncident)" }
            return toolResult(id: id, text: receipt)
        case "list_sessions":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let url = quotaDirectory.appendingPathComponent(CrewSessionsSnapshot.fileName)
            guard let data = try? Data(contentsOf: url),
                  let snap = try? JSONDecoder().decode(CrewSessionsSnapshot.self, from: data) else {
                return toolResult(id: id, text: "暂无成员状态快照（app 未在跑或刚启动）。")
            }
            // 产出证据（Todo #107 第三件：判活不判状态）。会话号账本一次读完
            // （每行现查会各上一次文件锁），取证面本身由 `SessionOutputProbe` 扫。
            let ledger = agentSessions.records(crewId: crewId)
            let probe = outputProbe
            let text = snap.renderRoster(crewId: crewId) { entry in
                guard let row = ledger[entry.sessionId] else {
                    // 没这条 = **还没记下会话号**，不是「没干活」：claude 的会话号是
                    // 起进程前指定的，codex 的要等握手回来才有。
                    return .unknown("还没记下它的 agent 会话号")
                }
                return probe.evidence(runnerKind: row.kind, agentSessionId: row.agentSessionId)
            }
            return toolResult(id: id, text: text)
        case "report_to_parent":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let msg = ((args["message"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !msg.isEmpty else { return toolResult(id: id, text: "ERROR: message 不能为空") }
            // 这一条是本族缺陷的**头号现场**：数据目录写失败的那 52 分钟里它照回
            // 「已提交向上汇报」，而父 crew 白板上一条都没有。它是子 crew 唯一的
            // 向上通道，它一撒谎，上级就以为下面没动静。
            if let failure = control.enqueueCrewMessage(
                crewId: crewId, sessionId: sessionId,
                direction: "to_parent", targetHint: nil, message: msg) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "这条向上汇报", error: failure, consequence:
                        "**上级那边什么都没有，也不会有回执** —— 请当作没汇报过。"
                        + "重试；仍不行就换条路（`contact` 直接打到父 crew 群里）。"))
            }
            return toolResult(id: id, text: "已提交向上汇报。送达结果（含「本 crew 无父」的情况）会回执到本 crew 群聊。")
        case "message_child_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let msg = ((args["message"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty, !msg.isEmpty else {
                return toolResult(id: id, text: "ERROR: crew 与 message 都不能为空")
            }
            if let failure = control.enqueueCrewMessage(
                crewId: crewId, sessionId: sessionId,
                direction: "to_child", targetHint: target, message: msg) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "给子 crew「\(target)」的消息", error: failure, consequence:
                        "**对方群里什么都没有，也不会有回执** —— 请当作未送达。"))
            }
            return toolResult(id: id, text: "已提交给子 crew「\(target)」的消息。送达/找不到的回执会出现在本 crew 群聊。")
        case "adopt_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: crew 不能为空") }
            if let failure = control.enqueueAdoptCrew(
                crewId: crewId, sessionId: sessionId, target: target) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "收编「\(target)」的请求", error: failure, consequence:
 "**组织树没有变**，也不会有回执。"))
            }
            return toolResult(id: id, text: "已提交收编「\(target)」。结果（含解析失败/成环被拒）会回执到群聊。")
        case "release_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let child = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !child.isEmpty else { return toolResult(id: id, text: "ERROR: crew 不能为空") }
            let dest = (args["to"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            if let failure = control.enqueueReleaseCrew(
                crewId: crewId, sessionId: sessionId,
                child: child, to: (dest?.isEmpty == false) ? dest : nil) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "把「\(child)」摘出/转挂的请求", error: failure, consequence:
 "**组织树没有变**，也不会有回执。"))
            }
            let destDesc = (dest?.isEmpty == false) ? "转挂到「\(dest!)」" : "摘出到顶层"
            return toolResult(id: id, text: "已提交把直系子「\(child)」\(destDesc)。结果会回执到群聊。")
        case "create_parent_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let rawParentTitle = (args["title"] as? String) ?? ""
            let parentTitle = rawParentTitle.split(whereSeparator: \.isWhitespace).joined(separator: " ")
            if let failure = control.enqueueCreateParentCrew(
                crewId: crewId, sessionId: sessionId,
                title: parentTitle.isEmpty ? nil : parentTitle) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "新建父 crew 的请求", error: failure, consequence:
 "**父 crew 没建**，本 crew 仍然没有上级。"))
            }
            return toolResult(id: id, text: "已安排在本 crew 头上新建父 crew。父机长起来后会报到；之后可用 report_to_parent 向它汇报、请它收编其它平级 crew。")
        case "adopt_parent":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let parent = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !parent.isEmpty else { return toolResult(id: id, text: "ERROR: crew 不能为空") }
            if let failure = control.enqueueAdoptParent(
                crewId: crewId, sessionId: sessionId, target: parent) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "认「\(parent)」为父的请求", error: failure, consequence:
 "**组织树没有变**，也不会有回执。"))
            }
            return toolResult(id: id, text: "已提交认「\(parent)」为父 crew。结果会回执到群聊。")
        case "create_child_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let brief = ((args["brief"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !brief.isEmpty else { return toolResult(id: id, text: "ERROR: brief 不能为空") }
            let rawTitle = (args["title"] as? String) ?? ""
            let title = rawTitle.split(whereSeparator: \.isWhitespace).joined(separator: " ")
            if let failure = control.enqueueCreateChildCrew(
                crewId: crewId, sessionId: sessionId,
                brief: brief, title: title.isEmpty ? nil : title) {
                return toolResult(id: id, text: WriteReceipt.notWritten(
                    what: "建子 crew 的请求", error: failure, consequence:
                        "**子 crew 没建，开场任务也没有任何地方存着** —— 那段 brief 只在你这儿，重试前别丢。"))
            }
            return toolResult(id: id, text: "已安排建子 crew。开场任务会写入子群并交给子机长执行；送达失败会在本群回执。")
        default:
            return toolResult(id: id, text: "ERROR: 未知工具 \(name ?? "nil")")
        }
    }

    // MARK: - 通讯录 contact（2026-08-11）

    /// 共享文件层目录（helper 的 `--dir`）—— 白板 / quota / 点名快照 / 通讯录
    /// （`local-crews.json` 在其父目录）都在这一份下面。

    /// 发一条群消息（`post_to_crew` 的单条本体）。分条发送把它当零件循环调用，
    /// 所以它返回**结果**而不是直接返回 JSON —— 一次批量里每条的成败要分别记账。
    ///
    /// `ok == false` = **这一条没有发出去**（参数不合法 / 落账失败 / 白板写失败）。
    private func postToCrewOnce(args: [String: Any]) -> (ok: Bool, text: String) {
            let message = (args["message"] as? String) ?? ""
            // Todo #48：附件（本机绝对路径）→ 收进 attachments/<crewId>/。判定与
            // 软报错文案跟人类拖入共用 `CrewFileAttachmentIntake`，不另立一套口径。
            let givenPaths = (args["attachments"] as? [Any])?.compactMap { $0 as? String } ?? []
            let intake = givenPaths.isEmpty
                ? (accepted: [LocalWhiteboardAttachment](), errors: [String]())
                : CrewFileAttachmentIntake.intake(
                    paths: givenPaths, crewId: crewId, root: attachmentRoot)
            let hasBody = !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            // 只发图（正文为空）放行 —— 与人类 composer 同规则。但**一张都没收下**
            // 时不能当成空消息发出去：那会变成一条空气泡 + 一句「已发到」，
            // 而图其实一张都没进去。
            guard hasBody || !intake.accepted.isEmpty else {
                let why = intake.errors.isEmpty
                    ? "" : "\n" + intake.errors.joined(separator: "\n")
                return (false, givenPaths.isEmpty
                        ? "ERROR: message 不能为空"
                        : "ERROR: 正文为空，且附件一个都没收下，这条没有发出去。\(why)")
            }
            // Phase 7：解析定向 @ + reply_to,记进本地白板（不静默吞）。本地这步只
            // 负责把信息留住；按 mention 唤醒目标 session 由 app 侧的本地直投
            // （CrewLocalMentionWaker / CrewLocalMentionDelivery）接。
            let given = parseMentions(args["mentions"])
            let replyTo = (args["reply_to"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            // Todo #14 ①：`reply_to` 此前**只**进了下面的 `inReplyTo:` —— 而工具
            // 描述和世界观模板都写着「给了会自动 @ 原发送者」。危害是静默的：
            // agent 以为自己点名了，一个 mention 都没长出来，回执照样回「已发到」，
            // 然后它安心等一个永远不会醒的人。
            let mentions = mentionsWithReplyAutoMention(given, replyTo: replyTo)
            // ── #115/#120：分类**真的驱动落账** ────────────────────────────────
            //
            // 人类原话的根：「不然第一 todo、cockpit等等不能及时更新」。做成一个
            // 标签颜色 = 白做，所以这里必须真的去写那本账。
            //
            // **顺序照 `TodoLandingFlow` 写死的那条：落账 → 发群，一步都不许跳。**
            // 这一单最坏的结果不是「落账失败」，是**消息发出去了、账没落上** ——
            // 那比现在还糟，因为人会**以为**账更新了。所以下面每一处失败都是
            // `return`，消息一个字都不发。
            //
            // 驾驶舱那四类（plan/progress/blocked/done）走 `landOnCockpit`，
            // 写入口复用 `CockpitPlanStore.add/update`（不另开一个）。谁写得动由
            // `CrewCockpitWritePermission` 判：**报进度的人 ≠ 决定条目存不存在的人**。
            // #136：这次发言顺手报一句**整组**的状态（侧栏那一行读它）。
            // 拒了就一个字都不发 —— 半截状态（消息发了、状态没落）会让侧栏显示
            // 一句过期的话，而看的人以为那是刚报的。
            // #143：作者自己写的那一行结论。**空白当没给** —— 写个空格就算写过，
            // 是最廉价的一种假账（同 `crew_status` 那条）。
            let headlineRaw = ((args["headline"] as? String) ?? "")
                .trimmingCharacters(in: .whitespacesAndNewlines)
            let headline: String? = headlineRaw.isEmpty ? nil : headlineRaw
            var crewStatus: String?
            var statusHint: String?
            switch CrewStatusIntake.decide(args["crew_status"], isCaptain: isCaptain) {
            case .none:
                break
            case let .refused(why):
                return (false, "ERROR: " + why)
            case let .accepted(value, hint):
                crewStatus = value
                statusHint = hint
            }
            var ledgerReceipts: [String] = []
            // #132/#133：这条消息**指向**了什么。每一项都来自结构化字段或落账刚拿到
            // 的号 —— 正文一个字都不参与（见 `CrewMessageReference`）。
            var refs = CrewMessageReferences.Input(
                planNumber: CrewCockpitLanding.number(args["plan"]),
                inReplyTo: replyTo,
                mentionedSessionIds: (mentions ?? []).compactMap {
                    $0.kind == "session" ? $0.targetId : nil
                })
            // (D)：谁写得动驾驶舱那本账。**拒了就一个字都不发** —— 跟落账失败同一条
            // 纪律：消息发出去了、账没落上，比现在还糟。
            if let why = cockpitPermissionRefusal(args: args) {
                return (false, "ERROR: " + why + "\n**这条消息也没有发出去。**")
            }
            switch CrewCategoryRouting.decide(category: args["category"] as? String, args: args,
                                              isCaptain: isCaptain) {
            case let .refuse(why):
                return (false, "ERROR: " + why)
            case .land(.humanTodo):
                guard let added = humanTodos.add(crewId: crewId, text: message,
                                                 bySessionId: sessionId,
                                                 bySenderName: sessionLabel) else {
                    return (false, "ERROR: 这条人类 Todo" + WriteReceipt.notWrittenMarker
                        + "（那本账这次读不出来或落不了盘），"
                        + "**这条消息也没有发出去** —— 顺序是「落账 → 发群」，账没落上就不发，"
                        + "免得你以为账更新了。（白板上有一条系统警示说明是哪种事故。）")
                }
                refs.humanTodoNumber = added.number
                ledgerReceipts.append("已建 人类 Todo #\(added.number)（人在面板里点得进去、撤得掉）")
            case let .skipped(hint):
                // 不落账，但**回执里说清楚它没落**——静默跳过就是这一单要治的病本身。
                ledgerReceipts.append("⚠️ " + hint)
            case let .land(cockpit) where cockpit.ledger == .cockpit:
                switch landOnCockpit(category: cockpit, args: args, message: message) {
                case let .refused(why):
                    return (false, why)
                case let .landed(planNumber, receipt):
                    refs.planNumber = planNumber
                    ledgerReceipts.append(receipt)
                }
            case .land, .noLedger:
                break
            }
            // #120：Todo 号是**独立参数**，跟 category 正交 —— 一条消息可能既是进度、
            // 又对应一条 Todo，绑在某个分类上只会让写的人纠结「这算哪一类」。
            switch CrewMessageTodoLink.decide(args: args) {
            case let .refuse(why):
                return (false, "ERROR: " + why)
            case .none:
                break
            case let .update(number, status):
                // ⚠️ 凭据闸走**原来那一份**（`judgeCompletionEvidence` 会当场解析
                // commit）。`CrewMessageTodoLink` 只查了「有没有给」——
                // 拿它的绿当凭据验过了，那道用 191 小时假账换来的闸就被放宽了。
                if status == "completed" {
                    switch judgeCompletionEvidence(args: args) {
                    case .commitResolved, .prose:
                        break
                    case .malformedCommit, .commitNotFound, .cannotVerify, .missing:
                        return (false, "ERROR: 要把 Todo #\(number) 挂成"
                            + "「完成」，凭据这一关没过（`evidence_commit` 会被当场解析）。"
                            + "**这条消息也没有发出去。** 还没做完就先挂 `in_progress`，"
                            + "做完拿到凭据再来。")
                    }
                }
                guard let updated = todos.respond(
                    crewId: crewId, number: number, sessionId: sessionId,
                    senderName: sessionLabel, text: message, newStatus: status) else {
                    return (false, "ERROR: Todo #\(number) 的更新"
                        + WriteReceipt.notWrittenMarker
                        + "（这本账上没有这条，或者这次读不出来 / 落不了盘）。"
                        + "**这条消息也没有发出去。**")
                }
                refs.agentTodoNumber = updated.number
                ledgerReceipts.append("已把 Todo #\(updated.number) 翻成 \(status)")
            }
            // 机长发言标 senderKind "captain" —— 渲染端据此用稳定的 captainBotId
            // 当头像种子（成员列表与气泡同一张脸），并点亮星标。
            //
            // 回执如实（#577）：走 ReportingFailure 变体，落盘失败就说没发出去 ——
            // 此前无论写没写成都回一句「已发到」，白板读不出来时消息全丢还报成功。
            do {
                let incident = try store.appendSessionMessageReportingFailure(
                    crewId: crewId, sessionId: sessionId,
                    text: message, category: args["category"] as? String,
                    senderName: sessionLabel,
                    mentions: mentions, inReplyTo: replyTo,
                    senderKind: isCaptain ? "captain" : "session",
                    attachments: intake.accepted,
                    references: CrewMessageReferences.build(refs),
                    crewStatus: crewStatus,
                    headline: headline)
                // 回执如实（#577）：发出去了几张、哪几张没收下，都得说 —— 只说
                // 「已发到」而漏掉「那张图没进去」，跟当初「写没写成都回已发到」
                // 是同一个病：agent 以为图递过去了，接收方那边什么都没有。
                let base = Self.postReceipt(
                    incident: incident,
                    attachmentCount: intake.accepted.count,
                    attachmentErrors: intake.errors)
                // 落账结果必须进回执：**「落账可撤」的前提是先让人知道它落了哪一条。**
                // #143：长消息没给结论时，把**猜出来的那一行**摆给作者看。
                // schema 里的说明只在写之前被读到一次（多半没读），真正教得会人的
                // 是出错那一刻的这句话。
                let guessHint = CrewMessageFold.receiptHintIfGuessed(
                    text: message, headline: headline)
                // #142：`question` 却没指定问谁 —— 不落账 + 不在他眼前 = 问出去就没了。
                // 只管这一类能从结构上证明的，别的等收件人真变成字段（人类 Todo #16）。
                let addressHint = CrewMessageRecipients.receiptHintIfUnaddressed(
                    category: args["category"] as? String,
                    mentionKinds: (mentions ?? []).map(\.kind))
                return (true, ([base] + ledgerReceipts
                               + [statusHint, guessHint, addressHint].compactMap { $0 })
                    .joined(separator: "\n"))
            } catch {
                return (false, Self.writeFailureReceipt(error))
            }
    }

    private var sharedDirectory: URL { quotaDirectory }

    /// `contact(to, message)`：按号码往目标 crew 的群里发一条消息。
    ///
    /// **投递复用现有链路，不另造**：写目标 crew 白板（广播不带 mentions、`-1` 带
    /// @captain、`-N` 带 @session）→ app 侧现成的 `CrewLocalMentionWaker` 负责唤醒 /
    /// 拉起（跨 crew 来电的广播按 @机长处理，见 `CrewLocalMentionWakeLogic`）。
    /// helper 这边只做寻址 + 两次白板 append，纯文件层，不需要 app 执行。
    ///
    /// 失败一律明说（#577）：号码不合法 / 查无此号 / 白板写不进去，都不静默丢。
    private func handleContact(id: Any?, args: [String: Any]) -> String? {
        let toRaw = ((args["to"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        let message = ((args["message"] as? String) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !toRaw.isEmpty else {
            return toolResult(id: id, text: "ERROR: to 不能为空 —— 填目标号码（如 7 或 7-3），用 directory 查。")
        }
        guard !message.isEmpty else {
            return toolResult(id: id, text: "ERROR: message 不能为空。")
        }
        guard let number = CrewPhoneNumber.parse(toRaw) else {
            return toolResult(
                id: id,
                text: "ERROR: 「\(toRaw)」不是有效号码。号码形如 7（整个 crew）或 7-3（某个成员，"
                    + "-1 恒是机长）。用 directory 查号。")
        }
        let directory: CrewDirectory
        do {
            directory = try CrewDirectory.load(whiteboardDirectory: sharedDirectory)
        } catch let failure as CrewDirectory.Unavailable {
            // 「读不到名单」不是「这个号不存在」。说错了人会去查号码，而号码是对的。
            return toolResult(id: id, text: "ERROR: " + failure.message)
        } catch {
            return toolResult(
                id: id, text: "ERROR: 通讯录读不出来：\(error.localizedDescription)")
        }
        guard let target = directory.resolve(number) else {
            return toolResult(
                id: id,
                text: "ERROR: 查无此号 \(number.text) —— 本机没有这个 crew / 这个分机。"
                    + "用 directory 查一下现有号码（号码永不回收，但从来没发过的号自然查不到）。")
        }
        guard target.crewId != crewId else {
            return toolResult(
                id: id,
                text: "ERROR: \(number.text) 就是你自己所在的群。本群的事直接用 post_to_crew"
                    + "（要点名某个成员就带 mentions），别绕一圈打给自己。")
        }
        // 署名：目标群里必须一眼看出这是外线打进来的 —— 带来源 crew 名 + 来源号码。
        let sourceTitle = directory.title(ofCrew: crewId) ?? crewId
        let myNumber = directory.phoneNumber(crewId: crewId, sessionId: sessionId,
                                             isCaptain: isCaptain)
        let signature = "\(sourceTitle) · \(myNumber?.text ?? sessionLabel ?? sessionId)"
        let mentions: [LocalWhiteboardMention]?
        switch target {
        case .broadcast:
            mentions = nil                                       // 广播：跟人类无 @ 发言一致
        case .captain:
            mentions = [LocalWhiteboardMention(kind: "captain", targetId: nil)]
        case .session(_, _, let sid, _):
            mentions = [LocalWhiteboardMention(kind: "session", targetId: sid)]
        }
        // 发送者标 "session"（不用 "captain"）：渲染端会拿本 crew 的 captainBotId 当
        // 头像种子，外线来电挂上目标 crew 机长的脸就全错了。身份靠署名 + 号码说清。
        let incident: String?
        do {
            incident = try store.appendSessionMessageReportingFailure(
                crewId: target.crewId, sessionId: sessionId, text: message,
                category: "contact", senderName: signature,
                mentions: mentions, senderKind: "session",
                externalContactFrom: myNumber?.text ?? sourceTitle,
                // #132：外线来电挂一颗指回**来电方**的机组胶囊 —— 收到的人点它就能
                // 找回是谁打的。号码是这一刻算出来的，不是从署名那行文本里抠的。
                references: CrewMessageReferences.build(
                    .init(crewNumber: myNumber?.text)))
        } catch {
            return toolResult(id: id, text: WriteReceipt.notWritten(
                what: "发给 \(number.text) 的这条消息", error: error,
                consequence: "请当作未送达处理（对方群里什么都没有）。"))
        }
        // 源群回执：让组织上看得见谁跨线找了谁。写不进去只在工具回执里说一声 ——
        // 正文已经送到对方群了，不能因为回执失败就谎报「没送到」。
        let snippet = Self.contactSnippet(message)
        var receiptIncident: String? = nil
        do {
            _ = try store.appendSessionMessageReportingFailure(
                crewId: crewId, sessionId: sessionId,
                text: "已联系 \(number.text)（\(target.displayName)）：\(snippet)",
                category: "progress", senderName: sessionLabel,
                senderKind: isCaptain ? "captain" : "session",
                // #132：这行回执挂一颗指向**目标机组**的胶囊。`number` 是 `contact`
                // 解析出来的号码对象，不是从这句话里认出来的。
                references: CrewMessageReferences.build(.init(crewNumber: number.text)))
        } catch {
            receiptIncident = "本群那行「已联系」回执" + WriteReceipt.notWrittenMarker
                + "（\(error.localizedDescription)）——"
                + "消息本身已送达，但组织上看不见你打过这通电话。"
        }
        let wakeNote: String
        switch target {
        case .broadcast: wakeNote = "对方机长会被叫醒（等同于在他们群里无 @ 发言）"
        case .captain: wakeNote = "对方机长会被叫醒"
        case .session: wakeNote = "对方那个 session 会被叫醒；它若已退出会被重新拉起"
        }
        var text = "已发到 \(number.text)（\(target.displayName)）的群聊白板，署名「\(signature)」。\(wakeNote)。"
        if let incident { text += "\n⚠️ 但请注意：\(incident)" }
        if let receiptIncident { text += "\n⚠️ \(receiptIncident)" }
        return toolResult(id: id, text: text)
    }

    /// 源群回执里带的正文摘要：折成单行、留个开头就够（细节在对方群里）。
    static func contactSnippet(_ message: String, limit: Int = 40) -> String {
        let flat = message.split(whereSeparator: \.isNewline)
            .joined(separator: " ")
            .trimmingCharacters(in: .whitespaces)
        guard flat.count > limit else { return flat }
        return String(flat.prefix(limit)) + "…"
    }

    /// 宽容的 ISO8601 解析：带/不带小数秒都收（agent 生成的时间串两种都常见）。
    static func parseISO(_ s: String) -> Date? {
        let f1 = ISO8601DateFormatter()
        if let d = f1.date(from: s) { return d }
        let f2 = ISO8601DateFormatter()
        f2.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f2.date(from: s)
    }

    /// 阻塞 long-poll 直到待决策被答复或到上限，返回答复文本 / 超时占位。
    /// **绝不无限等**：默认 ~30min 上限（3600 × 0.5s，与 codex 审批 provider 一致）——
    /// 一个没人答的 `ask` 若一直阻塞，会让 codex 那一轮的 `tools/call` 永不返回，整个
    /// turn 卡死在「运行中…」（codex session 跑半天不出东西的根因之一）。到点返回
    /// 「自行判断」让 agent 继续。`pollInterval`/`maxWaits` 给单测调小。
    /// inspect/nudge 命令应答的 long-poll 参数（单测把它调小免等真超时）。
    /// 默认 0.25s × 40 ≈ 10s —— app 的目录监听 tick 通常亚秒级就应答。
    var commandResponsePollInterval: TimeInterval = 0.25
    var commandResponseMaxWaits = 40

    /// long-poll 一条机长命令的应答文件（`LocalCrewControlStore.takeCommandResponse`）。
    /// 超时 → 提示 app 可能没在跑（helper 是离线子进程，只有 app 活着才有人执行命令）。
    /// long-poll 一条命令的应答。`maxWaits` 默认 `commandResponseMaxWaits`（10 秒够
    /// inspect/nudge/stop 这类瞬时操作）；**真会干活一阵子的命令要显式放宽**
    /// —— 比如 `change_workdir` 要复制整个项目记忆、还可能重试写
    /// `~/.claude.json`。超时那句必须留活口：命令**可能已经在执行**，别让机长以为没跑。
    func awaitCommandResponse(commandId: String, maxWaits: Int? = nil,
                              timeoutHint: String? = nil) -> String {
        var waits = 0
        let budget = maxWaits ?? commandResponseMaxWaits
        while waits < budget {
            if let text = control.takeCommandResponse(crewId: crewId, commandId: commandId) {
                return text
            }
            waits += 1
            Thread.sleep(forTimeInterval: commandResponsePollInterval)
        }
        return "（超时无应答 —— PendingCrew app 可能没在运行，或该命令未被执行。）"
            + (timeoutHint.map { " " + $0 } ?? "")
    }

    /// `ask` 阻塞等答复的预算（30 分钟）。**做成实例属性只为让单测调小** ——
    /// 一把要跑全部工具的尺子不能被一条 30 分钟的 long-poll 拖住。
    var askReplyMaxWaits = 3600
    var askReplyPollInterval: TimeInterval = 0.5

    func awaitReply(reqId: String, pollInterval: TimeInterval? = nil, maxWaits: Int? = nil) -> String {
        let pollInterval = pollInterval ?? askReplyPollInterval
        let maxWaits = maxWaits ?? askReplyMaxWaits
        var waits = 0
        while waits < maxWaits {
            if let it = approvals.item(crewId: crewId, id: reqId), it.status == "answered" {
                return it.reply ?? "（已答复，无文本）"
            }
            waits += 1
            Thread.sleep(forTimeInterval: pollInterval)
        }
        let timeoutReply = "（暂无人响应 —— 请自行判断后继续）"
        // 这个 MCP 调用已经要返回；之后点卡片的答复不可能再送达 agent。
        // 和 Codex manual approval bridge 的超时路径一样，必须把持久卡片同步结束，
        // 否则 session 虽已开始后续回合，仍会永久显示“等答复”。原子条件更新避免
        // 覆盖最后一拍同时到达的真实答复。
        if approvals.answerIfPending(crewId: crewId, id: reqId, reply: timeoutReply) {
            return timeoutReply
        }
        if let item = approvals.item(crewId: crewId, id: reqId), item.status == "answered" {
            return item.reply ?? "（已答复，无文本）"
        }
        return timeoutReply
    }

    /// 把 `post_to_crew` 的 `mentions` 参数（JSON 数组）解析成本地 mention 模型。
    /// 宽容解析：丢掉缺 `kind` 的坏条目，但保留其余 —— 别因一条坏 mention 把整条
    /// 消息或全部 mention 静默吞掉。空 / 非数组 → nil（= 广播）。
    private func parseMentions(_ raw: Any?) -> [LocalWhiteboardMention]? {
        guard let arr = raw as? [[String: Any]] else { return nil }
        let parsed: [LocalWhiteboardMention] = arr.compactMap { item in
            guard let kind = item["kind"] as? String, !kind.isEmpty else { return nil }
            let target = (item["target_id"] as? String).flatMap { $0.isEmpty ? nil : $0 }
            return LocalWhiteboardMention(kind: kind, targetId: target)
        }
        return parsed.isEmpty ? nil : parsed
    }

    /// `post_to_crew(reply_to:)` 的自动 @（Todo #14 ①）：把「被回复那条的发送者」
    /// 并进要落盘的 mentions。`replyTo` 为 nil / 指向一条找不到的消息 / 发送者转不成
    /// @ 目标 → 原样返回，**不编一个 @ 出来**。
    ///
    /// 两件事都**不在这里判**，一个字都没抄：
    ///   * 「回复谁 @ 谁」→ `CrewReplyTargetBuilder.mention(senderKind:…)`；
    ///   * 「合出来是什么形状」→ `CrewComposerMentionParser.mentionsToSend(staged:replyTo:)`，
    ///     人类 composer 走的是同一个函数。它带着两道守卫（调用方已手打定向 @ 时
    ///     不放宽 / 已显式给了 `broadcast` 时不再叠），在这条路上照样要成立 ——
    ///     所以只能复用，不能在这儿另写一份判定。
    ///
    /// 结果形状是 **`[broadcast, 被回复者]`**，不是裸 `session(X)`：裸 session 会
    /// **收窄可见范围**，让每一条回复悄悄变私信（理由见那个纯函数上的注释）。
    /// 全组照样看得见，只是把被回复的那个叫醒。
    ///
    /// 这里现读一次白板全量。`list` 是 flock + 整份解码，但这是 helper 子进程、
    /// 不是 SwiftUI 的 body 路径（第 3 条红线判的是后者），而且只在真给了
    /// `reply_to` 时才读。
    private func mentionsWithReplyAutoMention(
        _ given: [LocalWhiteboardMention]?, replyTo: String?
    ) -> [LocalWhiteboardMention]? {
        guard let replyTo,
              let target = store.list(crewId: crewId).first(where: { $0.id == replyTo }),
              let replied = CrewReplyTargetBuilder.mention(
                senderKind: target.senderKind,
                senderSessionId: target.senderSessionId,
                senderUserId: target.senderUserId)
        else { return given }

        let staged = (given ?? []).map { CrewStagedMention(token: "", mention: CrewMention($0)) }
        let sent = CrewComposerMentionParser.mentionsToSend(staged: staged, replyTo: replied)
        return sent.isEmpty ? nil : sent.map(LocalWhiteboardMention.init)
    }

    private func senderLabel(_ m: LocalWhiteboardMessage) -> String {
        // 与 HookEmitter.render 同款：有显示名优先用名，无名退回旧格式。
        if let name = m.senderName, !name.isEmpty {
            return name
        }
        switch m.senderKind {
        case "session": return "session:\(m.senderSessionId ?? "?")"
        case "user": return "人类"
        default: return m.senderKind
        }
    }

    private func integerArgument(_ raw: Any?) -> Int? {
        (raw as? Int) ?? (raw as? Double).map(Int.init)
    }

    private func renderRow(_ m: LocalWhiteboardMessage) -> String {
        let who = senderLabel(m)
        // agentText = 正文 + 附件绝对路径提示行（Todo #3 群聊图片）。
        return "[\(m.createdAt)] \(who): \(m.agentText)"
    }

    // MARK: - 可用模型表（Todo #37）与参数提醒（Todo #36）

    /// 现探表（app 的 `ModelCatalogCenter` 每 6 小时落一次盘）。读不到 → nil，
    /// 取用方经 `resolveTable` 回落手工兜底表。每次现读：helper 是常驻子进程，
    /// 缓存下来就会一直用启动那一刻的旧表。
    private var modelCatalogFile: AgentModelCatalogFile? {
        AgentModelCatalogFile.load(from: quotaDirectory)
    }

    /// 工具描述尾巴：每家一行「可用模型 + effort + 不选时跑什么 + 新鲜度警示」。
    /// `AgentModelCatalog.summaryLine` 把警示串在同一行里 —— 别在这里拆开，
    /// 拆开就会有调用方只取清单不取警示，那就等于把过时的表当事实呈现了。
    private func sanitizedProfileToken(_ raw: String?, lowercased: Bool) -> String? {
        guard var value = raw?.trimmingCharacters(in: .whitespacesAndNewlines),
              !value.isEmpty, !value.contains(where: \.isWhitespace) else { return nil }
        if lowercased { value = value.lowercased() }
        return value
    }

    private func catalogHint(agents: [String]) -> String {
        let file = modelCatalogFile
        let lines = agents.map { agent -> String in
            guard let table = AgentModelCatalogFile.resolveTable(agent: agent, file: file) else {
                return AgentModelCatalog.missingLine(for: agent)
            }
            var line = AgentModelCatalog.summaryLine(for: table)
            if let err = file?.error(agent: agent) { line += "（本轮探测：\(err)）" }
            return line
        }
        return "【可用模型表】" + lines.joined(separator: "\n")
    }

    /// 对着表检查 model / effort，返回要说的话。**只产出提醒，从不拦截** ——
    /// 实测过：不在活表里的旧别名后端往往仍解析得了（见 `AgentModelCheck`）。
    ///
    /// `agents` 给多家时（runner 没指定 / 不知道本 session 是哪家）：任一家认得
    /// 就不吭声；都不认得才说话，措辞取第一家的裁决。
    private func profileAdvisories(model: String?, effort: String?, agents: [String],
                                   phase: AgentModelEffortPhase) -> [String] {
        let file = modelCatalogFile
        let tables = agents.map { ($0, AgentModelCatalogFile.resolveTable(agent: $0, file: file)) }
        var out: [String] = []

        func advise(_ value: String, knob: String,
                    check: (AgentModelTable?) -> AgentModelCheck) -> String? {
            var first: (String, AgentModelCheck)?
            for (agent, table) in tables {
                let verdict = check(table)
                if verdict == .ok { return nil }          // 任一家认得 → 闭嘴
                if first == nil { first = (agent, verdict) }
            }
            guard let (agent, verdict) = first else { return nil }
            return AgentModelValidator.message(verdict, knob: knob, value: value, agent: agent)
        }

        if let model, let note = advise(model, knob: "model", check: {
            AgentModelValidator.checkModel(model, table: $0)
        }) { out.append(note) }
        if let effort, let note = advise(effort, knob: "effort", check: {
            AgentModelValidator.checkEffort(effort, model: model, table: $0, phase: phase)
        }) { out.append(note) }
        return out
    }

    /// 把参数提醒摆到群聊白板上 —— 这就是 Todo #36 说的 fail-loud 落点：
    /// 填了表里没有的值不许只烂在工具回执里（那只有调用方自己看得到）。
    /// 空提醒不发（没事就别刷屏）。贴不上去时把这件事本身报进回执（#577），
    /// 别让「fail-loud 落点」自己悄悄失声。
    private func announceProfileAdvisories(_ notes: [String], headline: String) -> String? {
        guard !notes.isEmpty else { return nil }
        do {
            return try store.appendSessionMessageReportingFailure(
                crewId: crewId, sessionId: sessionId,
                text: "⚠️ \(headline)：\n" + notes.map { "· \($0)" }.joined(separator: "\n"),
                category: "error", senderName: sessionLabel,
                senderKind: isCaptain ? "captain" : "session")
        } catch {
            return "上面的提醒没能贴到群聊白板 —— \(error.localizedDescription)"
        }
    }

    // MARK: - 写工具回执（#577：写没写成必须说实话）

    /// 白板写成功时的回执。`incident` 非 nil = 写进去了，但白板此前出过事 ——
    /// 一并报出来，别让「已发到」把归档 + 重建这件事盖过去。
    private static func postReceipt(
        incident: String?, attachmentCount: Int = 0, attachmentErrors: [String] = []
    ) -> String {
        var line = "已发到 crew 群聊白板。"
        if attachmentCount > 0 { line += "（带 \(attachmentCount) 个附件）" }
        if let incident { line += "⚠️ 但请注意：\(incident)" }
        // 收不下的逐条附在后面 —— 这几句是这条回执里唯一会说「有东西没发出去」的
        // 地方，前半句的「已发到」只管正文。
        if !attachmentErrors.isEmpty {
            line += "\n⚠️ 以下附件没有发出去：\n" + attachmentErrors.joined(separator: "\n")
        }
        return line
    }

    /// 白板写失败时的回执。措辞按「当没送达处理」写死 —— 调用方（编码 agent）看到
    /// 这句要知道刚才那段话群里没人看得见。
    /// 判销号凭据。**解析用的仓库是那条 crew 登记在册的工作目录，不是我的 cwd。**
    ///
    /// 拿不到登记的 workdir 就报 `.unavailable` —— 绝不退回去从 cwd 往上找 git 根：
    /// 往上找会找到「一个」仓库，但不保证是「那个」（worktree、`/private/tmp` 下的
    /// 发包树、别的项目仓都可能在祖先链上）。那条路的失败形态是**在错的仓库里解析
    /// 成功**，于是一条假账带着一句「凭据解析成功」挂上去。**验不了会逼人换条路；
    /// 假绿不会。**
    /// `landOnCockpit` 的结局。**不用 `Result`**：失败侧是一句给 agent 看的话，
    /// 不是 `Error`。
    enum CockpitLandingOutcome: Equatable {
        /// 落成了：动的是**哪一条**计划，以及给调用方的那句回执。
        /// 号码要带出来 —— 它同时是这条消息的引用（#132），
        /// 而引用**只能从这种结构化的地方来**，不能事后从回执文本里正则抠。
        case landed(planNumber: Int, receipt: String)
        case refused(String)
    }

    /// (D) 那道门：这条分类要写驾驶舱，而这个身份写不动时，把话原样回出去。
    ///
    /// **两处调用**（单条 / 分条批量的预校验）用同一份，免得批量那条路悄悄放宽 ——
    /// 分条之后「一半成功」本来就是新的失败形态。
    private func cockpitPermissionRefusal(args: [String: Any]) -> String? {
        let raw = ((args["category"] as? String) ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard let category = CrewMessageCategory(rawValue: raw) else { return nil }
        if case let .refused(why) = CrewCockpitWritePermission.decide(
            category: category, isCaptain: isCaptain) {
            return why
        }
        return nil
    }

    /// 分类 → 驾驶舱那本账。**写入口复用 `CockpitPlanStore`，不另开一个。**
    ///
    /// 每一处失败都是 `.failure`，调用方据此**一个字都不发** —— 顺序是「落账 → 发群」，
    /// 账没落上就不发，免得人以为板上已经更新了。
    private func landOnCockpit(category: CrewMessageCategory, args: [String: Any], message: String)
        -> CockpitLandingOutcome {
        let unsent = "\n**这条消息也没有发出去。**"
        if category == .plan {
            let title = CrewCockpitLanding.title(from: message)
            guard let planned = plans.add(crewId: crewId, title: title,
                                          bySessionId: sessionId, byName: sessionLabel) else {
                return .refused("ERROR: 这条计划" + WriteReceipt.notWrittenMarker
                    + " —— 任务列表这次读不出来或落不了盘"
                    + "（群聊白板上有一条系统警示说明是哪种事故）。" + unsent)
            }
            return .landed(planNumber: planned.number,
                receipt: "已排上 计划 #\(planned.number)：\(planned.title)（没做）"
                + " —— 标题是从这条消息第一行取的，不对就 `plan_update` 改。")
        }
        guard let number = CrewCockpitLanding.number(args["plan"]), number >= 1 else {
            return .refused("ERROR: `plan` 需为正整数（驾驶舱里那条计划的 #N）。" + unsent)
        }
        var statusRaw: String?
        var blocker: CockpitPlanBlocker?
        switch category {
        case .progress:
            // 只把「没做」翻成「进行中」；blocked 的不动（不许顺手把卡点清掉）。
            statusRaw = CrewCockpitLanding.statusForProgress(current:
                plans.item(crewId: crewId, number: number).flatMap { CockpitPlan.status($0.status) })
        case .blocked:
            statusRaw = CockpitPlanStatus.blocked.rawValue
            switch CrewCockpitLanding.blockerLedger(args["blocked_by_ledger"]) {
            case let .refused(why):
                return .refused("ERROR: " + why + unsent)
            case let .ok(ledger):
                guard let blockerNumber = CrewCockpitLanding.number(args["blocked_by_number"]),
                      blockerNumber >= 1 else {
                    return .refused("ERROR: `blocked_by_number` 需为正整数"
                        + "（卡在哪条 Todo 的 #N）。" + unsent)
                }
                blocker = CockpitPlanBlocker(ledger: ledger, number: blockerNumber)
            }
        case .done:
            // ⚠️ 凭据闸走**原来那一份**（当场解析 commit）。翻完成只有机长能做，
            // 但「有资格判」不等于「判过了」—— 那笔挂了 191 小时的假账带的正是一个
            // 根本不存在的 hash。
            switch judgeCompletionEvidence(args: args) {
            case .commitResolved, .prose:
                break
            case .malformedCommit, .commitNotFound, .cannotVerify, .missing:
                return .refused("ERROR: 要把 计划 #\(number) 翻成「完成」，凭据这一关没过"
                    + "（`evidence_commit` 会被当场解析）。还没做完就先标 `progress`，"
                    + "做完拿到凭据再来。" + unsent)
            }
            statusRaw = CockpitPlanStatus.done.rawValue
        default:
            break
        }
        switch plans.update(crewId: crewId, number: number, progress: message,
                            statusRaw: statusRaw, blocker: blocker,
                            bySessionId: sessionId, byName: sessionLabel) {
        case let .failure(failure):
            return .refused("ERROR: 计划 #\(number) 没更新成 —— " + failure.summary + unsent)
        case let .success(item):
            let state = CockpitPlan.status(item.status)?.title ?? item.status
            return .landed(planNumber: item.number,
                receipt: "已往 计划 #\(item.number)「\(item.title)」追加一条进展（现在是「\(state)」）")
        }
    }

    private func judgeCompletionEvidence(args: [String: Any]) -> TodoEvidence.Verdict {
        let commit = args["evidence_commit"] as? String
        let prose = args["evidence"] as? String
        let workdir = LocalCrewStore.workingDirectory(
            crewId: crewId, whiteboardDirectory: store.resolvedDirectory)
        return TodoEvidence.judge(commit: commit, prose: prose) { sha in
            guard let workdir else {
                return .unavailable("这台机器上查不到本 crew 登记的工作目录，所以没有一个「确定是那个」的仓库可查")
            }
            return GitObjectProbe(directory: workdir).resolve(sha)
        }
    }

    /// 把「撤回 人类 To Do #N：原因」那一行发进群，返回走到了哪一步。
    ///
    /// 撤回的两个入口（`withdraw_human_todo` 和 `add_human_todo(supersedes:)`）共用
    /// 这一份 —— 群里那行的措辞和失败处理只有一处，不会各写一套慢慢分叉。
    private func announceWithdrawal(number: Int, reason: String)
        -> (reached: TodoLandingFlow.Step, detail: String?) {
        do {
            let incident = try store.appendSessionMessageReportingFailure(
                crewId: crewId, sessionId: sessionId,
                text: TodoLedger.human.withdrawAnnouncement(number: number, reason: reason),
                category: "progress",
                senderName: sessionLabel,
                mentions: TodoLandingFlow.mentions(.withdrawn).map(LocalWhiteboardMention.init),
                inReplyTo: nil,
                senderKind: isCaptain ? "captain" : "session")
            if let incident { return (.persisted, incident) }
            return (TodoLandingFlow.terminal(.withdrawn), nil)
        } catch {
            return (.persisted, Self.writeFailureReceipt(error))
        }
    }

    /// `add_human_todo(supersedes:)` 的后半段：新的已经落好了，把旧的撤掉。
    ///
    /// 目标在**落新条目之前**就验过了（见 `add_human_todo` 里那段），所以这里理论上
    /// 不会撞上「不是你提的 / 没这条」。**但仍然把每种结果都说出来** —— 两次检查
    /// 之间隔着一次落盘，中间真被人删了/别人撤了就是会发生；那时候账上是「新的加了、
    /// 旧的还挂着」，人会同时看到两条问同一件事。**这种时候必须让 agent 知道，
    /// 它才有机会自己去补一刀。**
    private func supersedeOldOne(target: Int, replacedBy: Int) -> String {
        let reason = "被 #\(replacedBy) 取代"
        switch humanTodos.withdraw(crewId: crewId, number: target, sessionId: sessionId,
                                   senderName: sessionLabel, reason: reason,
                                   isCaptain: isCaptain) {
        case .withdrawn:
            let announced = announceWithdrawal(number: target, reason: reason)
            if let detail = announced.detail {
                return "旧的 #\(target) 已撤回，但**群里那行没发出去**：\(detail)。"
                    + "账是对的，群里没人看得见 —— 需要的话自己去补一句。"
            }
            return "同时撤回了旧的 #\(target)（原因：\(reason)）—— 人现在只会看到 #\(replacedBy) 在等他。"
        case .notFound, .notYours, .alreadyWithdrawn, .reasonRequired,
             .ledgerUnavailable, .notWritten:
            return "⚠️ **但旧的 #\(target) 没撤成**"
                + WriteReceipt.notWrittenMarker
                + "（刚才验的时候还好好的，这一下之间它被删/被撤/账读不出来或落不了盘了）。"
                + "现在 #\(target) 和 #\(replacedBy) **两条都挂在人的账上问同一件事** —— "
                + "请单独调 withdraw_human_todo 把 #\(target) 撤掉。"
        }
    }

    private static func writeFailureReceipt(_ error: Error) -> String {
        WriteReceipt.notWritten(
            what: "这条群消息", error: error,
            consequence: "请当作未送达处理（别把它当已说过的话）。")
    }

    // MARK: - JSON-RPC envelope helpers

    /// 作战板的一行行文本（工具回执 / plan_list 共用）。**带上「多久没更新」** ——
    /// 机长自己读这块板时也该被那面照妖镜照到，不能只在 UI 上显示。
    /// `plan_add` / `plan_update` 上那个督办参数的说明（两处一字不差，所以只写一份）。
    ///
    /// 措辞刻意把**解除条件**写死在参数说明里：这是整套机制唯一的出口，写在别处
    /// 都可能被略过。
    /// `confirm_todo_sweep` 的说明。**「为什么不能只说一句都做完了」必须留在这儿**——
    /// 机长读到的只有这段文字，把理由删成「请逐条归桶」它就只剩一条没来由的形式要求。
    static let confirmSweepDescription = """
    （机长专用）**回应「你停下来了，但账上还有 N 条没完成」那条提醒**——把每一条未完成的 Todo（人类派给你那本）归进一个桶，提醒才会停。

    **为什么不能只说一句「都做完了」**：有过一次现场——机长一整天以为自己在推进，把账拉出来才发现 23 条未完成里 **9 条是「做完了没翻牌」**，其中两条早在两个版本前就发出去了。它自己的说法是「如果确认只需要说一句都做完了，我会毫不犹豫地说出口」。所以这里要求逐条归桶：**归不进任何桶的那一条，多半就是你早就做完了却没翻牌的那条**——去 respond_todo 把它翻掉。

    三个桶的并集必须**恰好等于**当前未完成的那批，一条不多一条不少，否则整笔拒绝并告诉你差哪几条。确认之后，只要不冒出没被覆盖过的新条目，就不会再提醒你（**与过去多久无关**）。

    这里没有「我知道了 / 顺延」这种动作——那种出口会让整件事退化成看一眼就算办完。
    """

    /// MCP 过来的整数数组可能是 `[Int]`，也可能是 `[Any]`（JSON 解出来的 NSNumber）。
    /// **两种都要收** —— 只认前者的话，机长报的号会被静默当成空数组，
    /// 于是一份认真填的确认被回「你一条都没归桶」。
    static func intArray(_ raw: Any?) -> [Int] {
        if let ints = raw as? [Int] { return ints }
        if let anys = raw as? [Any] {
            return anys.compactMap { ($0 as? Int) ?? ($0 as? NSNumber)?.intValue }
        }
        return []
    }

    static let superviseParamDescription = "督办：过这么多分钟（\(Int(SupervisionLease.minMinutes))–\(Int(SupervisionLease.maxMinutes))）这条还没有结果，就把**你**叫醒去过问（只叫你一个，不进群、不打扰别人）。把活交出去/交接时挂上。\n**解除只有一条路：把这条翻到 done 或 blocked。** 没有「我知道了 / 已查看 / 顺延」这种动作 —— 看一眼不算有结果，所以想让它停就得把板更到有结果。仍无结果时它按 1×→2×→4×→8× 退避再叫你。已经挂着督办的计划不能重挂。"

    /// 把一笔督办挂上（写进控制通道，app 侧登记 + 起定时器），返回给机长看的那一行。
    ///
    /// **不写白板** —— 白板是给人看的，不是闹钟；挂上的那一刻群里不该多出任何东西。
    private func attachSupervisionLease(planNumber: Int, minutes: Double) -> String {
        let fireAt = Date().addingTimeInterval(minutes * 60)
        control.enqueueSupervisionLease(
            crewId: crewId, sessionId: sessionId, planNumber: planNumber,
            fireAt: ISO8601DateFormatter().string(from: fireAt),
            baseSeconds: minutes * 60)
        return "已挂督办：\(Int(minutes)) 分钟后（\(Self.iso.string(from: fireAt))）这条若仍没有结果，会把你叫醒。解除只有把 #\(planNumber) 翻到 done 或 blocked 一条路。"
    }

    private func planRows() -> String {
        let now = Date()
        let rows = CockpitPlan.newestFirst(plans.list(crewId: crewId)).map { item -> String in
            var line = "#\(item.number) [" 
                + CockpitPlan.statusLine(statusRaw: item.status,
                                         updated: Self.iso.date(from: item.updatedAt), now: now)
                + "] \(item.title)"
            if let b = item.blockedBy {
                line += "\n    " + CockpitPlan.blockerLine(b, state: blockerState(b))
            }
            if let last = item.updates.last {
                line += "\n    最近进度：\(last.text)"
            }
            return line
        }
        return rows.isEmpty ? "任务列表是空的 —— 用 plan_add 排第一条。" : rows.joined(separator: "\n")
    }

    /// 「卡住」指的那条 Todo 还在不在。**本层如实回答，不猜**：
    /// - `.agent` 那本（人类派给 agent）现在就查得了；
    /// - `.human` 那本是 Todo #62 的产物，**还没合进 main** —— 那就明说没核实，
    ///   不假装查过。#62 落地后这里换成对那本 store 的一次 `item(...)` 即可。
    private func blockerState(_ blocker: CockpitPlanBlocker) -> CockpitPlanBlockerState {
        CockpitPlan.blockerState(
            blocker,
            // 两行必须同一个来源：都用**注入的** store，不许写 `LocalTodoStore.shared(_:)`。
            // 那个共享实例吃的是 `LocalWhiteboardStore.defaultDirectory`，而 helper 靠
            // `--dir` 定目录；生产上两者恰好相等所以看不出来，**单测里会当场读错人** ——
            // 用例摆在 temp 目录里的那本压根不会被读到，读的是开发机上真实的账，
            // 而 CI 干净机器上恒 `.missing`，于是一条从没读过 temp 目录的用例会以
            // 稳定的绿一直活着。两行保持视觉对称，谁也别想只改一行。
            agentTodoExists: { todos.item(crewId: crewId, number: $0) != nil },
            // `self.` 是必须的、不是风格：`humanTodoExists` 是**可选**闭包参数，
            // 而可选函数类型没法 non-escaping，所以编译器要求显式捕获语义。
            humanTodoExists: { self.humanTodos.item(crewId: self.crewId, number: $0) != nil })
    }

    private static let iso = ISO8601DateFormatter()

    private func result(id: Any?, _ result: [String: Any]) -> String? {
        envelope(["jsonrpc": "2.0", "id": id ?? NSNull(), "result": result])
    }

    private func toolResult(id: Any?, text: String) -> String? {
        result(id: id, ["content": [["type": "text", "text": text]]])
    }

    private func error(id: Any?, code: Int, message: String) -> String? {
        envelope(["jsonrpc": "2.0", "id": id ?? NSNull(), "error": ["code": code, "message": message]])
    }

    private func envelope(_ dict: [String: Any]) -> String? {
        guard let data = try? JSONSerialization.data(withJSONObject: dict),
              let s = String(data: data, encoding: .utf8) else { return nil }
        return s
    }
}
