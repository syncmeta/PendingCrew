#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// 本地白板落盘行 → 中栏渲染形状的映射（`CrewLocalWhiteboardMapping`）。
///
/// **这一整条链此前一条断言都没有** —— 映射内联在 `PendingCrewBackend` 里，而那个
/// 文件在单测 bundle 之外。断掉的症状是最难发现的一类：字段在盘上好好的，界面上
/// 什么都不显示，**而「漏传了一个字段」和「这条消息本来就没有那个字段」长得一模
/// 一样**。引用胶囊（人类 Todo #132/#133）尤其如此：老消息本来就一颗都不长。
final class CrewLocalWhiteboardMappingTests: XCTestCase {

    private func message(
        id: String = "m1",
        senderKind: String = "session",
        references: [CrewMessageReference]? = nil,
        inReplyTo: String? = nil,
        mentions: [LocalWhiteboardMention]? = nil
    ) -> LocalWhiteboardMessage {
        var m = LocalWhiteboardMessage(
            id: id, senderKind: senderKind, senderUserId: nil,
            senderSessionId: senderKind == "session" ? "s1" : nil,
            category: nil, text: "正文", createdAt: "2026-01-01T00:00:00Z")
        m.senderName = "小绿"
        m.references = references
        m.inReplyTo = inReplyTo
        m.mentions = mentions
        return m
    }

    func test_引用原样带到渲染那一侧() {
        let refs = [CrewMessageReference(.humanTodo, "132"),
                    CrewMessageReference(.crew, "7-1")]
        let entry = CrewLocalWhiteboardMapping.entry(message(references: refs))
        XCTAssertEqual(entry.references, refs)
    }

    func test_没有引用的老消息映射出来仍是没有() {
        XCTAssertNil(CrewLocalWhiteboardMapping.entry(message()).references)
    }

    /// 同一趟把邻居也钉住 —— 这段映射以前每加一个字段就有一次「忘了接上」的机会，
    /// 而每一次都长成「界面上什么都没有」。
    func test_回复与定向at照旧带过去() {
        let entry = CrewLocalWhiteboardMapping.entry(message(
            inReplyTo: "m0",
            mentions: [LocalWhiteboardMention(kind: "session", targetId: "s2")]))
        XCTAssertEqual(entry.inReplyTo, "m0")
        XCTAssertEqual(entry.mentions?.map(\.kind), ["session"])
        XCTAssertEqual(entry.mentions?.map(\.targetId), ["s2"])
    }

    func test_正文与作者按原有口径映射() {
        let entry = CrewLocalWhiteboardMapping.entry(message())
        XCTAssertEqual(entry.displayText, "正文")
        XCTAssertEqual(entry.senderSessionId, "s1")
        XCTAssertEqual(entry.senderDisplayName, "小绿")
        // 本机人类自己发的不折 senderName（否则中栏会把自己误判成 relay → 左对齐）。
        XCTAssertNil(CrewLocalWhiteboardMapping.entry(
            message(senderKind: "user")).senderDisplayName)
    }
}
#endif
