import XCTest

/// **「回执说成功了，而那件事其实没发生」** —— 这一族缺陷的尺子。
///
/// ## 它盯的是什么
///
/// 一个写盘工具的回执，描述的必须是**写成功了**，而不是「我调用了写入函数」。
/// 两者平时长得一样，只在故障当口分岔 —— 而故障当口没人在看回执，
/// 于是这一整族 bug 的特征是：**成功和失败在调用方那儿长得一模一样**，
/// 所以没有人会回来查。
///
/// 三个已经发生过的现场：
/// * `report_to_parent` 在数据目录读写失败的窗口里报「已提交向上汇报」，
///   而那 52 分钟里父 crew 白板上**一条都没有**。
/// * `respond_todo` 指着人类那本账的 #N 调用，写的是 agent 那本，回执照说「已回应」。
/// * `post_to_crew` 的 `reply_to` 在旧 helper 上不 @ 任何人，回执照回「已发到」。
///
/// ## 尺子的形状（两半，缺一不可）
///
/// **① 全集闸**：工具名单不是手写的，是从 `tools/list` 这个**真注册表**里拿的。
/// 新加一个工具而没在下面 `classification` 里表态，这条就红 —— 这才是
/// 「下一个人加工具时谁来拦住他」的那道门。手写名单挡不住新工具。
///
/// **② 诚实闸**：每个会写盘的工具，在**写盘真的失败**时回执必须带
/// `WriteReceipt.notWrittenMarker`；在**写盘正常**时必须不带。
/// 只验前一半的尺子可以被「到处硬写那个记号」满足，那种尺子永远红、
/// 跟永远绿一样没用。
///
/// ## 写失败是怎么造出来的
///
/// **不是等它自己发生** —— 这类 bug 非故障时刻量一切正常。这里把数据根和白板目录
/// 都 `chmod 0o500`：文件读得到（原子写要在同目录建临时文件 + rename，两样都被拒），
/// 于是每一条 `data.write(options: .atomic)` 都真的抛错。造的是**那个文件**：
/// 路径全部经由生产代码自己的常量推导（`TodoLedger.fileSuffix` 等），
/// 一个都不在这里手拼 —— 造错了对象的结果是「不红」，而不红跟「已经修好了」长得一样。
@MainActor
final class McpWriteFailureHonestyTests: XCTestCase {

    // MARK: - 每个工具怎么被归类

    private enum Kind {
        /// 会写白板 / 写账本 / 写任何跨进程共享文件。必须过诚实闸。
        case writesSharedFile(args: String, captainOnly: Bool)
        /// 只读，或只碰本进程内存。写失败与它无关。
        case readOnly(why: String)
        /// 会写，但这把尺子**造不出**它的写失败（见 `why`）。
        /// 列在这里是为了让它出现在名单上 —— 一个没被覆盖的工具必须看得见，
        /// 而不是从名单里消失。
        case uncovered(why: String)
    }

    /// **全集**。键必须与 `tools/list` 的真注册表逐字对齐（下面 `test_全集闸` 钉住）。
    private static let classification: [String: Kind] = [

        // ---- 写共享文件的 ----
        "post_to_crew": .writesSharedFile(
            args: #"{"message":"一条进展"}"#, captainOnly: false),
        "contact": .writesSharedFile(
            args: #"{"to":"2","message":"外线来电"}"#, captainOnly: false),
        "ask": .writesSharedFile(
            args: #"{"question":"A 还是 B？"}"#, captainOnly: false),
        "continue_work": .writesSharedFile(
            args: #"{"note":"下一轮先跑全量"}"#, captainOnly: false),
        "respond_todo": .writesSharedFile(
            args: #"{"number":1,"response":"收到"}"#, captainOnly: false),
        "add_human_todo": .writesSharedFile(
            args: #"{"text":"请拍板 A/B"}"#, captainOnly: false),
        "withdraw_human_todo": .writesSharedFile(
            args: #"{"number":1,"reason":"方案变了"}"#, captainOnly: false),
        "schedule_wakeup": .writesSharedFile(
            args: #"{"after_minutes":30,"note":"回来接着跑"}"#, captainOnly: false),
        "listen": .writesSharedFile(
            args: #"{"minutes":30}"#, captainOnly: false),
        "set_session_profile": .writesSharedFile(
            args: #"{"effort":"high"}"#, captainOnly: false),
        "rename_crew": .writesSharedFile(
            args: #"{"name":"新名字"}"#, captainOnly: true),
        "raise_attention": .writesSharedFile(
            args: #"{"reason":"要人看一眼"}"#, captainOnly: true),
        "clear_attention": .writesSharedFile(
            args: #"{}"#, captainOnly: true),
        "confirm_todo_sweep": .writesSharedFile(
            args: #"{"running":[1],"blocked_on_human":[],"queued":[]}"#, captainOnly: true),
        "plan_add": .writesSharedFile(
            args: #"{"title":"排一条计划"}"#, captainOnly: true),
        "plan_update": .writesSharedFile(
            args: #"{"number":1,"progress":"推进了一点"}"#, captainOnly: true),
        "arrange_crews": .writesSharedFile(
            args: #"{"crew_ids":["c-other"],"reason":"这个先看"}"#, captainOnly: true),
        "start_session": .writesSharedFile(
            args: #"{"brief":"去查一件事","isolation":false}"#, captainOnly: true),
        "handoff_captain_to_session": .writesSharedFile(
            args: #"{"session_id":"sess-2"}"#, captainOnly: true),
        "create_and_handoff_captain": .writesSharedFile(
            args: #"{"runner":"claude"}"#, captainOnly: true),
        "report_to_parent": .writesSharedFile(
            args: #"{"message":"向上汇报一句"}"#, captainOnly: true),
        "message_child_crew": .writesSharedFile(
            args: #"{"crew":"某子群","message":"给你一句"}"#, captainOnly: true),
        "adopt_crew": .writesSharedFile(
            args: #"{"crew":"某平级"}"#, captainOnly: true),
        "release_crew": .writesSharedFile(
            args: #"{"crew":"某直系子"}"#, captainOnly: true),
        "create_parent_crew": .writesSharedFile(
            args: #"{"title":"总机组"}"#, captainOnly: true),
        "adopt_parent": .writesSharedFile(
            args: #"{"crew":"某个爹"}"#, captainOnly: true),
        "create_child_crew": .writesSharedFile(
            args: #"{"brief":"去做一件事"}"#, captainOnly: true),

        // ---- 会写、但这把尺子造不出它的写失败 ----
        // 这四条 enqueue 完要 long-poll 等 app 侧应答（app 不在跑 → 每次固定等满
        // 超时预算）。写失败的诚实性由它们共用的 `control.enqueue*` 保证，
        // 与上面 report_to_parent 那批是同一条代码路径、同一份回执；
        // 但**「回执如实」这半在它们身上没有被这把尺子直接量过**。
        "inspect_session": .uncovered(why: "enqueue 后 long-poll 等 app 应答，尺子里 app 不在跑"),
        "nudge_session": .uncovered(why: "同 inspect_session"),
        "stop_session": .uncovered(why: "同 inspect_session"),
        "change_workdir": .uncovered(why: "同 inspect_session，且超时预算是 12 倍"),

        // ---- 只读 ----
        "directory": .readOnly(why: "读 local-crews.json + crew-sessions.json"),
        "read_whiteboard": .readOnly(why: "读白板"),
        "search_whiteboard": .readOnly(why: "读白板"),
        "get_quota": .readOnly(why: "读 quota.json"),
        "plan_list": .readOnly(why: "读驾驶舱那本账"),
        "list_sessions": .readOnly(why: "读点名快照 + 会话号账本 + 取证面"),
        "crew_ordering_signals": .readOnly(why: "读组织树 + 白板 + viewed 镜像"),
    ]

    // MARK: - ① 全集闸

    /// 名单必须**恰好等于** `tools/list` 的真注册表。
    ///
    /// 两个方向都要断：多了（工具删了名单没删）跟少了（工具加了名单没加）
    /// 一样是账不对。今天全组在「报了 N 份、名单只有 N−1 个」这个形状上栽过四次，
    /// 靠的就是只断了一个方向。
    func test_全集闸_名单与真注册表逐字对齐() throws {
        let fx = try fixture()
        let registered = registeredToolNames(fx)
        let classified = Set(Self.classification.keys)

        XCTAssertEqual(
            registered.subtracting(classified), [],
            "有工具没在 McpWriteFailureHonestyTests.classification 里表态。"
                + "新加一个工具就要回答一句：它写不写共享文件？写失败时回执说什么？")
        XCTAssertEqual(
            classified.subtracting(registered), [],
            "名单里有 tools/list 已经没有的工具 —— 名单没跟着删")
        XCTAssertEqual(registered.count, classified.count)
    }

    /// 条数自己再数一遍：机长面 + worker 面合起来就是全集，且没有重复。
    func test_全集闸_条数对得上() throws {
        let fx = try fixture()
        XCTAssertEqual(registeredToolNames(fx).count, Self.classification.count)
    }

    // MARK: - ② 诚实闸

    /// **写盘真的失败时，回执必须带「没写进去」的记号。**
    ///
    /// 这是主断言。红的样子就是这一族 bug 本身：某个工具在写不进去的时候
    /// 照样回一句好看的成功。
    func test_诚实闸_写失败的回执必须说没写进去() throws {
        var liars: [String] = []
        for (name, kind) in Self.classification.sorted(by: { $0.key < $1.key }) {
            guard case let .writesSharedFile(args, captainOnly) = kind else { continue }
            let fx = try fixture()
            try seed(fx)
            makeWritesFail(fx)
            defer { restoreWrites(fx) }
            let receipt = call(fx, tool: name, args: args, captain: captainOnly)
            if !receipt.contains(WriteReceipt.notWrittenMarker) {
                liars.append("· \(name) → \(oneLine(receipt))")
            }
        }
        XCTAssertTrue(liars.isEmpty, """
            这些工具在**写盘失败**时仍然回了一条读起来像成功的回执 —— \
            调用方（一个 agent）分不出它到底写没写进去：
            \(liars.joined(separator: "\n"))
            口径：宁可回一条难看的失败，也不要回一条好看的成功。\
            失败回执要带 WriteReceipt.notWrittenMarker（\(WriteReceipt.notWrittenMarker)）。
            """)
    }

    /// **写盘正常时不许带那个记号。**
    ///
    /// 没有这一半，上面那条可以被「在所有回执里都硬写一个记号」满足 ——
    /// 一把永远红的尺子和一把永远绿的尺子一样没用，而且更快被关掉。
    func test_诚实闸_写成功时不许带那个记号() throws {
        var falseAlarms: [String] = []
        for (name, kind) in Self.classification.sorted(by: { $0.key < $1.key }) {
            guard case let .writesSharedFile(args, captainOnly) = kind else { continue }
            let fx = try fixture()
            try seed(fx)
            let receipt = call(fx, tool: name, args: args, captain: captainOnly)
            if receipt.contains(WriteReceipt.notWrittenMarker) {
                falseAlarms.append("· \(name) → \(oneLine(receipt))")
            }
        }
        XCTAssertTrue(falseAlarms.isEmpty, """
            这些工具在**写盘正常**时也报了「没写进去」——尺子分不出成功和失败，
            那它就既证明不了成功、也证明不了失败：
            \(falseAlarms.joined(separator: "\n"))
            """)
    }

    /// 造红样本自证：**尺子本身认得出一个说谎的回执。**
    ///
    /// 一个从来没红过的检查，跟一个触发不了的检查长得一模一样。这里现造一个
    /// 「写失败却回成功」的回执喂给同一个判据，确认它会被抓住。
    func test_尺子自证_一条假成功的回执会被抓住() {
        let lying = "已提交向上汇报。送达结果会回执到本 crew 群聊。"
        XCTAssertFalse(lying.contains(WriteReceipt.notWrittenMarker),
                       "这正是尺子该抓的形状：写没成，回执却读起来像成了")
        let honest = WriteReceipt.notWritten(
            what: "向上汇报", consequence: "上级那边什么都没有。")
        XCTAssertTrue(honest.contains(WriteReceipt.notWrittenMarker))
    }

    // MARK: - 造真实写失败

    /// 线上布局：`<base>/local-crews.json` + `<base>/whiteboards/`（helper 的 `--dir`）。
    private struct Fixture {
        let base: URL
        let whiteboards: URL
        let crewId: String
        let otherCrewId: String
    }

    private func fixture() throws -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-write-honesty-\(UUID().uuidString)")
        let whiteboards = base.appendingPathComponent("whiteboards", isDirectory: true)
        try FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)
        let crews = LocalCrewStore(baseDirectory: base)
        let mine = crews.createCrew(.make(
            responsibleSubjectId: "s", title: "本群", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        let other = crews.createCrew(.make(
            responsibleSubjectId: "s", title: "另一个群", machineId: nil,
            workingDirectory: "/tmp/y", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        addTeardownBlock {
            Self.chmod(base, 0o755)
            Self.chmod(whiteboards, 0o755)
            try? FileManager.default.removeItem(at: base)
        }
        return Fixture(base: base, whiteboards: whiteboards, crewId: mine, otherCrewId: other)
    }

    /// 先把每本账都写出一条真行 —— 否则读到空表时是「拒空写」那条路兜住的，
    /// 量到的就不是**写失败**这一条。行落下去，文件也就存在了，
    /// 后面 flock 的 sidecar 才打得开（只读目录里开不了新文件）。
    private func seed(_ fx: Fixture) throws {
        let s = server(fx, captain: true)
        _ = s.todos.add(crewId: fx.crewId, text: "agent 那本的一条")
        _ = s.humanTodos.add(crewId: fx.crewId, text: "人类那本的一条",
                             bySessionId: "sess-1", bySenderName: "我")
        _ = s.plans.add(crewId: fx.crewId, title: "板上的一条",
                        bySessionId: "sess-1", byName: "我")
        _ = s.approvals.raise(crewId: fx.crewId, kind: "decision",
                              sessionId: "sess-1", summary: "一条待决策")
        s.store.appendSessionMessage(crewId: fx.crewId, sessionId: "sess-1", text: "开张")
        s.store.appendSessionMessage(crewId: fx.otherCrewId, sessionId: "sess-1", text: "开张")
        // flock sidecar 与命令队列文件都要先存在/目录先建好，别让「新建文件失败」
        // 混进「写失败」里当替身。
        s.continuations.arm(crewId: fx.crewId, sessionId: "seed-session", note: "占位")
        _ = s.sweeps.row(crewId: fx.crewId)
    }

    /// 数据根与白板目录都收成只读。原子写要在同目录建临时文件再 rename，两样都被拒，
    /// 于是每一条落盘都真的抛错；已存在文件的**读**不受影响（0o500 保留了 r-x）。
    private func makeWritesFail(_ fx: Fixture) {
        Self.chmod(fx.whiteboards, 0o500)
        Self.chmod(fx.base, 0o500)
    }

    private func restoreWrites(_ fx: Fixture) {
        Self.chmod(fx.base, 0o755)
        Self.chmod(fx.whiteboards, 0o755)
    }

    private static func chmod(_ url: URL, _ mode: Int) {
        try? FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: mode)], ofItemAtPath: url.path)
    }

    // MARK: - 调工具

    private func server(_ fx: Fixture, captain: Bool) -> McpServer {
        let s = McpServer(
            store: LocalWhiteboardStore(directory: fx.whiteboards),
            approvals: LocalApprovalStore(directory: fx.whiteboards),
            control: LocalCrewControlStore(directory: fx.whiteboards),
            crewId: fx.crewId, sessionId: "sess-1", isCaptain: captain,
            sessionLabel: "尺子", quotaDirectory: fx.whiteboards,
            todos: LocalTodoStore(directory: fx.whiteboards),
            sweeps: CaptainTodoSweepStore(directory: fx.whiteboards))
        // long-poll 的工具（`ask` 超时那条路）在这把尺子里不该拖住整趟。
        s.commandResponseMaxWaits = 1
        s.commandResponsePollInterval = 0.01
        s.askReplyMaxWaits = 1
        s.askReplyPollInterval = 0.01
        return s
    }

    private func call(_ fx: Fixture, tool: String, args: String, captain: Bool) -> String {
        let s = server(fx, captain: captain)
        var args = args
        if tool == "contact" {
            // 号码是通讯录发的，不是我们拼的。
            guard let dir = try? CrewDirectory.load(whiteboardDirectory: fx.whiteboards),
                  let n = dir.phoneNumber(crewId: fx.otherCrewId, sessionId: "",
                                          isCaptain: true) else {
                return "（拿不到对方号码，contact 没量到）"
            }
            args = #"{"to":"\#(n.text)","message":"外线来电"}"#
        }
        let line = """
        {"jsonrpc":"2.0","id":1,"method":"tools/call",\
        "params":{"name":"\(tool)","arguments":\(args)}}
        """
        return s.handleLine(line) ?? ""
    }

    private func registeredToolNames(_ fx: Fixture) -> Set<String> {
        var names = Set<String>()
        for captain in [false, true] {
            let raw = server(fx, captain: captain)
                .handleLine(#"{"jsonrpc":"2.0","id":9,"method":"tools/list"}"#) ?? ""
            guard let data = raw.data(using: .utf8),
                  let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
                  let result = obj["result"] as? [String: Any],
                  let tools = result["tools"] as? [[String: Any]] else {
                XCTFail("tools/list 解不出来")
                return names
            }
            for t in tools { if let n = t["name"] as? String { names.insert(n) } }
        }
        return names
    }

    private func oneLine(_ s: String) -> String {
        String(s.split(whereSeparator: \.isNewline).joined(separator: " ").prefix(160))
    }
}
