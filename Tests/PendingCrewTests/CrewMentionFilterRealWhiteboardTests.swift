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
/// **覆盖不到**：`CrewChatView.localUserId` 的取值 —— 它住在没有编进 test bundle
/// 的 app 模块里。所以下面用源码文本把这个链接**钉住**
/// （`testTheLinkThisBundleCannotRun`），谁改坏了当场红。这不是"验过了"，是"改动
/// 会被拦下"，两者别混。
///
/// **原来还钉着第二个链接**（`LocalBackend` 那一步映射里的 `senderUserId` 透传）。
/// 那段映射已经抽进 `CrewLocalWhiteboardMapping` 并编进了这个 bundle —— 它现在被
/// `CrewLocalWhiteboardMappingTests` **真的跑着**，所以这里的源码文本钉子撤掉了。
/// 留着会变成同一件事的第二份名单：两处各说各的，改动只更新其中一处时，那份没更新
/// 的会以「还绿着」的样子继续待着。
final class CrewMentionFilterRealWhiteboardTests: XCTestCase {

    // MARK: - 真白板

    /// **真白板的路径，显式算，不走 `LocalWhiteboardStore.defaultDirectory`。**
    ///
    /// 2026-09-09 起整趟测试的数据根被 `PENDINGCREW_DATA_DIR` 挪到了隔离目录
    /// （见 `TestProcessDataRootIsolationTests`），默认目录因此指向一个空壳。
    /// 而这三条测试**的全部意义就是读人类真实的那份数据** —— 跟着默认目录走的话，
    /// 它们会从「拿真数据验」**静默退化成每趟都 skip**，而报告上跟「这台机器没数据」
    /// 长得一模一样。
    ///
    /// 所以这里独立算真路径。**只读**：这三条一个字节都不往里写。
    private static let whiteboardDir: URL = (
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
            ?? FileManager.default.temporaryDirectory)
        .appendingPathComponent("PendingCrew", isDirectory: true)
        .appendingPathComponent("whiteboards", isDirectory: true)

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
            // **读失败不许当成「没数据」。** 2026-09-09 现场：本机数据目录出了一次
            // EPERM，这里的 `try?` 把它吞成空 → 三条一起 skip、**而且 skip 没有原因**，
            // 整套照报 `0 failures`。「一个没有原因的 skip」和「一条本来就该跳过的
            // 测试」在报告上长得一模一样 —— 那正是这次要治的东西。
            //
            // 现在分三种：读不出来 → **说出是哪个文件、什么错**再 skip（不是 pass，
            // 也不是无声）；不是数组 → 跳过这个文件（旁挂账本形状不同，正常）；
            // 正常 → 照旧逐条 lenient 解。
            let data: Data
            do { data = try Data(contentsOf: url) } catch {
                throw XCTSkip("""
                    ✗ 真白板读不出来，**这不是「本机没有数据」**：
                      \(url.lastPathComponent) — \(error.localizedDescription)

                      这三条因此没有跑。修好再来，别把这次 skip 当成通过。
                    """)
            }
            guard let rows = (try? JSONSerialization.jsonObject(with: data)) as? [Any]
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

    /// `LocalBackend.listCrewWhiteboard` 的那一步映射（`senderUserId` 是原样透传 ——
    /// 由 `CrewLocalWhiteboardMappingTests` 真的跑着断言，不再靠源码文本钉）。
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
    func testTheLinkThisBundleCannotRun() throws {
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
