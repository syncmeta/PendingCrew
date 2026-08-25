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
    /// 人类 Todo 列表（task #478）。机器人经 `respond_todo` 追加回应 + 推进状态；
    /// 新增条目只有人类能做（app 面板），MCP 不暴露新增。与 store 同 `--dir`。
    let todos: LocalTodoStore
    /// 机长作战板（人类 Todo #66）。**只有机长写得动** —— `plan_add` / `plan_update`
    /// 前面站着 `guard isCaptain`，worker 连工具列表里都看不到它们。与 store 同 `--dir`。
    let plans: CockpitPlanStore
    /// 本 session 跑在哪家 runner 上（helper `--agent claude|codex`）。
    /// `set_session_profile` 拿它挑对照哪张模型表；nil（旧调用/没传）→ 两家都对照，
    /// 任一家认得就不吭声（宁可少说，也别对着错的表瞎报）。
    let agentKey: String?

    init(store: LocalWhiteboardStore, approvals: LocalApprovalStore, control: LocalCrewControlStore,
         crewId: String, sessionId: String,
         isCaptain: Bool = false, sessionLabel: String? = nil,
         quotaDirectory: URL? = nil, todos: LocalTodoStore? = nil,
         plans: CockpitPlanStore? = nil,
         agentKey: String? = nil) {
        self.store = store
        self.approvals = approvals
        self.control = control
        self.crewId = crewId
        self.sessionId = sessionId
        self.isCaptain = isCaptain
        self.sessionLabel = sessionLabel
        self.quotaDirectory = quotaDirectory ?? LocalWhiteboardStore.defaultDirectory
        self.todos = todos ?? LocalTodoStore()
        self.plans = plans ?? CockpitPlanStore()
        self.agentKey = agentKey
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
                    "description": "把关键节点发到 crew 群聊白板（只发要紧的：开始/完成/卡住/交接/重要发现，别倒 IO 日志）。可带 mentions 定向 @ 某个 session/captain/人类，或 reply_to 回复某条（自动 @ 原发送者）。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "message": ["type": "string"],
                            "category": ["type": "string", "enum": ["progress", "question", "milestone"]],
                            "mentions": [
                                "type": "array",
                                "description": "可选定向 @ 列表 —— 要某个具体对象接手/回应时带上；不填=广播给全 crew。@session / @captain 会**收窄可见范围**：只有被点到的 agent 看得到，并把这条投进它的定向信箱（它优先看到）。@human 不收窄 —— 它只是「这条是讲给人听的、别为它叫醒 agent」的标记，消息对全 crew 照常可见。",
                                "items": [
                                    "type": "object",
                                    "properties": [
                                        "kind": ["type": "string", "enum": ["session", "captain", "human"]],
                                        "target_id": ["type": "string", "description": "kind=session 时必填：目标 session 的 id。"],
                                    ],
                                    "required": ["kind"],
                                ],
                            ],
                            "reply_to": [
                                "type": "string",
                                "description": "可选：你在回复哪条群聊消息的 id —— 给了会自动 @ 那条的原发送者。",
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
                    "description": "读取 crew 群聊白板的当前全部消息（按时间序）。",
                    "inputSchema": ["type": "object", "properties": [String: Any]()],
                ],
                [
                    "name": "ask",
                    "description": "向负责人提问并拿到答复（先 captain，再人类）。任何需要人 / captain 判断、决策、方向选择、澄清、授权的，都走这个 —— 不要在纯文本里问。会阻塞直到有人答。",
                    "inputSchema": [
                        "type": "object",
                        "properties": ["question": ["type": "string"]],
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
                    "description": "回应本 crew 人类 Todo 列表的某个条目（右栏 Todo 面板）。**追加式**：每次调用追加一条回应，不覆盖旧回应；可同时用 status 推进条目状态（待办 pending → 进行中 in_progress → 完成 completed）。人类加条目时群里会出现「To do +1: #N …」——看到后用这个工具认领/回应，number 填那个 N。每个条目都该尽快有机器人回应；status 只在真有进展时才给（开始做→in_progress，做完验证过→completed）。领了 Todo 对应的活，落 main 时顺手翻牌——人类 Todo 面板和 task 账是两本账，别只更 task 漏翻 Todo。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "number": ["type": "integer", "description": "条目编号（群消息「To do +1: #N」里的 N）。"],
                            "response": ["type": "string", "description": "回应内容（认领/进展/结果，一两句说清）。"],
                            "status": ["type": "string", "enum": ["pending", "in_progress", "completed"],
                                       "description": "可选：把条目状态推进到这个值。不填=只回应不动状态。"],
                        ],
                        "required": ["number", "response"],
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
                tools.append([
                    "name": "answer_decision",
                    "description": "（机长专用）答复一条待决策：用你的判断给出答复，answering 会立刻解开发起 session 的等待。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "reqId": ["type": "string"],
                            "reply": ["type": "string"],
                        ],
                        "required": ["reqId", "reply"],
                    ],
                ])
                // 机长作战板（人类 Todo #66）—— 与两本 Todo 的关系：Todo 是**别人给的**
                // （`.agent` 人类派活 / `.human` 请人拍板），这一本是**机长自己排的**。
                // 派活 / 收活 / 翻牌这三个动作发生时顺手更一条，是这块板唯一的活法。
                tools.append([
                    "name": "plan_add",
                    "description": "（机长专用）往**你自己的任务列表**上排一条活。这本账只有你写得动，人类只读——它是你整理出来的作战板，不是 Todo（Todo 是别人给你的）。新条目从「没做」起。派活给 worker、接下一件事、拆出一个阶段时顺手排一条；一条一句话说清做什么，别把整段 brief 塞进来。",
                    "inputSchema": [
                        "type": "object",
                        "properties": [
                            "title": ["type": "string", "description": "一句话说清这条活是什么。"],
                        ],
                        "required": ["title"],
                    ],
                ])
                tools.append([
                    "name": "plan_update",
                    "description": "（机长专用）推进任务列表上的一条：追加进度描述 / 翻进度档 / 改标题 / 撤下，一次可以做完几样。\n**四档**：not_started（没做）· in_progress（进行中）· blocked（卡住）· done（完成）——注意跟 Todo 的三档不是一回事。\n**翻成 blocked 必须指明卡在哪条人类 Todo**（blocked_by_number，默认指 human 那本，也就是你请人类拍板的那本）：「卡住」的意思就是**卡在人身上**，不指出是哪一条，人看到板也不知道该推什么。翻成 blocked 时（且仅此一档）会往群里发一条——其余的进度更新**不进群**，这块板存在的意义就是让进度不必靠刷屏传达。\n**什么时候更**：派活、收活、给 Todo 翻牌，这三个动作发生时顺手更一条。板上每条都记着最后更新时间并显示在界面上（「进行中 · 最后更新 3 天前」），久没碰的条目人一眼就看得见——这是你自己装的照妖镜，别让它照出一板子 6 天前。",
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
                    "description": "（机长专用）点亮侧栏本 crew 头像上的黄色提醒点，提示人类来看。用于：有事需要人类决策、或遇到你自己解决不了的问题。reason 一句话说清为什么需要人类注意——会作为黄点的悬浮提示展示给人类。何时点亮由你自主判断；黄点是打扰人类的信号，别滥用。事情解决或人类已回复不再需要时，记得用 clear_attention 熄灭。",
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
                    "description": "（机长专用）熄灭侧栏本 crew 头像上的黄色提醒点。事情解决、或人类已回复不再需要关注时调用。",
                    "inputSchema": ["type": "object", "properties": [String: Any]()],
                ])
                tools.append([
                    "name": "change_workdir",
                    "description": "（机长专用）改这个 crew 的工作目录，并把 agent 侧的上下文一起迁过去（claude 的会话记录与项目记忆、两家 agent 的「这个目录信任过 / 这些工具允许过」）。仓库搬家、目录改名时用它，别去手改文件。\n\n**不带 confirm 就是预览**：返回要搬什么、影响哪些成员、有什么拦路的，什么都不动。看过没问题再带 confirm:true 调一次才真执行。\n\n几件必须知道的事：\n· 新目录必须**已经存在**，不会替你创建。\n· 有**别的成员正在干活**会拒绝执行并点名；你自己（发起的机长）和空闲的成员都不拦路。\n· **还活着的成员，会话记录不会搬**（它正写着那份日志，搬会搬到半截）——这些成员会列在「留待清扫」里。等它们停了，**用同一个路径再调一次**，就会把剩下的补搬过去。这个工具可以反复调，幂等。\n· 新目录只对**之后新起/重启**的 session 生效；此刻在跑的（包括你自己）还在旧目录里，直到重启。\n· 动手前会把 ~/.claude.json、~/.codex/config.toml、crew 账本各备份一份。",
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
                    "description": "（机长专用）点名：列出本 crew 全部 session 成员的实时状态（干活中/空闲/异常/已退出 + 各自任务）。派活前先点名——有空闲的合适成员就 @ 它接手,别急着 start_session 起新人;有异常的（未登录/额度）先处置或上报。",
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
        case "post_to_crew":
            let message = (args["message"] as? String) ?? ""
            guard !message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: message 不能为空")
            }
            // Phase 7：解析定向 @ + reply_to,记进本地白板（不静默吞）。把它们按
            // mention 投递到 edge 信箱 / 唤醒目标 session 还需 block 3 relay 同步链
            // （见 CrewRelayAgent.push）—— 本地这步只负责把信息留住。
            let mentions = parseMentions(args["mentions"])
            let replyTo = (args["reply_to"] as? String).flatMap { $0.isEmpty ? nil : $0 }
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
                    senderKind: isCaptain ? "captain" : "session")
                return toolResult(id: id, text: Self.postReceipt(incident: incident))
            } catch {
                return toolResult(id: id, text: Self.writeFailureReceipt(error))
            }
        case "directory":
            // 通讯录（2026-08-11）：纯文件层汇总 —— local-crews.json（号码 + 组织边 +
            // 持久成员）× crew-sessions.json（实时状态）。helper 碰不到 app 内存态，
            // 这两份共享文件就是全部数据源。
            let directory = CrewDirectory.load(whiteboardDirectory: sharedDirectory)
            var text = directory.render(query: args["query"] as? String)
            if let mine = directory.phoneNumber(
                crewId: crewId, sessionId: sessionId, isCaptain: isCaptain) {
                text = "你的号码：\(mine.text)\n" + text
            }
            return toolResult(id: id, text: text)
        case "contact":
            return handleContact(id: id, args: args)
        case "read_whiteboard":
            let rows = store.list(crewId: crewId).map(renderRow)
            return toolResult(id: id, text: rows.isEmpty ? "（白板为空）" : rows.joined(separator: "\n"))
        case "ask":
            let question = (args["question"] as? String) ?? ""
            guard !question.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: question 不能为空")
            }
            // 直达人类退化路径（captain-first triage 等 chunk 2）：raise 一条待决策 →
            // 阻塞 long-poll 人类/captain 答复（spec ask-approval §3/§5）。
            guard let reqId = approvals.raise(
                crewId: crewId, kind: "decision", sessionId: sessionId, summary: question) else {
                // 待决策没落盘（读不出来 / 漏读，白板上有系统警示）：再 long-poll
                // 就是对着不存在的条目干等 30 分钟。如实说没提上去（#577）。
                // 措辞不说「已归档」—— 读不出来时原件一字未动、不产生归档（2026-08-12）。
                return toolResult(
                    id: id,
                    text: "ERROR: 这个问题没能记进待决策列表（文件这次读不出来或漏读，"
                        + "原有内容没被动过，群聊白板上有系统警示）。没有人会看到你在问什么 —— "
                        + "改用 post_to_crew 在群里直接问，或稍后重试。")
            }
            // 通知半边（spec §6「PendingCrew 只做通知+列表+答复」）：把问题贴到本地群聊白板，
            // 并 **@ 到能处理的人** —— 决策 captain-first，@captain 让机长优先看到来答；同时
            // @human 兜底（人默认不进 session 详情，全靠群里这条 @ 才会注意到、去待办列表答）。
            // captain 自己发起 ask 时不 @ 自己（去重），只 @human。答复仍走待办列表（approvals）。
            let askMentions: [LocalWhiteboardMention] = isCaptain
                ? [LocalWhiteboardMention(kind: "human", targetId: nil)]
                : [LocalWhiteboardMention(kind: "human", targetId: nil),
                   LocalWhiteboardMention(kind: "captain", targetId: nil)]
            // 通知半边写不进去就别傻等（#577）：白板上没有这条 @，没人知道你在问，
            // long-poll 会一直挂着。如实说明并让调用方自己决定怎么办 —— 待决策已经
            // 进了待办列表，人类仍可在那里答复。
            do {
                let incident = try store.appendSessionMessageReportingFailure(
                    crewId: crewId, sessionId: sessionId,
                    text: "待决策：\(question)\n（去待办列表答复）",
                    category: "question", senderName: sessionLabel,
                    mentions: askMentions,
                    senderKind: isCaptain ? "captain" : "session")
                let reply = awaitReply(reqId: reqId)
                guard let incident else { return toolResult(id: id, text: reply) }
                return toolResult(id: id, text: "⚠️ \(incident)\n\n\(reply)")
            } catch {
                return toolResult(
                    id: id,
                    text: "ERROR: 问题没能贴到 crew 群聊白板 —— \(error.localizedDescription)。"
                        + "没人会在群里看到你在问什么，这次 ask 不再等待。"
                        + "待决策 \(reqId) 已经进了人类的待办列表，可提醒人去那里答复。")
            }
        case "answer_decision":
            // 机长专用（chunk2 T4）：答复一条 decision，解开发起 session 的 long-poll。
            guard isCaptain else {
                return toolResult(id: id, text: "ERROR: 仅机长可用")
            }
            let reqId = (args["reqId"] as? String) ?? ""
            let reply = (args["reply"] as? String) ?? ""
            guard !reply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: reply 不能为空")
            }
            guard let item = approvals.item(crewId: crewId, id: reqId) else {
                return toolResult(id: id, text: "ERROR: 找不到待决策 \(reqId)")
            }
            guard item.kind == "decision" else {
                return toolResult(id: id, text: "ERROR: 该条不是待决策（权限审批请人类处理）")
            }
            guard item.status == "pending" else {
                return toolResult(id: id, text: "ERROR: 该条已被答复")
            }
            approvals.answer(crewId: crewId, id: reqId, reply: reply)
            return toolResult(id: id, text: "已答复，发起的 session 将继续。")
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
            control.requestRename(crewId: crewId, name: name)
            return toolResult(id: id, text: "已把 crew 改名为「\(name)」。")
        case "raise_attention":
            // 机长专用（crew-sidebar-status §3）：写 attention 变更进控制通道；app 侧
            // CrewStore 排空落地到 LocalCrewStore.setAttention → 侧栏头像黄点点亮。
            guard isCaptain else {
                return toolResult(id: id, text: "ERROR: 仅机长可用")
            }
            let reason = ((args["reason"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !reason.isEmpty else {
                return toolResult(id: id, text: "ERROR: reason 不能为空 —— 一句话说明为什么需要人类注意。")
            }
            control.requestAttention(crewId: crewId, reason: reason)
            return toolResult(id: id, text: "已点亮本 crew 的黄色提醒点：\(reason)。事了记得 clear_attention 熄灭。")
        case "clear_attention":
            guard isCaptain else {
                return toolResult(id: id, text: "ERROR: 仅机长可用")
            }
            control.requestClearAttention(crewId: crewId)
            return toolResult(id: id, text: "已熄灭本 crew 的黄色提醒点。")
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
            control.enqueueStartSession(crewId: crewId, brief: brief, runner: runner,
                                        isolation: isolation, model: model, effort: effort, title: title)
            let announceIncident = announceProfileAdvisories(
                notes, headline: "start_session（\(title ?? brief)）的参数对不上模型表")
            var text = "已安排起 session：\(title ?? brief)。它起来后会在群聊报到。"
            if !notes.isEmpty {
                text += "\n⚠️ 参数提醒（已照常起，没拦你；同一份提醒已发白板）：\n"
                    + notes.map { "· \($0)" }.joined(separator: "\n")
            }
            if let announceIncident { text += "\n⚠️ \(announceIncident)" }
            return toolResult(id: id, text: text)
        case "inspect_session":
            // 机长自愈（wake-resilience 层4）：@ 不应时自己看终端现场。命令经
            // 控制通道给 app 执行（helper 碰不到 run/PTY），long-poll 应答文件。
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["session_id"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: session_id 不能为空") }
            let cmdId = control.enqueueInspectSession(crewId: crewId, targetSessionId: target)
            return toolResult(id: id, text: awaitCommandResponse(commandId: cmdId))
        case "nudge_session":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["session_id"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let input = (args["input"] as? String) ?? ""
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: session_id 不能为空") }
            guard !input.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
                return toolResult(id: id, text: "ERROR: input 不能为空（\"Enter\"/\"Esc\"/文本）")
            }
            let cmdId = control.enqueueNudgeSession(crewId: crewId, targetSessionId: target, input: input)
            return toolResult(id: id, text: awaitCommandResponse(commandId: cmdId))
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
            let cmdId = control.enqueueStopSession(
                crewId: crewId, requesterSessionId: sessionId,
                targetSessionId: target, reason: reason)
            return toolResult(id: id, text: awaitCommandResponse(commandId: cmdId))
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
            let cmdId = control.enqueueChangeWorkdir(
                crewId: crewId, sessionId: sessionId, targetHint: targetHint,
                path: newPath, includeChildren: includeChildren, confirm: confirm)
            // 迁移可能要搬上百个文件 + 重试写 ~/.claude.json，默认 10 秒不够 —— 放宽 12 倍
            // （按 `commandResponseMaxWaits` 成比例，单测把基数调小后不会被这条拖慢）。
            return toolResult(id: id, text: awaitCommandResponse(
                commandId: cmdId, maxWaits: commandResponseMaxWaits * 12,
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
            control.enqueueScheduleWakeup(crewId: crewId, sessionId: sessionId, fireAt: iso, note: note)
            return toolResult(id: id, text: "已设定时唤醒：\(iso)。到点会把你的备注注入回本 session；若届时你已退出，会落到群聊白板由机长接手。")
        case "listen":
            // 群聊收听（#465）：写一条 listen 命令进控制通道，app 侧 CrewSessionRunner
            // 登记后把收听期内的广播消息直投注入本 session。到期/off 都是 app 侧语义，
            // 这里只做参数卫生 + 计算截止时刻。
            if (args["off"] as? Bool) == true {
                control.enqueueListen(crewId: crewId, sessionId: sessionId,
                                      until: nil, senders: nil, off: true)
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
            control.enqueueListen(crewId: crewId, sessionId: sessionId, until: until,
                                  senders: (senders?.isEmpty ?? true) ? nil : senders, off: false)
            let who = (senders?.isEmpty ?? true) ? "全部成员" : senders!.joined(separator: "、")
            return toolResult(id: id, text: "已开启群聊收听至 \(until)（听：\(who)）。期间无定向 @ 的新消息和 @ 你的消息会注入唤醒你；到期自动停。现在正常结束你的回合等消息即可，不要空转轮询。")
        case "plan_add":
            // 机长作战板（人类 Todo #66）。门禁与其余机长工具同一道 `guard`。
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let planTitle = ((args["title"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !planTitle.isEmpty else {
                return toolResult(id: id, text: "ERROR: title 不能为空 —— 一句话说清这条活是什么。")
            }
            guard let planned = plans.add(crewId: crewId, title: planTitle,
                                          bySessionId: sessionId, byName: sessionLabel) else {
                // nil ≠「没排」这么轻描淡写：账读不出来时本次写已拒，白板上有一条如实警示。
                return toolResult(id: id, text: "ERROR: 没排进去 —— 任务列表这次读不出来，本次写已拒（群聊白板上有一条系统警示说明是哪种事故）。")
            }
            return toolResult(id: id, text: "已排上 计划 #\(planned.number)：\(planned.title)（没做）。")
        case "plan_update":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let planNumber = (args["number"] as? Int) ?? (args["number"] as? Double).map(Int.init)
            guard let planNumber, planNumber >= 1 else {
                return toolResult(id: id, text: "ERROR: number 需为正整数（plan_list 里的 #N）。")
            }
            if (args["drop"] as? Bool) == true {
                guard plans.drop(crewId: crewId, number: planNumber) else {
                    return toolResult(id: id, text: "ERROR: 撤不下 计划 #\(planNumber)（找不到这条，或任务列表读不出来 —— 后者白板上有警示）。\n" + planRows())
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
            let blockerLedger = ((args["blocked_by_ledger"] as? String) ?? "human")
                .trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
            guard ["human", "agent"].contains(blockerLedger) else {
                return toolResult(id: id, text: "ERROR: blocked_by_ledger 只能是 human（你请人类拍板那本）或 agent（人类派给你那本）。")
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
                return toolResult(id: id, text: "ERROR: status 只能是 pending / in_progress / completed。")
            }
            guard let updated = todos.respond(crewId: crewId, number: number, sessionId: sessionId,
                                              senderName: sessionLabel, text: response,
                                              newStatus: status) else {
                let rows = todos.list(crewId: crewId).map {
                    "#\($0.number) [\(LocalTodoItem.statusLabel($0.status))] \($0.text)"
                }
                return toolResult(id: id, text: "ERROR: 没能回应 Todo #\(number)（找不到这条，"
                                  + "或 Todo 列表文件读不出来 —— 后者群聊白板上会有一条系统警示）。"
                                  + "当前列表：\n"
                                  + (rows.isEmpty ? "（空）" : rows.joined(separator: "\n")))
            }
            return toolResult(id: id, text: "已回应 Todo #\(number)（状态：\(LocalTodoItem.statusLabel(updated.status))）。")
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
            control.enqueueSetProfile(crewId: crewId, sessionId: sessionId, model: model, effort: effort)
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
            return toolResult(id: id, text: snap.renderRoster(crewId: crewId))
        case "report_to_parent":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let msg = ((args["message"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !msg.isEmpty else { return toolResult(id: id, text: "ERROR: message 不能为空") }
            control.enqueueCrewMessage(crewId: crewId, sessionId: sessionId,
                                       direction: "to_parent", targetHint: nil, message: msg)
            return toolResult(id: id, text: "已提交向上汇报。送达结果（含「本 crew 无父」的情况）会回执到本 crew 群聊。")
        case "message_child_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            let msg = ((args["message"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty, !msg.isEmpty else {
                return toolResult(id: id, text: "ERROR: crew 与 message 都不能为空")
            }
            control.enqueueCrewMessage(crewId: crewId, sessionId: sessionId,
                                       direction: "to_child", targetHint: target, message: msg)
            return toolResult(id: id, text: "已提交给子 crew「\(target)」的消息。送达/找不到的回执会出现在本 crew 群聊。")
        case "adopt_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let target = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !target.isEmpty else { return toolResult(id: id, text: "ERROR: crew 不能为空") }
            control.enqueueAdoptCrew(crewId: crewId, sessionId: sessionId, target: target)
            return toolResult(id: id, text: "已提交收编「\(target)」。结果（含解析失败/成环被拒）会回执到群聊。")
        case "release_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let child = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !child.isEmpty else { return toolResult(id: id, text: "ERROR: crew 不能为空") }
            let dest = (args["to"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            control.enqueueReleaseCrew(crewId: crewId, sessionId: sessionId,
                                       child: child, to: (dest?.isEmpty == false) ? dest : nil)
            let destDesc = (dest?.isEmpty == false) ? "转挂到「\(dest!)」" : "摘出到顶层"
            return toolResult(id: id, text: "已提交把直系子「\(child)」\(destDesc)。结果会回执到群聊。")
        case "create_parent_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let rawParentTitle = (args["title"] as? String) ?? ""
            let parentTitle = rawParentTitle.split(whereSeparator: \.isWhitespace).joined(separator: " ")
            control.enqueueCreateParentCrew(crewId: crewId, sessionId: sessionId,
                                            title: parentTitle.isEmpty ? nil : parentTitle)
            return toolResult(id: id, text: "已安排在本 crew 头上新建父 crew。父机长起来后会报到；之后可用 report_to_parent 向它汇报、请它收编其它平级 crew。")
        case "adopt_parent":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let parent = ((args["crew"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !parent.isEmpty else { return toolResult(id: id, text: "ERROR: crew 不能为空") }
            control.enqueueAdoptParent(crewId: crewId, sessionId: sessionId, target: parent)
            return toolResult(id: id, text: "已提交认「\(parent)」为父 crew。结果会回执到群聊。")
        case "create_child_crew":
            guard isCaptain else { return toolResult(id: id, text: "ERROR: 仅机长可用") }
            let brief = ((args["brief"] as? String) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            guard !brief.isEmpty else { return toolResult(id: id, text: "ERROR: brief 不能为空") }
            let rawTitle = (args["title"] as? String) ?? ""
            let title = rawTitle.split(whereSeparator: \.isWhitespace).joined(separator: " ")
            control.enqueueCreateChildCrew(
                crewId: crewId, sessionId: sessionId,
                brief: brief, title: title.isEmpty ? nil : title)
            return toolResult(id: id, text: "已安排建子 crew。开场任务会写入子群并交给子机长执行；送达失败会在本群回执。")
        default:
            return toolResult(id: id, text: "ERROR: 未知工具 \(name ?? "nil")")
        }
    }

    // MARK: - 通讯录 contact（2026-08-11）

    /// 共享文件层目录（helper 的 `--dir`）—— 白板 / quota / 点名快照 / 通讯录
    /// （`local-crews.json` 在其父目录）都在这一份下面。
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
        let directory = CrewDirectory.load(whiteboardDirectory: sharedDirectory)
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
                externalContactFrom: myNumber?.text ?? sourceTitle)
        } catch {
            return toolResult(
                id: id,
                text: "ERROR: 没能写进 \(number.text) 的群聊白板 —— \(error.localizedDescription)。"
                    + "这条消息没有发出去，请当作未送达处理（对方群里什么都没有）。")
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
                senderKind: isCaptain ? "captain" : "session")
        } catch {
            receiptIncident = "本群那行「已联系」回执没写进去（\(error.localizedDescription)）——"
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
    /// —— 比如 `change_workdir` 要搬上百个会话文件、复制记忆、还可能重试写
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

    func awaitReply(reqId: String, pollInterval: TimeInterval = 0.5, maxWaits: Int = 3600) -> String {
        var waits = 0
        while waits < maxWaits {
            if let it = approvals.item(crewId: crewId, id: reqId), it.status == "answered" {
                return it.reply ?? "（已答复，无文本）"
            }
            waits += 1
            Thread.sleep(forTimeInterval: pollInterval)
        }
        return "（暂无人响应 —— 请自行判断后继续）"
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

    private func renderRow(_ m: LocalWhiteboardMessage) -> String {
        // 与 HookEmitter.render 同款：有显示名优先用名，无名退回旧格式。
        let who: String
        if let name = m.senderName ?? m.senderDisplayName, !name.isEmpty {
            who = name
        } else {
            switch m.senderKind {
            case "session": who = "session:\(m.senderSessionId ?? "?")"
            case "user": who = "人类"
            default: who = m.senderKind
            }
        }
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
    private static func postReceipt(incident: String?) -> String {
        guard let incident else { return "已发到 crew 群聊白板。" }
        return "已发到 crew 群聊白板。⚠️ 但请注意：\(incident)"
    }

    /// 白板写失败时的回执。措辞按「当没送达处理」写死 —— 调用方（编码 agent）看到
    /// 这句要知道刚才那段话群里没人看得见。
    private static func writeFailureReceipt(_ error: Error) -> String {
        "ERROR: 没能写进 crew 群聊白板 —— \(error.localizedDescription)。"
            + "这条消息没有发出去，请当作未送达处理（别把它当已说过的话）。"
    }

    // MARK: - JSON-RPC envelope helpers

    /// 作战板的一行行文本（工具回执 / plan_list 共用）。**带上「多久没更新」** ——
    /// 机长自己读这块板时也该被那面照妖镜照到，不能只在 UI 上显示。
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
            agentTodoExists: { todos.item(crewId: crewId, number: $0) != nil },
            // Todo #62 合 main 后，这里换成对 `.human` 那本 store 的一次查询即可。
            // 在那之前**如实说没核实**，不假装查过。
            humanTodoExists: nil)
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
