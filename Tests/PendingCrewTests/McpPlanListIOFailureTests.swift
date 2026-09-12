import XCTest

/// 机长任务列表读不出来时，`plan_list` 不许说「任务列表是空的」。
///
/// **出处是一次真实断线**（2026-09-12 00:50 起，机长亲历）：
/// `~/Library/Application Support/PendingCrew/` 下的文件读被拒（EPERM），
/// 写 / stat / 列目录 / 删除 / link 全部照常。那一刻机长调 `plan_list`，
/// 拿到的是：
///
/// > 任务列表是空的 —— 用 plan_add 排第一条。
///
/// 而盘上那本账 **186 KB、92 条**。**「读不出来」被压成了「一条都没有」**，
/// 而后者会直接导致机长把已经排过的活再排一遍。
///
/// 这个病在本仓已经被点名修过两次 —— `CrewSessionRunner` 里那两处都写着
/// 「**必须走 `read`，不能走 `list`**：后者把读失败压成空表」。
/// `plan_list` 是同一条链上**漏掉的第三处**。
///
/// 这里量的是**工具吐出来的那句话**，不是内部返回值 —— 因为受害者看到的就是那句话。
/// 照抄 `McpDirectoryIOFailureTests` 的形状，不发明第二种。
@MainActor
final class McpPlanListIOFailureTests: XCTestCase {

    private struct Fixture {
        let base: URL
        let whiteboards: URL
        let planFile: URL
        let crewId: String
    }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpplanio-\(UUID().uuidString)")
        let whiteboards = base.appendingPathComponent("whiteboards", isDirectory: true)
        try? FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)
        let store = LocalCrewStore(baseDirectory: base)
        let crew = store.createCrew(.make(
            responsibleSubjectId: "s", title: "本组", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        store.recordSessionMember(crewId: crew, sessionId: "sess-1", displayName: "机长")
        return Fixture(base: base, whiteboards: whiteboards,
                       planFile: whiteboards.appendingPathComponent("\(crew).plan.json"),
                       crewId: crew)
    }

    private func server(_ f: Fixture) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.crewId, sessionId: "sess-1",
                  isCaptain: true, sessionLabel: "机长",
                  quotaDirectory: f.whiteboards)
    }

    private func call(_ s: McpServer, _ name: String, _ arguments: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call",\
        "params":{"name":"\(name)","arguments":\(arguments)}}
        """) ?? ""
    }

    /// 断线那一刻系统给出的原文。**断言要拿它去比**，别去比某个语言里的词
    /// （本机 locale 是繁体时 `Data(contentsOf:)` 回的是「沒有權限檢視」）。
    private var systemReason: String = ""

    /// 把那本账变成「读不出来」，并**当场证明它真的读不出来**。
    /// 不验这一步的话，权限没生效时测试会以「没红」的方式绿掉。
    private func makeUnreadable(_ f: Fixture) throws {
        _ = call(server(f), "plan_add", #"{"title":"先排一条，好让文件真的存在"}"#)
        XCTAssertTrue(FileManager.default.fileExists(atPath: f.planFile.path),
                      "前置条件没成立：账本文件都没生成，后面的断言不算数")
        try FileManager.default.setAttributes([.posixPermissions: 0],
                                              ofItemAtPath: f.planFile.path)
        addTeardownBlock {
            try? FileManager.default.setAttributes([.posixPermissions: 0o644],
                                                   ofItemAtPath: f.planFile.path)
        }
        XCTAssertThrowsError(try Data(contentsOf: f.planFile),
                             "前置条件没成立：文件仍然读得出来，后面的断言不算数") { error in
            self.systemReason = (error as NSError).localizedDescription
        }
    }

    // MARK: - 读不出来的时候

    func test_账读不出来时_plan_list不许说任务列表是空的() throws {
        let f = fixture()
        try makeUnreadable(f)
        let r = call(server(f), "plan_list", "{}")
        // ⚠️ 这条禁的是**字面串**。撞上时先想「该改文案还是该改这条断言」，
        // 别默认是文案的错 —— 一句本来有用的话也可能含这四个字。
        XCTAssertFalse(r.contains("任务列表是空的"),
                       "把「我读不到」说成「一条都没有」——机长会把排过的活再排一遍：\(r)")
        XCTAssertTrue(r.contains("读不出来"), "没说这是一次读失败：\(r)")
    }

    /// 光说「读不出来」还不够：**得带上真实原因和是哪份文件**，
    /// 否则人只知道失败、不知道往哪查。对着 `post_to_crew` 已经做对的那个口径。
    func test_plan_list的IO失败必须带上是哪份文件和真实原因() throws {
        let f = fixture()
        try makeUnreadable(f)
        let r = call(server(f), "plan_list", "{}")
        XCTAssertTrue(r.contains(".plan.json"), "没说是哪份文件读不出来：\(r)")
        let core = systemReason.trimmingCharacters(in: CharacterSet(charactersIn: "。."))
        XCTAssertFalse(core.isEmpty, "没取到系统报错原文，这条断言就没有判据")
        XCTAssertTrue(r.contains(core),
                      "没带上系统给的原因（应含：\(core)），人无从判断是权限还是别的：\(r)")
    }

    // MARK: - 真空的时候（**这一半同样要有**）

    /// 只证明「读不出来时会说读不出来」不够 —— 一个**永远**说「读不出来」的实现
    /// 也会让上面两条全绿。这一条钉住它在真空账上仍然说「空的」。
    func test_账真的是空的时候_照旧说空的() {
        let f = fixture()
        let r = call(server(f), "plan_list", "{}")
        XCTAssertTrue(r.contains("任务列表是空的"),
                      "真空账被说成读不出来，人会去查一个根本没坏的文件：\(r)")
        XCTAssertFalse(r.contains("读不出来"), r)
    }

    // MARK: - 同一个病的第四处：`search_whiteboard`

    /// 白板整份读不出来时 `list` 回的是**一行内存警示**，搜索照着它搜必然零命中，
    /// 于是工具说「没有找到匹配消息」—— **把「我读不到」说成「它不在」**，
    /// 而提问的人会据此认定那条消息不存在。判据收在
    /// `LocalWhiteboardStore.readFailure(in:)`，别在调用点各写各的。
    func test_白板读不出来时_搜索不许说没找到() throws {
        let f = fixture()
        let posted = call(server(f), "post_to_crew", #"{"message":"甲乙丙这条要被搜到"}"#)
        let board = f.whiteboards.appendingPathComponent("\(f.crewId).json")
        XCTAssertTrue(FileManager.default.fileExists(atPath: board.path),
                      "post_to_crew 没把白板写出来，回的是：\(posted)")
        try FileManager.default.setAttributes([.posixPermissions: 0],
                                              ofItemAtPath: board.path)
        addTeardownBlock {
            try? FileManager.default.setAttributes([.posixPermissions: 0o644],
                                                   ofItemAtPath: board.path)
        }
        XCTAssertThrowsError(try Data(contentsOf: board),
                             "前置条件没成立：白板仍然读得出来，后面的断言不算数")
        let r = call(server(f), "search_whiteboard", #"{"query":"甲乙丙"}"#)
        XCTAssertFalse(r.contains("没有找到匹配消息"),
                       "把「我读不到」说成「它不在」——问的人会认定那条消息不存在：\(r)")
        XCTAssertTrue(r.contains("读不出来"), "没说这是一次读失败：\(r)")
    }

    /// **边界：损坏重建之后的白板也只剩一行，但那一行是真的、在磁盘上。**
    ///
    /// 两种事故在「只剩一行」这个形状上长得一样，性质却相反：
    /// - 读失败 → `readFailureRowId`，**只在内存里**，磁盘上没有；
    /// - 确认损坏 → 归档原件 + 写一行新警示，**id 是新 UUID、真落盘**。
    ///
    /// 判据只认前者。要是写成「只剩一行就当读不出来」，一个刚被重建的白板会被
    /// 永远说成读不出来，而它其实是好的（只是历史被归档了）。
    func test_损坏重建后那一行不算读失败() {
        let rebuilt = LocalWhiteboardMessage(
            id: UUID().uuidString.lowercased(), senderKind: "session",
            senderUserId: nil, senderSessionId: "system", category: nil,
            text: "内容损坏，已归档为 xxx.corrupt-123（whiteboards 目录），本板从这条警示重新开始。",
            createdAt: ISO8601DateFormatter().string(from: Date()))
        XCTAssertNil(LocalWhiteboardStore.readFailure(in: [rebuilt]),
                     "把「损坏重建」误判成「读不出来」—— 那块白板其实是好的，只是历史被归档了")

        let failure = LocalWhiteboardMessage(
            id: LocalWhiteboardStore.readFailureRowId, senderKind: "session",
            senderUserId: nil, senderSessionId: "system", category: nil,
            text: "白板文件存在但暂时无法读取，原始记录未被改动。",
            createdAt: ISO8601DateFormatter().string(from: Date()))
        XCTAssertNotNil(LocalWhiteboardStore.readFailure(in: [failure]),
                        "读失败那一行没被认出来 —— 那正是这个判据唯一的职责")
    }

    /// 反面：白板读得好好的、只是真没匹配 —— 照旧说「没找到」。
    /// 少了这一条，一个**永远**说「读不出来」的实现也会让上面那条绿。
    func test_白板读得出来但真没匹配时_照旧说没找到() {
        let f = fixture()
        _ = call(server(f), "post_to_crew", #"{"message":"甲乙丙"}"#)
        let r = call(server(f), "search_whiteboard", #"{"query":"戊己庚辛"}"#)
        XCTAssertTrue(r.contains("没有找到匹配消息"),
                      "真没匹配却说成读不出来，人会去查一个根本没坏的文件：\(r)")
        XCTAssertFalse(r.contains("读不出来"), r)
    }

    /// 账有内容时照常列出来 —— 第三条「会绿」的样本。
    func test_账有内容时照常列出() {
        let f = fixture()
        _ = call(server(f), "plan_add", #"{"title":"甲乙丙"}"#)
        let r = call(server(f), "plan_list", "{}")
        XCTAssertTrue(r.contains("甲乙丙"), r)
        XCTAssertFalse(r.contains("读不出来"), r)
        XCTAssertFalse(r.contains("任务列表是空的"), r)
    }
}
