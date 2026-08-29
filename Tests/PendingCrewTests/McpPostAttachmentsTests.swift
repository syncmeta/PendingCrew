import XCTest

/// `post_to_crew` 带附件（Agent Todo #48：session 能不能往群聊发图）。
///
/// 收图那半条路早就通了 —— 人类 composer 走 `appendUserMessage(attachments:)` 落盘，
/// `agentText` 把绝对路径附在正文后喂给 agent。**发图那半条从来没接上**：
/// `post_to_crew` 的 inputSchema 里没有附件字段，`appendSessionMessage*` 连形参都没有。
/// 这一族钉住 session 发得出、落得下盘、群里显示得出、接收方拿得到能 Read 的绝对路径。
///
/// claude 与 codex 共用同一个 helper（`--mcp-serve`），`agentKey` 不参与工具门禁 ——
/// 所以这里对 schema 的断言对两家同时成立，不必也不该分两套用例。
final class McpPostAttachmentsTests: XCTestCase {

    private func tempDir(_ tag: String) -> URL {
        let d = FileManager.default.temporaryDirectory
            .appendingPathComponent("mcp-attach-\(tag)-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
        return d
    }

    /// 一台钉在临时目录上的 server：白板、附件根都不碰真实数据目录。
    private func server(dir: URL, attachmentRoot: URL,
                        sessionLabel: String? = "Claude Code · abc123") -> McpServer {
        McpServer(store: LocalWhiteboardStore(directory: dir),
                  approvals: LocalApprovalStore(directory: dir),
                  control: LocalCrewControlStore(directory: dir),
                  crewId: "c", sessionId: "sess-1",
                  sessionLabel: sessionLabel,
                  attachmentRoot: attachmentRoot)
    }

    /// 造一个真实存在的文件，返回绝对路径。
    @discardableResult
    private func makeFile(_ dir: URL, _ name: String, bytes: Int = 32) -> URL {
        let url = dir.appendingPathComponent(name)
        try? Data(repeating: 0x41, count: bytes).write(to: url)
        return url
    }

    private func call(_ s: McpServer, _ arguments: String) -> String {
        toolText(s.handleLine(
            #"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"post_to_crew","arguments":"#
            + arguments + "}}") ?? "")
    }

    /// JSON-RPC 应答 → 工具回执正文。
    ///
    /// **必须解码，不能拿裸线格串做断言**：`JSONSerialization` 会把 `/` 转义成
    /// `\/`，所以线格里根本不存在 `/var/folders/…` 这个子串 —— 对着线格断言路径
    /// 会红，而接收方（MCP 客户端解完 JSON）读到的其实是对的。这条尺子要量的是
    /// **agent 真正读到的文本**，不是它在管子里的样子。
    private func toolText(_ raw: String) -> String {
        guard let data = raw.data(using: .utf8),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any],
              let result = obj["result"] as? [String: Any],
              let content = result["content"] as? [[String: Any]],
              let text = content.first?["text"] as? String
        else { return raw }
        return text
    }

    // MARK: - schema

    /// 枚举/属性表里没有这个字段，模型就不会去填它 —— 这是「能不能发」的第一道门。
    func testToolsListAdvertisesAttachmentsParam() throws {
        let r = try XCTUnwrap(server(dir: tempDir("schema"), attachmentRoot: tempDir("root"))
            .handleLine(#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#))
        XCTAssertTrue(r.contains("attachments"),
                      "post_to_crew 的 inputSchema 里没有 attachments —— session 发不出图")
    }

    // MARK: - 落盘 + 往返

    /// 给绝对路径 → 落进附件目录 → 挂到白板那条消息上。
    func testAttachmentPathLandsOnWhiteboardMessage() {
        let dir = tempDir("land"), root = tempDir("root"), src = tempDir("src")
        let png = makeFile(src, "shot.png")
        let s = server(dir: dir, attachmentRoot: root)
        _ = call(s, #"{"message":"看这张","attachments":["\#(png.path)"]}"#)

        let msgs = s.store.list(crewId: "c")
        XCTAssertEqual(msgs.count, 1)
        let atts = msgs.first?.attachments ?? []
        XCTAssertEqual(atts.count, 1, "附件没有挂到白板消息上")
        XCTAssertEqual(atts.first?.mime, "image/png")
        XCTAssertEqual(atts.first?.filename, "shot.png")
        // 落盘位置在附件根下的本 crew 目录 —— 不是原地引用 agent 的工作目录，
        // worktree 被清掉后历史气泡还渲染得出来。
        XCTAssertTrue(atts.first?.path.hasPrefix(root.appendingPathComponent("c").path) == true,
                      "附件没落进 <attachmentRoot>/<crewId>/：\(atts.first?.path ?? "nil")")
        XCTAssertTrue(FileManager.default.fileExists(atPath: atts.first?.path ?? ""),
                      "白板上记的路径在磁盘上不存在")
    }

    /// **agent 自己那份文件不许被搬走。** `CrewChatAttachmentStore.save(fileAt:)` 是
    /// move 语义（同卷零拷贝），直接拿它收 agent 给的路径会把人家的产物从工作目录里
    /// 偷走 —— agent 发完图，自己的截图就没了。
    func testOriginalFileIsLeftInPlace() {
        let dir = tempDir("keep"), root = tempDir("root"), src = tempDir("src")
        let png = makeFile(src, "keep.png")
        let s = server(dir: dir, attachmentRoot: root)
        _ = call(s, #"{"message":"发一张","attachments":["\#(png.path)"]}"#)

        XCTAssertTrue(FileManager.default.fileExists(atPath: png.path),
                      "发完图之后 agent 自己那份文件被搬走了")
    }

    /// 只发图、不写字（人类 composer 允许，session 这条路也该允许）。
    func testAttachmentOnlySendIsAllowed() {
        let dir = tempDir("only"), root = tempDir("root"), src = tempDir("src")
        let png = makeFile(src, "only.png")
        let s = server(dir: dir, attachmentRoot: root)
        _ = call(s, #"{"message":"","attachments":["\#(png.path)"]}"#)

        XCTAssertEqual(s.store.list(crewId: "c").count, 1, "只发图被当成空消息拒了")
    }

    /// 一条都没有时保持原状：空消息仍然拒。
    func testEmptyMessageWithoutAttachmentsStillRejected() {
        let s = server(dir: tempDir("empty"), attachmentRoot: tempDir("root"))
        let r = call(s, #"{"message":"  "}"#)
        XCTAssertTrue(r.contains("ERROR"), "空消息且无附件时不该放行")
        XCTAssertEqual(s.store.list(crewId: "c").count, 0)
    }

    // MARK: - 接收方拿到的路径

    /// 接收方（另一个 session）读白板时要拿到**能直接 Read 的绝对路径**。
    func testReadWhiteboardExposesAbsolutePathToPeers() {
        let dir = tempDir("read"), root = tempDir("root"), src = tempDir("src")
        let png = makeFile(src, "peer.png")
        let s = server(dir: dir, attachmentRoot: root)
        _ = call(s, #"{"message":"给你看","attachments":["\#(png.path)"]}"#)

        let stored = s.store.list(crewId: "c")[0].attachments?.first?.path ?? "nil"
        let board = toolText(s.handleLine(
            #"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"read_whiteboard","arguments":{}}}"#) ?? "")
        XCTAssertTrue(board.contains(stored),
                      "read_whiteboard 没把附件的绝对路径给接收方")
    }

    /// **别把 session 发的图说成是人发的。** 提示行原来写死「用户发来图片」——
    /// 接收方会照着这句话把作者认成人类，又是一件真事挂错对象。
    func testAgentHintDoesNotAttributeSessionImageToHuman() {
        let dir = tempDir("who"), root = tempDir("root"), src = tempDir("src")
        let png = makeFile(src, "who.png")
        let s = server(dir: dir, attachmentRoot: root, sessionLabel: "机长")
        _ = call(s, #"{"message":"我截的","attachments":["\#(png.path)"]}"#)

        let text = s.store.list(crewId: "c")[0].agentText
        XCTAssertFalse(text.contains("用户发来"),
                       "session 发的图被说成「用户发来」：\(text)")
        XCTAssertTrue(text.contains("机长"), "提示行没说清是谁发的：\(text)")
        XCTAssertTrue(text.contains("请 Read 查看"), "提示行丢了「去 Read」的动作")
    }

    /// 人类发的图照旧说「用户发来」—— 修 session 那半不许把已经对的这半改坏。
    func testHumanAttachmentHintUnchanged() {
        let dir = tempDir("human")
        let store = LocalWhiteboardStore(directory: dir)
        store.appendUserMessage(
            crewId: "c", text: "看这个", senderName: "人",
            attachments: [LocalWhiteboardAttachment(
                id: "a", mime: "image/png", size: 1, path: "/tmp/a.png")])
        XCTAssertEqual(store.list(crewId: "c")[0].agentText,
                       "看这个\n用户发来图片：/tmp/a.png（请 Read 查看）")
    }

    // MARK: - 群里显示得出来

    /// 落盘 → 中栏气泡这一段里**测得住的那一跳**：`CrewChatAdapter.adapt`。
    ///
    /// 这条路本来就不看发送者，人类发的图早就显示得出来 —— 钉它是因为「不看发送者」
    /// 是一句读代码读出来的话，而 session 发的图从来没真的走过这段。
    ///
    /// **说清楚这条测不到什么**：中间还有一跳是 `LocalBackend.listCrewWhiteboard`
    /// 把 `LocalWhiteboardMessage.attachments` 映射成 `CrewAttachment`（`path` →
    /// `file://` URL）。那个文件没编进 test bundle（见 project.yml 的白名单），
    /// 所以那一跳这里**没有覆盖**，只读过代码：映射不带任何 senderKind 条件。
    /// 真正把两跳串起来的证据得靠 GUI 上看一眼 —— 那一步没做。
    func testAdapterKeepsAttachmentsOnASessionSentMessage() throws {
        let entry = try JSONDecoder().decode(CrewWhiteboardEntry.self, from: JSONSerialization.data(
            withJSONObject: [
                "id": "e1",
                "sender_kind": "session",
                "sender_session_id": "sess-9",
                "sender_display_name": "队友",
                "message_kind": "instruction",
                "summary": "看气泡",
                "created_at": "2026-01-01T00:00:00Z",
                "attachments": [[
                    "id": "a1", "mime": "image/png", "size": 12,
                    "url": "file:///tmp/bubble.png", "filename": "bubble.png",
                ]],
            ]))

        let (msg, sender) = CrewChatAdapter.adapt(
            entry, members: [], captainBotId: nil,
            localUserId: LocalWhiteboardStore.localUserId)

        XCTAssertNotNil(sender, "session 发的消息该是别人的气泡（左对齐），不是自己的")
        XCTAssertEqual(msg.attachments?.count, 1, "附件在 adapter 这一步被丢了")
        XCTAssertEqual(msg.attachments?.first?.isImage, true,
                       "没被认成图片 —— 会退化成文件条，不在气泡里显示")
        XCTAssertEqual(msg.attachments?.first?.url, "file:///tmp/bubble.png",
                       "渲染层拿不到本地文件 URL 就读不出图")
    }

    // MARK: - 收不下的要说出来（#577 回执如实）

    /// 路径不存在 → 回执里点名说没发出去，**不许静默吞**。
    func testMissingPathIsReportedNotSwallowed() {
        let dir = tempDir("missing"), root = tempDir("root")
        let s = server(dir: dir, attachmentRoot: root)
        let r = call(s, #"{"message":"带一张","attachments":["/nope/ghost.png"]}"#)

        XCTAssertTrue(r.contains("ghost.png"), "收不下的附件没在回执里点名：\(r)")
        XCTAssertNil(s.store.list(crewId: "c").first?.attachments,
                     "收不下的附件不该挂到消息上")
    }

    /// 目录（含 .app 这类包）不是单个文件，照 `CrewFileAttachmentIntake` 的口径拒，
    /// 且用同一套软报错文案 —— 不新造第二份判定。
    func testDirectoryIsRejectedWithSharedWording() {
        let dir = tempDir("dirarg"), root = tempDir("root"), src = tempDir("src")
        let folder = src.appendingPathComponent("bundle.app", isDirectory: true)
        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let s = server(dir: dir, attachmentRoot: root)
        let r = call(s, #"{"message":"带个目录","attachments":["\#(folder.path)"]}"#)

        XCTAssertTrue(r.contains("bundle.app"), "目录被静默丢了：\(r)")
        XCTAssertTrue(r.contains("压成 zip"),
                      "没有复用 CrewFileAttachmentIntake 的软报错文案：\(r)")
    }

    /// 一好一坏：好的照发，坏的照报 —— 不因为一条坏的把整条消息毙了。
    func testGoodAttachmentSurvivesAlongsideBadOne() {
        let dir = tempDir("mixed"), root = tempDir("root"), src = tempDir("src")
        let png = makeFile(src, "good.png")
        let s = server(dir: dir, attachmentRoot: root)
        let r = call(s, #"{"message":"两个","attachments":["\#(png.path)","/nope/bad.png"]}"#)

        XCTAssertEqual(s.store.list(crewId: "c").first?.attachments?.count, 1,
                       "好的那份没发出去")
        XCTAssertTrue(r.contains("bad.png"), "坏的那份没报出来：\(r)")
    }
}
