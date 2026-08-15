import XCTest

/// 通讯录两个全员工具（2026-08-11）的端到端单测：`directory` 查、`contact` 联系。
/// helper 只碰共享文件层，所以这里造 `local-crews.json` + 白板目录就能全量验证。
@MainActor
final class McpServerDirectoryToolsTests: XCTestCase {

    /// 线上布局：`<base>/local-crews.json` + `<base>/whiteboards/`（helper 的 --dir）。
    private struct Fixture {
        let base: URL
        let whiteboards: URL
        let crewStore: LocalCrewStore
        let sourceCrewId: String
        let targetCrewId: String
    }

    private func fixture() -> Fixture {
        let base = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcpdir-\(UUID().uuidString)")
        let whiteboards = base.appendingPathComponent("whiteboards", isDirectory: true)
        try? FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)
        let store = LocalCrewStore(baseDirectory: base)
        let source = store.createCrew(.make(
            responsibleSubjectId: "s", title: "PendingCrew", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        let target = store.createCrew(.make(
            responsibleSubjectId: "s", title: "应用自动更新", machineId: nil,
            workingDirectory: "/tmp/x", captainAgentKind: "claude_code",
            initialTitleSource: .human, captain: .systemGenerated(templateName: nil))).crewId
        store.recordSessionMember(crewId: source, sessionId: "sess-1", displayName: "通讯录")
        store.recordSessionMember(crewId: target, sessionId: "worker-abc", displayName: "Sparkle 接入")
        return Fixture(base: base, whiteboards: whiteboards, crewStore: store,
                       sourceCrewId: source, targetCrewId: target)
    }

    private func server(_ f: Fixture, isCaptain: Bool = false) -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: f.whiteboards),
                  approvals: LocalApprovalStore(directory: f.whiteboards),
                  control: LocalCrewControlStore(directory: f.whiteboards),
                  crewId: f.sourceCrewId, sessionId: "sess-1",
                  isCaptain: isCaptain, sessionLabel: "通讯录",
                  quotaDirectory: f.whiteboards)
    }

    private func call(_ s: McpServer, _ name: String, _ arguments: String) -> String {
        s.handleLine("""
        {"jsonrpc":"2.0","id":1,"method":"tools/call",\
        "params":{"name":"\(name)","arguments":\(arguments)}}
        """) ?? ""
    }

    // MARK: - 注册

    func testBothToolsAreListedForEveryone() {
        let f = fixture()
        // 全员工具 —— worker（非机长）也必须看得到。
        let r = server(f).handleLine(#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)!
        XCTAssertTrue(r.contains("\"directory\""))
        XCTAssertTrue(r.contains("\"contact\""))
    }

    // MARK: - directory

    func testDirectoryListsNumbersAndOwnNumber() {
        let f = fixture()
        let r = call(server(f), "directory", "{}")
        XCTAssertTrue(r.contains("你的号码：1-2"), r)   // sess-1 是 1 号 crew 的第一个 worker
        XCTAssertTrue(r.contains("1 · PendingCrew"), r)
        XCTAssertTrue(r.contains("2 · 应用自动更新"), r)
        XCTAssertTrue(r.contains("2-2 · Sparkle 接入"), r)
    }

    func testDirectoryQueryFilters() {
        let f = fixture()
        let r = call(server(f), "directory", #"{"query":"2"}"#)
        XCTAssertTrue(r.contains("应用自动更新"), r)
        XCTAssertFalse(r.contains("1 · PendingCrew"), r)
    }

    // MARK: - contact 投递

    func testContactBroadcastWritesTargetWhiteboardWithoutMentions() {
        let f = fixture()
        let s = server(f)
        let r = call(s, "contact", #"{"to":"2","message":"你们那边的发布脚本能复用吗？"}"#)
        XCTAssertTrue(r.contains("已发到 2"), r)

        let landed = s.store.list(crewId: f.targetCrewId)
        XCTAssertEqual(landed.count, 1)
        XCTAssertEqual(landed[0].text, "你们那边的发布脚本能复用吗？")
        // 广播：不带 mentions（等同人类在群里无 @ 发言）——
        // 唤醒靠 externalContactFrom 这个外线标记接通到机长。
        XCTAssertNil(landed[0].mentions)
        XCTAssertEqual(landed[0].externalContactFrom, "1-2")
        // 署名一眼看出是外线：来源 crew 名 + 来源号码。
        XCTAssertEqual(landed[0].senderName, "PendingCrew · 1-2")
        XCTAssertEqual(landed[0].senderKind, "session")

        // 源群留一行回执，组织上看得见谁跨线找了谁。
        let receipts = s.store.list(crewId: f.sourceCrewId)
        XCTAssertEqual(receipts.count, 1)
        XCTAssertTrue(receipts[0].text.hasPrefix("已联系 2（应用自动更新 全群）："), receipts[0].text)
        XCTAssertNil(receipts[0].externalContactFrom)
    }

    func testContactCaptainExtensionMentionsCaptain() {
        let f = fixture()
        let s = server(f)
        _ = call(s, "contact", #"{"to":"2-1","message":"借一步说话"}"#)
        let landed = s.store.list(crewId: f.targetCrewId)
        XCTAssertEqual(landed[0].mentions?.map(\.kind), ["captain"])
    }

    func testContactSessionExtensionMentionsThatSession() {
        let f = fixture()
        let s = server(f)
        _ = call(s, "contact", #"{"to":"2-2","message":"你那个分支合了吗"}"#)
        let landed = s.store.list(crewId: f.targetCrewId)
        XCTAssertEqual(landed[0].mentions?.first?.kind, "session")
        XCTAssertEqual(landed[0].mentions?.first?.targetId, "worker-abc")
    }

    func testCaptainCallerSignsWithCaptainExtension() {
        let f = fixture()
        let s = server(f, isCaptain: true)
        _ = call(s, "contact", #"{"to":"2","message":"同步一下"}"#)
        let landed = s.store.list(crewId: f.targetCrewId)
        XCTAssertEqual(landed[0].senderName, "PendingCrew · 1-1")
        XCTAssertEqual(landed[0].externalContactFrom, "1-1")
    }

    // MARK: - contact 报错（不静默丢）

    func testContactRejectsMalformedNumber() {
        let f = fixture()
        let s = server(f)
        let r = call(s, "contact", #"{"to":"七号","message":"喂"}"#)
        XCTAssertTrue(r.contains("ERROR"), r)
        XCTAssertTrue(r.contains("directory"), r)
        XCTAssertTrue(s.store.list(crewId: f.targetCrewId).isEmpty)
        XCTAssertTrue(s.store.list(crewId: f.sourceCrewId).isEmpty)
    }

    func testContactRejectsUnknownNumber() {
        let f = fixture()
        let s = server(f)
        let r = call(s, "contact", #"{"to":"9-9","message":"喂"}"#)
        XCTAssertTrue(r.contains("查无此号"), r)
        XCTAssertTrue(s.store.list(crewId: f.sourceCrewId).isEmpty)
    }

    func testContactRejectsOwnCrew() {
        let f = fixture()
        let s = server(f)
        let r = call(s, "contact", #"{"to":"1","message":"喂"}"#)
        XCTAssertTrue(r.contains("post_to_crew"), r)
        XCTAssertTrue(s.store.list(crewId: f.sourceCrewId).isEmpty)
    }

    func testContactRejectsEmptyArgs() {
        let f = fixture()
        let s = server(f)
        XCTAssertTrue(call(s, "contact", #"{"to":"","message":"喂"}"#).contains("ERROR"))
        XCTAssertTrue(call(s, "contact", #"{"to":"2","message":"  "}"#).contains("ERROR"))
        XCTAssertTrue(s.store.list(crewId: f.targetCrewId).isEmpty)
    }

    func testReceiptSnippetIsSingleLineAndClamped() {
        let long = String(repeating: "长", count: 80) + "\n第二行"
        let snippet = McpServer.contactSnippet(long)
        XCTAssertFalse(snippet.contains("\n"))
        XCTAssertTrue(snippet.hasSuffix("…"))
        XCTAssertEqual(snippet.count, 41)
    }
}
