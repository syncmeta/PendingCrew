import XCTest

/// 通讯录读不出来时，`contact` / `directory` 不许把 IO 失败翻译成业务结论。
///
/// **出处是一次真实断线**（2026-09-05 11:02–11:11，三个 crew 同时中招）：那 9 分钟里
/// `~/Library/Application Support/PendingCrew/` 下的文件内容读不了（EPERM，写和 stat
/// 照常）。同一次断线里两个工具的表现差得很远：
///
/// - `post_to_crew` 回执：「**这条消息没有发出去，请当作未送达处理**」+ 原始系统报错
///   → 人会去查通道。**对的。**
/// - `contact` 回执：「**查无此号 1-1**」
///   → 人会去查号码。**而号码是对的。** 一条错误指引能烧掉一整轮排查。
///
/// 病根是 `CrewDirectory.load` 里 `try? Data(contentsOf:)` 把读失败吞成空数组：
/// **「读不到」和「不存在」在那一行之后就再也分不开了。**
///
/// 这里量的是**工具吐出来的话**，不是内部返回值 —— 因为受害者看到的就是那句话。
@MainActor
final class McpDirectoryIOFailureTests: XCTestCase {

    private struct Fixture {
        let base: URL
        let whiteboards: URL
        let crewFile: URL
        let sourceCrewId: String
    }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpdirio-\(UUID().uuidString)")
        let whiteboards = base.appendingPathComponent("whiteboards", isDirectory: true)
        try? FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)
        let store = LocalCrewStore(baseDirectory: base)
        let source = store.createCrew(.make(
            responsibleSubjectId: "s", title: "机组群聊体验", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        _ = store.createCrew(.make(
            responsibleSubjectId: "s", title: "常驻后台", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil)))
        store.recordSessionMember(crewId: source, sessionId: "sess-1", displayName: "机长")
        return Fixture(base: base, whiteboards: whiteboards,
                       crewFile: base.appendingPathComponent("local-crews.json"),
                       sourceCrewId: source)
    }

    private func server(_ f: Fixture) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.sourceCrewId, sessionId: "sess-1",
                  isCaptain: true, sessionLabel: "机长",
                  quotaDirectory: f.whiteboards)
    }

    private func call(_ s: McpServer, _ name: String, _ arguments: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call",\
        "params":{"name":"\(name)","arguments":\(arguments)}}
        """) ?? ""
    }

    /// 把通讯录变成「读不出来」，并**当场证明它真的读不出来**。
    ///
    /// 不验这一步的话，权限没生效时测试会以「没红」的方式绿掉 —— 那种绿跟真绿
    /// 长得一模一样。（同一天栽过：唯一会红的样本被扫描器自己跳过，全绿看着像已核。）
    /// 断线那一刻系统给出的原文。**断言要拿它去比**，别去比某个语言里的词——
    /// 本机 locale 是繁体，`Data(contentsOf:)` 回的是「沒有權限檢視」，
    /// 而原断言找的是简体「权限」，于是一条正确的实现会被判红。
    private var systemReason: String = ""

    private func makeUnreadable(_ f: Fixture) throws {
        try FileManager.default.setAttributes([.posixPermissions: 0],
                                              ofItemAtPath: f.crewFile.path)
        addTeardownBlock {
            try? FileManager.default.setAttributes([.posixPermissions: 0o644],
                                                   ofItemAtPath: f.crewFile.path)
        }
        XCTAssertThrowsError(try Data(contentsOf: f.crewFile),
                             "前置条件没成立：文件仍然读得出来，后面的断言不算数") { error in
            self.systemReason = (error as NSError).localizedDescription
        }
    }

    // MARK: - contact

    func test_通讯录读不出来时_contact不许说查无此号() throws {
        let f = fixture()
        try makeUnreadable(f)
        let r = call(server(f), "contact", #"{"to":"2-1","message":"喂"}"#)
        // ⚠️ 这条禁的是**字面串**，不是那个论断 —— 一句本来有用的
        // 「这不是『查无此号』」也会被它判红。**撞上时先想「该改文案还是该改这条断言」**，
        // 别默认是文案的错。（本次实现确实改了文案：话说全了又绕开这四个字。）
        XCTAssertFalse(r.contains("查无此号"),
                       "把「我读不到」说成「它不存在」——人会去查号码，而号码是对的：\(r)")
        XCTAssertTrue(r.contains("通讯录读不出来"), r)
    }

    /// 光说「读不出来」还不够：**得带上真实原因**，否则人只知道失败、不知道往哪查。
    /// 这条对着的是 `post_to_crew` 已经做对的那个口径（它会把系统报错原文附上）。
    func test_contact的IO失败必须带上真实原因() throws {
        let f = fixture()
        try makeUnreadable(f)
        let r = call(server(f), "contact", #"{"to":"2-1","message":"喂"}"#)
        XCTAssertTrue(r.contains("local-crews.json"), "没说是哪份文件读不出来：\(r)")
        let core = systemReason.trimmingCharacters(in: CharacterSet(charactersIn: "。."))
        XCTAssertFalse(core.isEmpty, "没取到系统报错原文，这条断言就没有判据")
        XCTAssertTrue(r.contains(core),
                      "没带上系统给的原因（应含：\(core)），人无从判断是权限还是别的：\(r)")
    }

    // MARK: - directory

    /// 同一个 `load()` 也喂着 `directory` —— 断线时整张表会显示成
    /// 「通讯录是空的（本机还没有登记在案的 crew）」。**同一个病的第二个出口**，
    /// 受害者那次回执只暴露了第一个。
    func test_通讯录读不出来时_directory不许显示成空表() throws {
        let f = fixture()
        try makeUnreadable(f)
        let r = call(server(f), "directory", "{}")
        XCTAssertFalse(r.contains("通讯录是空的"),
                       "把「我读不到」说成「本机没有 crew」：\(r)")
        XCTAssertTrue(r.contains("通讯录读不出来"), r)
    }

    // MARK: - 反面：这两种情况的措辞不许被一起改掉

    /// 号码是真的没发过 —— 这时「查无此号」是**对的答案**，不许因为上面几条被改掉。
    func test_号码真的没发过时仍然说查无此号() {
        let f = fixture()
        let r = call(server(f), "contact", #"{"to":"99-1","message":"喂"}"#)
        XCTAssertTrue(r.contains("查无此号"), r)
        XCTAssertFalse(r.contains("读不出来"), r)
    }

    /// 文件压根不存在 = 全新机器、还没建过 crew，那是**真的空**，不是 IO 失败。
    /// 这条把「缺席」和「读不动」分开 —— 只有后者才该改口。
    func test_文件不存在算真空不算IO失败() throws {
        let f = fixture()
        try FileManager.default.removeItem(at: f.crewFile)
        let r = call(server(f), "directory", "{}")
        XCTAssertTrue(r.contains("通讯录是空的"), r)
        XCTAssertFalse(r.contains("读不出来"), r)
    }
}
