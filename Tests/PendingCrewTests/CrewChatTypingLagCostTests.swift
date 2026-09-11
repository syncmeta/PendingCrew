import XCTest

/// 「打字都不畅顺」（人类 Todo #140）第二半的**成本回归**：聊天窗主线程上三处具名的
/// 重复构造。现场读数在 `docs/internal/2026-09-11-typing-lag-profile.md`，原始 sample
/// 在 `docs/internal/samples/2026-09-11-typing-lag/`。
///
/// ## 它量的是「做了几次」，不是「快了多少毫秒」
///
/// 理由写在 `CrewChatCostCounters` 的注释里：那趟现场采样的机器在换页（free 0.9 GB、
/// 交换只剩 798 MB、load 5.9），主线程忙的比例两趟分别是 77% 和 45% —— 毫秒在这种机器
/// 上不可比。所以断言全部钉在离散计数上，毫秒只 `print` 出来当观测，一条也不断言。
///
/// ## 这一整个文件绿了，证明什么、不证明什么
///
/// **证明**：三处重复构造不再发生，且缓存在输入变化时会失效（不吐旧结果）。
///
/// **不证明界面变快了。** 真验收要对装了这一版的界面进程重新 `sample` 一趟、跟
/// `samples/2026-09-11-typing-lag/pc-sample-3s.txt.gz` 比 `libicucore` 那一栏 ——
/// 本 session 装不了新版（详见收尾报告的边界那一栏），所以这里只有测试层面的证据。
///
/// ## 语料是合成的，规模照着真白板配
///
/// 真白板（本 crew）不入 git，所以语料在 `Corpus` 里按现场量到的规模合成：
/// **2618 条、正文 ≈ 965 KB、正文里 814 个 `@`**。下面第一条用例就是守这个规模的 ——
/// 语料一旦退化成玩具，后面所有成本数字都不再说明任何事（#443 那套测试吃过这个教训：
/// 「造数据量不出真问题」）。
final class CrewChatTypingLagCostTests: XCTestCase {

    // MARK: - 语料

    /// 照着 2026-09-11 现场实测的规模合成。确定性（无随机），所以跨机器跨趟可比。
    enum Corpus {
        /// 现场实测：本 crew 白板 2618 条。
        static let messageCount = 2618
        /// 现场实测：正文 965 KB。
        static let bodyBytesTarget = 965 * 1024
        /// 现场实测：正文里 814 个 `@`。
        static let atCount = 814

        /// 花名册：一个人类 + 若干 session/bot 名。
        ///
        /// **`人机交互组` 是故意放进来的**：它以人类名「人」开头，是
        /// `CrewMentionFilter` 那套「取最长匹配」的唯一理由。成本优化要是把最长匹配
        /// 退化成「撞上第一个就算」，`@人机交互组` 会被误判成 @ 了人类 —— 那是行为变了，
        /// 不是变快了。
        static let humanNames = ["人"]
        static let otherNames = [
            "机长", "人机交互组", "小绿", "聊天窗主线程三处浪费", "孤儿 helper 泄漏",
            "装了新版工具却没换", "起步就失败却显示空闲", "驾驶舱改造", "跨机与Workspace",
            "总机长功能", "ACP 接入研判", "Codex 可用性", "应用自动更新", "开源与发版",
            "技术栈梳理", "协作系统规划", "聊天记录搜索", "常驻后台·前后端分离",
            "预设文案逐条手改", "机组群聊体验", "Changyon", "Crew自举打磨",
        ]

        static var roster: CrewMentionFilter.Roster {
            CrewMentionFilter.Roster(humanNames: humanNames, otherNames: otherNames)
        }

        /// 一段足够长的正文填充（≈ 965 KB / 2618 条 ≈ 377 字节一条）。
        private static let filler = String(
            repeating: "主线程忙的比例两趟分别是 77% 和 45%，差这么多说明是阵发的。", count: 6)

        /// 2618 条消息。其中 814 条正文里带一个 `@`，按固定步长分布；`@` 后面轮换
        /// 「人 / 人机交互组 / 机长 / 小绿」四种，所以**命中与不命中都有真实份额**。
        static let entries: [CrewWhiteboardEntry] = {
            let atEvery = max(1, messageCount / atCount)
            let atTargets = ["人", "人机交互组", "机长", "小绿"]
            var out: [CrewWhiteboardEntry] = []
            out.reserveCapacity(messageCount)
            for i in 0 ..< messageCount {
                var text = "第 \(i) 条。" + filler
                if i % atEvery == 0 {
                    text = "@\(atTargets[(i / atEvery) % atTargets.count]) " + text
                }
                // 结构化 mention 一条都不给：要逼着判定走**正文**那一半（贵的那一半）。
                out.append(CrewWhiteboardEntry(
                    id: "m\(i)", senderKind: "session", senderSessionId: "s\(i % 17)",
                    senderUserId: nil, senderBotId: nil, messageKind: "announcement",
                    summary: nil,
                    createdAt: i % 2 == 0
                        ? "2026-09-11T17:00:00Z"          // 不带小数秒（本机写的）
                        : "2026-09-11T17:00:00.123Z",     // 带小数秒（relay 搬进来的）
                    payload: CrewWhiteboardEntry.Payload(text: text),
                    attachments: nil, senderDisplayName: "成员\(i % 17)",
                    senderMemberId: nil, inReplyTo: nil, mentions: nil))
            }
            return out
        }()

        static var inputs: CrewTimelineFilter.Inputs {
            CrewTimelineFilter.Inputs(
                entries: entries, onlyMentions: true, roster: roster,
                localUserId: "local-byok-user", searchText: "",
                crewId: "crew-1", crewTitle: "PendingCrew")
        }
    }

    /// 语料规模守门 —— **必须是第一条看的用例**。它红了，下面每一个成本数字都失去意义。
    func test_语料规模对得上现场读数() {
        let bodyBytes = Corpus.entries
            .reduce(0) { $0 + $1.displayText.utf8.count }
        let ats = Corpus.entries
            .reduce(0) { $0 + $1.displayText.filter { $0 == "@" }.count }
        print("""

        ╔══ [#140] 语料规模（对着 2026-09-11 现场实测配，刻意不低于现场）
        ║ 条数      \(Corpus.entries.count) 条      = 现场 2618 条
        ║ 正文字节  \(bodyBytes) B   = 现场 965 KB 的 \
        \(String(format: "%.2f", Double(bodyBytes) / Double(Corpus.bodyBytesTarget))) 倍
        ║ `@` 个数  \(ats) 个        = 现场 814 个的 \
        \(String(format: "%.2f", Double(ats) / Double(Corpus.atCount))) 倍
        ╚══ 比现场大是安全方向（成本测试更难过），比现场小才是问题 —— 下面只断言下界。

        """)
        XCTAssertEqual(Corpus.entries.count, 2618, "条数要对得上现场读数")
        XCTAssertGreaterThan(bodyBytes, 800 * 1024, "正文体量不许退化成玩具语料")
        XCTAssertGreaterThan(ats, 700, "`@` 的个数是 ② 的主要成本来源，不许退化")
    }

    // MARK: - ① ISO8601DateFormatter 反复新建（现场 libicucore 7.1%）

    /// 整个进程里这两个格式器**一共**只该被构造 2 次。
    ///
    /// 这一处**故意断言绝对值**，与 `CrewChatCostCounters.delta` 的纪律相反，因为它的
    /// 上界由「两个 `static let`」这个结构本身封死，与用例执行顺序无关。
    ///
    /// 它同时是「**这把尺子是活的**」的证明：一个接错了、永远返回 0 的计数器在这里会红
    /// （期望是 2 而不是 0）。下面那些 `delta == 0` 的断言单独看不出计数器死没死，
    /// 靠这一条撑着。
    func test_群聊解析路一共只构造两个ISO8601格式器() {
        // 先把懒加载那两下跑掉（它们就是那 2 次）。
        _ = CrewTimestamp.parse("2026-09-11T17:00:00Z")
        _ = CrewTimestamp.parse("2026-09-11T17:00:00.123Z")

        let iso = ["2026-09-11T17:00:00Z", "2026-09-11T17:00:00.123Z", "garbage", ""]
        let delta = CrewChatCostCounters.delta(.iso8601FormatterBuild) {
            for i in 0 ..< 500 {
                let s = iso[i % iso.count]
                _ = CrewTimestamp.parse(s)
                _ = CrewMemberOrdering.parseDate(s)
                _ = CrewMessageSearch.parseISO(s)
            }
        }
        let total = CrewChatCostCounters.total(.iso8601FormatterBuild)
        print("[#140 ①] 1500 次解析之后：新增构造 \(delta) 次，进程累计 \(total) 次（期望 0 / 2）")
        XCTAssertEqual(delta, 0, "热路径上一次都不该再构造 ISO8601DateFormatter")
        XCTAssertEqual(total, 2, "整个进程只该有两个格式器（带小数秒 / 不带）")
    }

    /// 排 200 个成员一次，不许构造任何格式器。
    ///
    /// 这就是现场那条路：`CrewSessionWindowView.memberRowItems` 是计算属性，每次 body
    /// 求值重排一遍，每个成员每次都过 `parseDate`。
    func test_排两百个成员不构造任何格式器() {
        _ = CrewTimestamp.parse("2026-09-11T17:00:00Z")   // 预热懒加载
        let members = (0 ..< 200).map { i in
            CrewMemberOrdering.Key(
                id: "m\(i)", isPinned: i < 3,
                createdAt: CrewTimestamp.parse("2026-09-11T17:00:00.\(i % 900)Z"))
        }
        let raw = (0 ..< 200).map { i in "2026-09-11T17:00:0\(i % 10).\(100 + i % 800)Z" }
        let delta = CrewChatCostCounters.delta(.iso8601FormatterBuild) {
            for _ in 0 ..< 20 {
                _ = CrewMemberOrdering.sortedIds(members)
                for s in raw { _ = CrewMemberOrdering.parseDate(s) }
            }
        }
        print("[#140 ①] 排 200 个成员 × 20 帧 + 4000 次 parseDate：新增构造 \(delta) 次（期望 0）")
        XCTAssertEqual(delta, 0, "排序/解析路不许每次新建格式器（现场 libicucore 7.1% 全在这儿）")
    }

    /// 搜索路同理：`CrewMessageSearch.search` 对**每个 document** 都调一次 `parseISO`。
    func test_搜索两千六百条不构造任何格式器() {
        _ = CrewMessageSearch.parseISO("2026-09-11T17:00:00Z")   // 预热
        let documents = Corpus.entries.map {
            CrewMessageSearchAdapters.entry($0, crewId: "crew-1", crewTitle: "PendingCrew")
        }
        let delta = CrewChatCostCounters.delta(.iso8601FormatterBuild) {
            _ = CrewMessageSearch.search(documents, query: "阵发", limit: 200, order: .newestFirst)
        }
        print("[#140 ①] 搜索 \(documents.count) 条：新增构造 \(delta) 次（期望 0）")
        XCTAssertEqual(delta, 0, "搜一次不许构造 2×N 个格式器")
    }

    /// 共享实例**能不能共享**，这一条是实测不是引文。
    ///
    /// 边界：它证明的是「并发调 `date(from:)` 的**结果**与串行逐条一致」。它**不是**
    /// 数据竞争的证明 —— 这一趟没开 TSan，也没有任何内存模型层面的保证。真正的依据是
    /// 「构造完就不再写 `formatOptions`」这个结构事实（所以是两个实例，而不是一个来回切）。
    func test_共享格式器并发解析结果与串行一致() {
        let samples = [
            "2026-09-11T17:17:00Z", "2026-09-11T17:17:00.123Z", "2026-09-11T17:17:00+08:00",
            "2026-09-11T17:17:00.123456+08:00", "2026-09-11 17:17:00Z", "", "garbage",
        ]
        let serial = samples.map { CrewTimestamp.parse($0) }
        let lock = NSLock()
        var mismatches = 0
        DispatchQueue.concurrentPerform(iterations: 8) { _ in
            for _ in 0 ..< 500 {
                for (i, s) in samples.enumerated() where CrewTimestamp.parse(s) != serial[i] {
                    lock.lock(); mismatches += 1; lock.unlock()
                }
            }
        }
        print("[#140 ①] 8 队列 × 500 轮 × \(samples.count) 串并发解析：不一致 \(mismatches) 次（期望 0）")
        XCTAssertEqual(mismatches, 0, "共享格式器在并发只读解析下结果必须与串行一致")
    }

    /// 行为不变：三个入口对同一批字符串的结果，与改动前那三段代码逐条一致。
    ///
    /// 参照实现**就是改动前那几行的原样抄录**（带各自原来的尝试顺序：成员排序是
    /// 「先带小数秒」，搜索是「先不带」）。它是一份会跟着一起错的替身，所以它只负责
    /// 一件事：两边不一致就红。它不给生产实现背书。
    func test_三个解析入口的结果与改动前逐条一致() {
        let corpus = [
            "2026-09-11T17:17:00Z", "2026-09-11T17:17:00.1Z", "2026-09-11T17:17:00.123Z",
            "2026-09-11T17:17:00.123456Z", "2026-09-11T17:17:00+08:00",
            "2026-09-11T17:17:00.5+08:00", "2026-09-11T17:17:00-05:00",
            "2026-01-01T00:00:00Z", "1970-01-01T00:00:00.000Z",
            "2026-09-11 17:17:00Z", "2026-09-11", "17:17:00", "garbage", "", "@人",
        ]
        func oldMemberOrdering(_ raw: String?) -> Date? {
            guard let raw, !raw.isEmpty else { return nil }
            let withFraction = ISO8601DateFormatter()
            withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            return withFraction.date(from: raw) ?? ISO8601DateFormatter().date(from: raw)
        }
        func oldSearch(_ value: String) -> Date? {
            let plain = ISO8601DateFormatter()
            if let date = plain.date(from: value) { return date }
            let fractional = ISO8601DateFormatter()
            fractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            return fractional.date(from: value)
        }
        func oldTimestamp(_ iso: String) -> Date? {
            let f = ISO8601DateFormatter()
            f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
            if let d = f.date(from: iso) { return d }
            f.formatOptions = [.withInternetDateTime]
            return f.date(from: iso)
        }
        for s in corpus {
            XCTAssertEqual(CrewMemberOrdering.parseDate(s), oldMemberOrdering(s), "成员排序解析：\(s)")
            XCTAssertEqual(CrewMessageSearch.parseISO(s), oldSearch(s), "搜索解析：\(s)")
            XCTAssertEqual(CrewTimestamp.parse(s), oldTimestamp(s), "时间戳解析：\(s)")
        }
        XCTAssertNil(CrewMemberOrdering.parseDate(nil), "nil 仍然是 nil")
    }

    // MARK: - ② 花名册判定索引每条消息重建一次

    /// 筛一整群（2618 条 / 814 个 `@`），判定索引只该建**一次**。
    ///
    /// 改动前它建 2618 次（`bodyMentionsHuman` 每条消息重建一次
    /// `Set(roster.humanNames.map { $0.lowercased() })`）。
    func test_筛两千六百条只建一次花名册判定索引() {
        var kept = 0
        // **花名册的构造必须在量的窗口里面** —— 第一版把它挪到了外面，于是量到 0 次，
        // 那个「比期望还少」的红正好暴露了窗口划错了。改动前这里是 2618 次（每条消息
        // 一次），现在是 1 次（跟着 Roster 一次）；窗口外建 Roster 会得到 0，两种都不对。
        let delta = CrewChatCostCounters.delta(.mentionMatcherBuild) {
            let roster = Corpus.roster
            kept = CrewMentionFilter.onlyHumanMentions(
                Corpus.entries, roster: roster, includingFrom: "local-byok-user").count
        }
        print("[#140 ②] 筛 \(Corpus.entries.count) 条（留下 \(kept) 条）：判定索引建了 \(delta) 次（期望 1）")
        XCTAssertEqual(delta, 1, "花名册判定索引必须跟着 Roster 建一次，不是每条消息建一次")
        XCTAssertGreaterThan(kept, 0, "语料必须真有命中，否则走不到贵的那条路")
        XCTAssertLessThan(kept, Corpus.entries.count, "也必须真有不命中的，否则最长匹配没被走到")
    }

    /// **证明这把尺子会叫**：显式建 7 个 Roster，计数就该是 7。
    ///
    /// 不做这一条的话，上面那个 `delta == 1` 和「计数器根本没接上（恒 0）」分不开 ——
    /// 那种尺子平时永远是绿的，正是本仓反复抓到的「从不发声的检查」。
    func test_花名册计数器会叫() {
        let delta = CrewChatCostCounters.delta(.mentionMatcherBuild) {
            for i in 0 ..< 7 {
                _ = CrewMentionFilter.Roster(humanNames: ["人\(i)"], otherNames: ["机长"])
            }
        }
        print("[#140 ②] 显式建 7 个 Roster：计数 \(delta) 次（期望 7 —— 证明尺子不是恒 0）")
        XCTAssertEqual(delta, 7, "建 N 个 Roster 就该记 N 次；记 0 次说明尺子没接上")
    }

    /// 行为不变：正文最长匹配的判定结果与改动前逐条一致（含那几条会咬人的）。
    func test_正文最长匹配行为与改动前一致() {
        let roster = Corpus.roster
        let cases: [(String, Bool, String)] = [
            ("@人 这条是给人看的", true, "正好是人类名"),
            ("@人机交互组 这条是给那个组的", false, "以人类名开头的更长成员名 —— 不算命中"),
            ("@机长 帮忙拍一下", false, "别的成员"),
            ("@小绿 和 @人 都在", true, "多个 @，有一个是人类"),
            ("@人机交互组 和 @机长", false, "两个都不是人类"),
            ("@", false, "光一个 @ 在末尾"),
            ("邮箱 a@b.c 不算", false, "@ 后面不是任何成员名"),
            ("@人", true, "@ 加人类名顶到末尾"),
            ("没有任何 at", false, "一个 @ 都没有"),
            ("@@人", true, "连着两个 @"),
            ("@人机交互组@人", true, "前一个不是、后一个是"),
        ]
        for (text, expected, why) in cases {
            XCTAssertEqual(
                CrewMentionFilter.bodyMentionsHuman(text: text, roster: roster), expected,
                "\(why)：\(text)")
        }
        // 花名册为空 → 正文那一半整条关掉（改动前同样）。
        XCTAssertFalse(CrewMentionFilter.bodyMentionsHuman(
            text: "@人 在吗", roster: CrewMentionFilter.Roster(humanNames: [])))
    }

    /// 观测（不断言）：「每条消息重建一次 Roster」对「一份 Roster 用到底」的毫秒差。
    ///
    /// 左边那一栏就是改动前的成本形状 —— 它走的是同一份生产代码，只是把 Roster 的
    /// 构造挪进了循环里，所以不是替身。
    func test_观测_花名册复用与每条重建的毫秒差() {
        let sample = Array(Corpus.entries.prefix(400))
        func ms(_ body: () -> Void) -> Double {
            let t = DispatchTime.now().uptimeNanoseconds
            body()
            return Double(DispatchTime.now().uptimeNanoseconds - t) / 1_000_000
        }
        let reused = Corpus.roster
        let tReuse = ms {
            for e in sample { _ = CrewMentionFilter.isHumanMention(e, roster: reused) }
        }
        let tRebuild = ms {
            for e in sample {
                _ = CrewMentionFilter.isHumanMention(e, roster: Corpus.roster)
            }
        }
        print(String(
            format: "[#140 ②] %d 条：复用一份花名册 %.2f ms / 每条重建 %.2f ms（只作观测，不断言）",
            sample.count, tReuse, tRebuild))
    }

    // MARK: - ③ 一帧里整条时间线被筛八遍

    /// 一帧读八次（`CrewChatView` 实际的读法），只该真筛**一遍**。
    func test_一帧读八次只筛一遍() {
        let cache = CrewTimelineFilterCache()
        let inputs = Corpus.inputs
        var results: [[CrewWhiteboardEntry]] = []
        let delta = CrewChatCostCounters.delta(.timelineFilterRun) {
            for _ in 0 ..< 8 { results.append(cache.entries(for: inputs)) }
        }
        print("[#140 ③] 同一帧读 8 次 \(Corpus.entries.count) 条：真筛了 \(delta) 遍（期望 1）")
        XCTAssertEqual(delta, 1, "一帧里八次访问必须只筛一遍")
        XCTAssertEqual(Set(results.map(\.count)).count, 1, "八次读必须拿到同一份结果")
        XCTAssertEqual(results[0].map(\.id), CrewTimelineFilter.resolve(inputs).map(\.id),
                       "缓存的结果必须与直算逐条一致")
    }

    /// **证明它该红的时候会红**：输入的每一个字段变了，都必须重筛一遍。
    ///
    /// 一个只会说「已经算过了」的缓存在成本上永远完美，在行为上是个 bug ——
    /// 界面会吐旧内容。所以这五条和上面那一条是一对，缺一条都不算数。
    func test_输入每变一样都必须重筛() {
        let cache = CrewTimelineFilterCache()
        let base = Corpus.inputs
        _ = cache.entries(for: base)

        func mutated(_ label: String, _ inputs: CrewTimelineFilter.Inputs) {
            let delta = CrewChatCostCounters.delta(.timelineFilterRun) {
                _ = cache.entries(for: inputs)
            }
            XCTAssertEqual(delta, 1, "\(label) 变了必须重筛")
            // 变回去也要重筛（单槽缓存，不是一张表）。
            let back = CrewChatCostCounters.delta(.timelineFilterRun) {
                _ = cache.entries(for: base)
            }
            XCTAssertEqual(back, 1, "\(label) 改回来时槽位里是另一份输入，也必须重筛")
        }

        mutated("条目（少一条）", .init(
            entries: Array(base.entries.dropLast()), onlyMentions: base.onlyMentions,
            roster: base.roster, localUserId: base.localUserId, searchText: base.searchText,
            crewId: base.crewId, crewTitle: base.crewTitle))
        mutated("筛选开关", .init(
            entries: base.entries, onlyMentions: false, roster: base.roster,
            localUserId: base.localUserId, searchText: base.searchText,
            crewId: base.crewId, crewTitle: base.crewTitle))
        mutated("花名册", .init(
            entries: base.entries, onlyMentions: base.onlyMentions,
            roster: CrewMentionFilter.Roster(humanNames: ["另一个人"], otherNames: ["机长"]),
            localUserId: base.localUserId, searchText: base.searchText,
            crewId: base.crewId, crewTitle: base.crewTitle))
        mutated("本机 user id", .init(
            entries: base.entries, onlyMentions: base.onlyMentions, roster: base.roster,
            localUserId: nil, searchText: base.searchText,
            crewId: base.crewId, crewTitle: base.crewTitle))
        mutated("搜索词", .init(
            entries: base.entries, onlyMentions: base.onlyMentions, roster: base.roster,
            localUserId: base.localUserId, searchText: "阵发",
            crewId: base.crewId, crewTitle: base.crewTitle))
    }

    /// **条数一样但内容变了**也必须重筛 —— 撤回一条又来一条、订阅重放就是这个形状。
    ///
    /// 这一条专门挡「拿条数/指纹当缓存键」那种便宜做法：它平时都对，只在这里错。
    func test_条数没变但内容变了也必须重筛() {
        let cache = CrewTimelineFilterCache()
        let base = Corpus.inputs
        _ = cache.entries(for: base)

        var swapped = base.entries
        let last = swapped.removeLast()
        swapped.append(CrewWhiteboardEntry(
            id: last.id, senderKind: last.senderKind, senderSessionId: last.senderSessionId,
            senderUserId: nil, senderBotId: nil, messageKind: last.messageKind,
            summary: nil, createdAt: last.createdAt,
            payload: CrewWhiteboardEntry.Payload(text: "@人 换了一句完全不同的正文"),
            attachments: nil, senderDisplayName: last.senderDisplayName,
            senderMemberId: nil, inReplyTo: nil, mentions: nil))
        XCTAssertEqual(swapped.count, base.entries.count, "这一条的前提就是条数没变")

        let changed = CrewTimelineFilter.Inputs(
            entries: swapped, onlyMentions: base.onlyMentions, roster: base.roster,
            localUserId: base.localUserId, searchText: base.searchText,
            crewId: base.crewId, crewTitle: base.crewTitle)
        let delta = CrewChatCostCounters.delta(.timelineFilterRun) {
            _ = cache.entries(for: changed)
        }
        XCTAssertEqual(delta, 1, "条数相同、内容不同 → 必须重筛")
        XCTAssertEqual(cache.entries(for: changed).map(\.id),
                       CrewTimelineFilter.resolve(changed).map(\.id), "结果仍要与直算一致")
    }

    /// 搜索那一半也要等价（它是 `resolve` 里第二段，容易在重构中被漏掉）。
    func test_搜索路的结果与直算一致() {
        let cache = CrewTimelineFilterCache()
        for (onlyMentions, query) in [(true, "阵发"), (false, "阵发"), (true, ""), (false, "第 7 条")] {
            let inputs = CrewTimelineFilter.Inputs(
                entries: Array(Corpus.entries.prefix(600)), onlyMentions: onlyMentions,
                roster: Corpus.roster, localUserId: "local-byok-user", searchText: query,
                crewId: "crew-1", crewTitle: "PendingCrew")
            XCTAssertEqual(cache.entries(for: inputs).map(\.id),
                           CrewTimelineFilter.resolve(inputs).map(\.id),
                           "onlyMentions=\(onlyMentions) query=「\(query)」")
        }
    }

    /// 观测（不断言）：一帧八次读，缓存前后的毫秒。
    func test_观测_一帧八次读的毫秒差() {
        func ms(_ body: () -> Void) -> Double {
            let t = DispatchTime.now().uptimeNanoseconds
            body()
            return Double(DispatchTime.now().uptimeNanoseconds - t) / 1_000_000
        }
        let inputs = Corpus.inputs
        let cache = CrewTimelineFilterCache()
        _ = cache.entries(for: inputs)                       // 先把第一次算掉
        let cached = ms { for _ in 0 ..< 8 { _ = cache.entries(for: inputs) } }
        let uncached = ms { for _ in 0 ..< 8 { _ = CrewTimelineFilter.resolve(inputs) } }
        print(String(format: "[#140 ③] 一帧八次读 %d 条：有缓存 %.2f ms / 每次重算 %.2f ms（只作观测）",
                     Corpus.entries.count, cached, uncached))
    }
}
