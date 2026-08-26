#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// Todo #69 第 3 条：「筛选开着时，自己发的消息要保留」到底成不成立 —— **拿本机真
/// 白板跑，不读代码推**。
///
/// ## 为什么非真数据不可
///
/// 这一半的成立与否全压在一个字符串相等上：白板里人类那条的 `senderUserId`，与
/// `CrewChatView.localUserId` 算出来的那个值，是不是同一个。造 fixture 时两边都是
/// 我自己写的常量，永远相等 —— 造出来的绿证明不了任何事。真正会坏的场景是
/// 「磁盘上存的是 A、UI 算出来的是 B」，只有真盘上的字节能证伪。
///
/// ## 这个测试覆盖到哪、覆盖不到哪（别含糊）
///
/// **覆盖到**：磁盘上真实的 `<crewId>.json` → `LocalWhiteboardMessage` 解码 →
/// `senderUserId` → `CrewMentionFilter.onlyHumanMentions(includingFrom:)` 的判定。
///
/// **覆盖不到**：`LocalBackend`（`PendingCrewBackend.swift`）那一步映射和
/// `CrewChatView.localUserId` 的取值 —— 两者都住在没有编进 test bundle 的 app 模块
/// 里。所以下面用源码文本把那两个链接**钉住**（`testTheTwoLinksThisBundleCannotRun`），
/// 谁改坏了当场红。这不是"验过了"，是"改动会被拦下"，两者别混。
final class CrewMentionFilterRealWhiteboardTests: XCTestCase {

    // MARK: - 真白板

    private static let whiteboardDir = LocalWhiteboardStore.defaultDirectory

    private static let missingHint = """

        ✗ Todo #69 第 3 条要拿**真白板**验，但本机没有：
          \(whiteboardDir.path)

          这份数据是人类真实的群聊内容，不入 git、也不该造。跑过 PendingCrew 的机器上
          它自然存在；干净 clone / CI 上没有，所以这里是 skip 而不是 pass。

        """

    /// 真白板上所有 `senderKind == "user"` 的行（= 人类自己发的）。
    private func realHumanMessages() throws -> [LocalWhiteboardMessage] {
        let fm = FileManager.default
        guard let files = try? fm.contentsOfDirectory(
            at: Self.whiteboardDir, includingPropertiesForKeys: nil), !files.isEmpty
        else { throw XCTSkip(Self.missingHint) }

        let decoder = JSONDecoder()
        var out: [LocalWhiteboardMessage] = []
        for url in files where url.pathExtension == "json" {
            // 旁挂账本（approvals / todos / awareness …）不是白板，跳过。
            let name = url.deletingPathExtension().lastPathComponent
            guard !name.contains(".") else { continue }
            guard let data = try? Data(contentsOf: url),
                  let rows = (try? JSONSerialization.jsonObject(with: data)) as? [Any]
            else { continue }
            // 逐条 lenient —— 与 LocalWhiteboardStore.list 同口径：一行坏掉不该让
            // 整个文件消失（而且这里是别人正在写的活文件，撞上半截很正常）。
            for row in rows {
                guard let rowData = try? JSONSerialization.data(withJSONObject: row),
                      let m = try? decoder.decode(LocalWhiteboardMessage.self, from: rowData)
                else { continue }
                if m.senderKind == "user" { out.append(m) }
            }
        }
        guard !out.isEmpty else { throw XCTSkip(Self.missingHint) }
        return out
    }

    /// `LocalBackend.listCrewWhiteboard` 的那一步映射（`senderUserId` 是原样透传，
    /// 见 `testTheTwoLinksThisBundleCannotRun`）。
    private func entry(from m: LocalWhiteboardMessage) -> CrewWhiteboardEntry {
        CrewWhiteboardEntry(
            id: m.id, senderKind: m.senderKind, senderSessionId: m.senderSessionId,
            senderUserId: m.senderUserId, senderBotId: nil, messageKind: "instruction",
            summary: m.text, createdAt: m.createdAt,
            payload: CrewWhiteboardEntry.Payload(text: m.text),
            attachments: nil, senderDisplayName: nil, senderMemberId: nil,
            inReplyTo: nil,
            mentions: m.mentions?.map { CrewMention(kind: $0.kind, targetId: $0.targetId) })
    }

    // MARK: - 1) 真盘上的 senderUserId 就是那个哨兵常量

    func testEveryRealHumanMessageCarriesTheLocalSentinelId() throws {
        let humans = try realHumanMessages()
        let ids = Set(humans.map { $0.senderUserId ?? "<nil>" })
        XCTAssertEqual(
            ids, [LocalWhiteboardStore.localUserId],
            """
            真白板上人类消息的 senderUserId 出现了 \(ids.sorted()) —— \
            只要有一个不是 \(LocalWhiteboardStore.localUserId)，「保留自己发的消息」\
            对那些消息就是坏的。
            """)
        // 样本量太小就没有证明力 —— 顺手把它写进失败信息里，免得哪天悄悄退化成 1 条。
        XCTAssertGreaterThan(humans.count, 50, "真白板上人类消息只有 \(humans.count) 条，样本太小")
    }

    // MARK: - 2) 端到端：筛选开着时，自己发的一条都不许被滤掉

    func testFilterKeepsEveryRealMessageTheHumanSent() throws {
        let humans = try realHumanMessages()
        let entries = humans.map(entry(from:))
        // 花名册取本机人类的兜底显示名（`CrewSenderNaming` 那份），与 UI 同一套。
        let roster = CrewMentionFilter.Roster(humanNames: ["人"], otherNames: ["机长"])

        let kept = CrewMentionFilter.onlyHumanMentions(
            entries, roster: roster, includingFrom: LocalWhiteboardStore.localUserId)
        XCTAssertEqual(
            kept.count, entries.count,
            "筛选把人类自己发的 \(entries.count - kept.count) 条给滤掉了（共 \(entries.count) 条）")

        // 反证：这一半真的是 `includingFrom` 挣来的，不是碰巧因为「正文里都写了 @」。
        // 不传 localUserId 时必须有一大批掉出去，否则上面那条绿是假的。
        let withoutSelf = CrewMentionFilter.onlyHumanMentions(entries, roster: roster)
        XCTAssertLessThan(
            withoutSelf.count, entries.count,
            "不传 localUserId 也一条不掉 —— 那说明上面那条绿不是 includingFrom 挣来的")
    }

    /// 传错的 id 必须失效 —— 钉住「相等判定真的在比这个字符串」，而不是恒真。
    func testAWrongLocalUserIdDoesNotKeepThem() throws {
        let entries = try realHumanMessages().map(entry(from:))
        let roster = CrewMentionFilter.Roster(humanNames: ["人"], otherNames: ["机长"])
        let kept = CrewMentionFilter.onlyHumanMentions(
            entries, roster: roster, includingFrom: "some-other-user")
        XCTAssertLessThan(kept.count, entries.count)
    }

    // MARK: - 3) 本 bundle 跑不到的那两个链接，用源码文本钉住

    /// `LocalBackend` 的映射 + `CrewChatView` 的 `localUserId` 取值都在 app 模块，
    /// 编不进 test bundle。改坏了上面两条测试**照样绿**、而人在窗口里看到的是自己
    /// 的消息全没了 —— 所以这两处只能这样拦。
    func testTheTwoLinksThisBundleCannotRun() throws {
        let backend = try Self.source("PendingCrewBackend.swift")
        XCTAssertTrue(
            backend.contains("senderUserId: m.senderUserId"),
            "本地白板 → CrewWhiteboardEntry 的映射不再原样透传 senderUserId，筛选会认不出「我」")

        let chat = try Self.source("CrewChatView.swift")
        XCTAssertTrue(
            chat.contains("LocalWhiteboardStore.localUserId"),
            "CrewChatView.localUserId 不再回落到本机哨兵常量 —— 未登录（本机常态）下它会是 nil")
        XCTAssertTrue(
            chat.contains("includingFrom: localUserId"),
            "时间线筛选没把 localUserId 喂进去，自己发的消息会被筛没")
    }

    private static func source(_ fileName: String) throws -> String {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Sources", isDirectory: true)
        guard let walker = FileManager.default.enumerator(
            at: root, includingPropertiesForKeys: nil, options: [.skipsHiddenFiles])
        else { throw XCTSkip("读不到源码目录 \(root.path)（不在开发机上跑）") }
        for case let url as URL in walker
        where url.lastPathComponent == fileName {
            return try String(contentsOf: url, encoding: .utf8)
        }
        throw XCTSkip("找不到源码文件 \(fileName)")
    }
}
#endif
