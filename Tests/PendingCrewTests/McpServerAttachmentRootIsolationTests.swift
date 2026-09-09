#if os(macOS)
import XCTest

/// 隔离数据根下，附件根必须跟着走 —— 这是 `PendingCrewDataRoot` 那套隔离唯一漏掉的一处。
///
/// **为什么这条以前没有**：`McpPostAttachmentsTests` 每次都**显式传** `attachmentRoot`，
/// 所以它测的是「传了会怎样」，从来没测过「不传会怎样」。而 helper（`McpHelperMain`）
/// 恰恰**一次都没传过** —— 于是冒烟环境里带附件发消息，附件写进真实数据根
/// `~/Library/Application Support/PendingCrew/attachments/`。
///
/// 2026-09-05 发包前审计逮到：daemon 走 env `PENDINGCREW_DATA_DIR`、helper 走 argv
/// `--dir`，而附件这处**两条通道都不走**，直接取静态默认。当天没咬人纯属运气
/// （冒烟 brief 全是纯文本）。
final class McpServerAttachmentRootIsolationTests: XCTestCase {

    /// 照 `McpHelperMain` 的构造方式来：只注入 `--dir`（= `<数据根>/whiteboards`），
    /// **不传** `attachmentRoot`。附件根必须落在同一个数据根下。
    func testAttachmentRootFollowsInjectedDirectoryWhenNotGivenExplicitly() throws {
        let root = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("attroot-\(UUID().uuidString)")
        let whiteboards = root.appendingPathComponent("whiteboards")
        try FileManager.default.createDirectory(at: whiteboards, withIntermediateDirectories: true)

        let server = McpServer(
            store: LocalWhiteboardStore(directory: whiteboards),
            approvals: LocalApprovalStore(directory: whiteboards),
            control: LocalCrewControlStore(directory: whiteboards),
            crewId: "c", sessionId: "s",
            quotaDirectory: whiteboards,
            todos: LocalTodoStore(directory: whiteboards),
            plans: CockpitPlanStore(directory: whiteboards))

        XCTAssertEqual(
            server.attachmentRoot.standardizedFileURL,
            root.appendingPathComponent("attachments").standardizedFileURL,
            "附件根没跟着注入的数据根走 —— 它落在了 \(server.attachmentRoot.path)")

        // 最要紧的那半：绝不许落在真实数据根下。
        XCTAssertNotEqual(
            server.attachmentRoot.standardizedFileURL,
            CrewChatAttachmentStore.defaultDirectory.standardizedFileURL,
            "隔离环境下附件根仍指向真实数据目录")
    }

    /// 反面：没有注入任何目录（app 进程自己用）时，仍然逐字等于原来那条路径。
    /// 附件要和人类 composer 发的图同目录同命名，这条不能被上面那条改掉。
    func testAttachmentRootStaysRealWhenNothingInjected() {
        let server = McpServer(
            store: LocalWhiteboardStore(directory: nil),
            approvals: LocalApprovalStore(directory: nil),
            control: LocalCrewControlStore(directory: nil),
            crewId: "c", sessionId: "s",
            quotaDirectory: nil)
        XCTAssertEqual(server.attachmentRoot.standardizedFileURL,
                       CrewChatAttachmentStore.defaultDirectory.standardizedFileURL)
    }
}
#endif
