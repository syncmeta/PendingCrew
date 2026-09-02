import Foundation

/// 「一轮收尾停在一个没人被叫到的问句上，系统替它把这个问题发进群」的判定 + 文案
/// （人类 Todo #25 层 1）。
///
/// **要解决的病**：session 在终端里用大白话问了一句然后停住 —— 既没调 `ask`，画面上
/// 也没有带序号的选择菜单，于是 `SessionPendingDecision` 那两条路一条都不占，群里一个
/// 字都没有，人和机长都不知道它在等，session 事实上死掉。
///
/// **判据只有一条：它是不是停在一个没 @ 到人的问题上。**「这一轮往群里说过话没有」
/// 不是判据 —— 曾经是，那是个错误（#25 首版）：机长在等 worker 干活、这一轮无话可说
/// 是完全正常的状态，却被播成「没往群里说过话…需要人回它一句」，等于凭空给人造待办，
/// 两个 crew 群同时被同一句式刷屏。**静默是正常状态，不是异常信号。**
///
/// **与层 2 同源，别分叉**：这里的「停在问句上」用的就是 `SessionAwaitingReply` 判
/// `.question` 的那个 `trailingQuestion`。那个文件是「在等谁」的唯一判定
/// （approval / menu / question 三条），层 1 只是把其中 question 那条的结果**捅进群里**，
/// 不许另立一套更弱的判据（「没说话」「没动静」「久了」都不行）—— 界面标红和群里代发
/// 必须是同一件事的两个出口，否则一定会再退化回凭空造待办。
///
/// 选靶规则与 `SessionDecisionNotice`（macOS-only，本文件跨平台编译故不能直接引用）
/// **保持同一套**，改一处务必改两处：
/// - worker → @机长（机长有 inspect_session / nudge_session，能自己拍的就拍了，不惊动人）
/// - 机长 → @人。**绝不 @ 自己** —— @captain 会触发「目标缺席拉起」，卡着的机长起不来
///   又发一条，就此成环（#541 同款坑）。
///
/// 判定全是可注入输入的纯函数（照 `PendingDecisionTracker` / `CrewMailboxWakeLogic` 的
/// 风格），时间与 IO 都由调用方喂 → 单测钉得住。
enum SessionTurnTrace {

    // MARK: - 有没有人被叫到

    /// `since` 之后，这个 session 自己说的话里**有没有点到能答的人**（@机长 / @人）。
    ///
    /// 这是「别重复叫人」的闸：它停在问句上、但已经自己 @ 了机长或人 —— 群里已经有人
    /// 被叫到了，系统再补一条只是噪音。反过来，广播一句「…要不要接着做 B？」谁都没 @
    /// 的，界面这时会把它标红等回复，群里却没有一个人知道该去答，那才是要补的口子。
    ///
    /// 注意「群里有它的字」不等于「有人被叫到」—— 只 @ 了别的 session 也不算，那个
    /// session 拍不了板。
    ///
    /// - Parameter since: 上一轮结束时的白板末条 id；nil = 头一回（把在场消息都当本轮
    ///   范围，宁可判「已经叫过人」而少发一条，噪音是这功能的头号死因）。
    static func hasCallOutTrace(
        in messages: [LocalWhiteboardMessage], sessionId: String, since: String?
    ) -> Bool {
        var slice = messages
        if let since, let i = messages.firstIndex(where: { $0.id == since }) {
            slice = Array(messages[(i + 1)...])
        }
        return slice.contains { m in
            m.senderSessionId == sessionId
                && (m.mentions ?? []).contains { $0.kind == "captain" || $0.kind == "human" }
        }
    }

    // MARK: - 收尾话头

    /// 收尾**那一句**，且必须是问句 —— 层 1 代发与层 2「待回复标红」共用的**唯一**判据
    /// （见 `SessionAwaitingReply`）。
    ///
    /// **必须严**：中途反问然后自己接着干完的那种（「这样对吗？我先按 A 做了。」）不是
    /// 在等人，收尾不是问句就返 nil。曾经另有一个宽判据（`closing`，正文中间的问句也捞、
    /// 没问句就退回收尾那句）专供层 1 代发用 —— 已删：宽判据配上「没说话就发」的弱门，
    /// 正是把陈述句收尾播成「需要人回它一句」的那条路。别再加回来。
    ///
    /// 已知边界：`split` 不认半角句点（`1.5` / `main.swift` / `…` 会被切碎），所以整段
    /// 英文常被当成一句。方向是安全的 —— 以陈述句收尾的英文段落整体不以问号结尾 → 不红；
    /// 以问号收尾的会红，只是带的原文长一点。**不误报**才是这条判据的命门。
    static func trailingQuestion(from raw: String, limit: Int = 160) -> String? {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, let last = split(text).last,
              hasContent(last), endsAsQuestion(last) else { return nil }
        return truncate(last, limit)
    }

    /// 收尾闭合符 —— 句末标点**后面**还能合法跟着的那些字符。它们是上一句的尾巴，
    /// 不是新的一句（见 `split` 的合并、`endsAsQuestion` 的剥离）。
    private static let closers: Set<Character> = [
        "）", ")", "」", "』", "】", "》", "〉", "〕", "］", "]", "｝", "}",
        "”", "’", "\"", "'"
    ]

    /// 末尾挂着闭合符时照样算问句：「…这样行吗？）」「…这样行吗？”」。
    ///
    /// **这不是洁癖**：收窄后「停在问句上」是本功能**唯一**的触发条件，正文以
    /// 「（顺带一提：……行吗？）」这种极常见的形式收尾就漏判，等于整个功能静默失灵。
    private static func endsAsQuestion(_ s: String) -> Bool {
        var t = Substring(s)
        while let l = t.last, closers.contains(l) || l == " " { t = t.dropLast() }
        guard let l = t.last else { return false }
        return l == "？" || l == "?"
    }

    /// 剥掉标点、符号与空白后还剩东西没有 —— 没有就不是「一句话」，别拿去当收尾话头
    /// （事故现场：正文以「……不该产生任何群消息。）」收尾，代发稿的正文只有一个 `）`）。
    private static func hasContent(_ s: String) -> Bool {
        s.unicodeScalars.contains {
            !CharacterSet.punctuationCharacters.contains($0)
                && !CharacterSet.symbols.contains($0)
                && !CharacterSet.whitespacesAndNewlines.contains($0)
        }
    }

    /// 切句：按中英文句末标点与换行切，保留标点（问号是判据，丢了就认不出问句）。
    ///
    /// 切完还要**把只剩闭合符的碎片并回前一句**：`。` 触发切分后，紧跟其后的 `）`
    /// 会独自成为「一句」，于是收尾话头抽出来是个孤零零的右括号（真事故），而
    /// 「…行吗？）」的末段变成 `）` 更直接让问句判不出来。
    private static func split(_ text: String) -> [String] {
        var out: [String] = []
        var cur = ""
        for ch in text {
            if ch == "\n" {
                if !cur.trimmingCharacters(in: .whitespaces).isEmpty { out.append(clean(cur)) }
                cur = ""
                continue
            }
            cur.append(ch)
            if "。？！?!".contains(ch) {
                out.append(clean(cur))
                cur = ""
            }
        }
        if !cur.trimmingCharacters(in: .whitespaces).isEmpty { out.append(clean(cur)) }

        var merged: [String] = []
        for piece in out where !piece.isEmpty {
            if isClosersOnly(piece), let prev = merged.last {
                merged[merged.count - 1] = prev + piece
            } else {
                merged.append(piece)
            }
        }
        return merged
    }

    /// 整段只由闭合符与空白构成 —— 这种碎片没有独立成句的资格。
    /// （没有前一句可并时它会原样留下，随后被 `hasContent` 判掉。）
    private static func isClosersOnly(_ s: String) -> Bool {
        !s.isEmpty && s.allSatisfy { closers.contains($0) || $0 == " " }
    }

    /// 去掉 markdown 列表/标题前缀与两端空白 —— 群里是给人读的一句话，不是原样正文。
    private static func clean(_ s: String) -> String {
        var t = Substring(s.trimmingCharacters(in: .whitespaces))
        while let f = t.first, f == "#" || f == "-" || f == "*" || f == ">" || f == " " {
            t = t.dropFirst()
        }
        return String(t).trimmingCharacters(in: .whitespaces)
    }

    private static func truncate(_ s: String, _ limit: Int) -> String {
        s.count <= limit ? s : String(s.prefix(limit)) + "…"
    }

    // MARK: - 发什么、@ 谁

    struct Post: Equatable {
        let text: String
        /// mention kind 列表，顺序即 @ 顺序；空 = 广播。
        let mentionKinds: [String]
    }

    /// 代发稿。短、结论先行 —— 这条消息的全部作用是让人知道「它停在这儿、在等」。
    ///
    /// **文案是呈现层的地盘（Todo #43），判定层不碰。** 收窄判定后这里只会走 `isQuestion`
    /// 为真的那条分支（`decide` 只在停在问句上时才调本函数），另一条已成死路，留着不动。
    static func post(
        sessionName: String, sessionId: String, isCaptain: Bool, closing: String
    ) -> Post {
        let isQuestion = closing.hasSuffix("？") || closing.hasSuffix("?")
        var lines: [String] = []
        lines.append(isQuestion
            ? "\(sessionName) 这轮停在一个问题上，在等回复："
            : "\(sessionName) 这轮结束了，没往群里说过话，收尾是：")
        lines.append(closing)
        lines.append(isCaptain
            ? "需要人打开这个 session 的终端回它一句。"
            : "机长可 inspect_session 看现场、nudge_session 代答；拍不了板就 @人。")
        lines.append("（session: \(sessionId)）")
        return Post(text: lines.joined(separator: "\n"),
                    mentionKinds: isCaptain ? ["human"] : ["captain"])
    }

    // MARK: - 整轮判定（把上面几块串起来的纯函数，便于单测一把过）

    struct Input {
        let messages: [LocalWhiteboardMessage]
        let sessionId: String
        let sessionName: String
        let isCaptain: Bool
        /// 上一轮结束时记下的白板末条 id。
        let sinceMessageId: String?
        /// 上一轮已代发过的 turn 标识（claude 的 `prompt_id`）—— 同一轮只发一次。
        let lastHandledTurnId: String?
        /// 本轮标识；hook payload 没给就传 nil（退化成「不按 id 去重」—— 一轮结束只回调
        /// 一次，且下一轮要重新停在问句上才会再发，重复的窗口本来就窄）。
        let turnId: String?
        /// 本轮最后一条 assistant 正文。
        let lastAssistantMessage: String
    }

    /// nil = 不发。**只有一种情况发**：本轮收尾停在一个问句上，且它这一轮说过的话没有
    /// @ 到任何能答的人。
    ///
    /// 顺序即判据，三条各自的道理：
    /// 1. 同轮去重 —— hook 重放 / 一轮多次回调不该刷第二条。
    /// 2. 不是停在问句上 → 不发。**静默是正常状态**：机长在等 worker 干活、worker 收到
    ///    一句不需要回的「收到」，这一轮无话可说是对的，不是异常。这条判据与
    ///    `SessionAwaitingReply` 的 `.question` **同源**（同一个 `trailingQuestion`）——
    ///    界面标红和群里代发必须是同一件事的两个出口。谁要把它放宽回「这轮没说话就发」，
    ///    就是在把静默播成待办，#25 首版的病根，别再走一遍。
    /// 3. 已经 @ 到机长/人 → 不发，人已经被叫到了，再补一条只是噪音。
    static func decide(_ input: Input) -> Post? {
        if let t = input.turnId, !t.isEmpty, t == input.lastHandledTurnId { return nil }
        guard let question = trailingQuestion(from: input.lastAssistantMessage) else { return nil }
        guard !hasCallOutTrace(in: input.messages, sessionId: input.sessionId,
                               since: input.sinceMessageId) else { return nil }
        return post(sessionName: input.sessionName, sessionId: input.sessionId,
                    isCaptain: input.isCaptain, closing: question)
    }
}

// MARK: - 跨进程记账（marker 落盘）

/// 回合钩子的 per-session 记账：上一轮结束时的白板末条 id + 上一轮已处理的 turn id。
///
/// 与 `WhiteboardCursor` 分开一份文件：那个是**未读注入**游标，被 hook 路与唤醒路
/// 双写并 forward-only 推进；这里是**留痕判定**的边界，语义不同，混用会互相踩。
struct SessionTurnMarker {
    let directory: URL
    let crewId: String
    let sessionId: String

    /// marker 文件路径。对外只用来 stat 指纹（点名快照那一拍的门控），
    /// 读写一律走下面的 `read()` / `write(_:)`。
    var fileURL: URL {
        directory.appendingPathComponent("\(crewId).\(sessionId).turn")
    }
    private var url: URL { fileURL }

    struct State: Codable, Equatable {
        var lastMessageId: String?
        var lastTurnId: String?
        /// 最近一轮最后一条 assistant 正文。session 进程自行退出时，生命周期层用它
        /// 生成统一的「它最后一句话」通知；旧 marker 缺键自动解成 nil。
        var lastAssistantMessage: String? = nil
        /// 上一轮收尾那句问句（层 2）—— 非 nil = 它说完停在一个问题上。app 侧每拍读它
        /// 判「待回复」并点红点（`SessionAwaitingReply`）。**每轮结束都重写**（不是问句
        /// 就写 nil），所以答完下一轮一结束它自己就没了，不会进得去出不来。
        /// 旧 JSON 缺键 → nil，向后兼容。
        var awaitingQuestion: String?
    }

    func read() -> State {
        guard let d = try? Data(contentsOf: url),
              let s = try? JSONDecoder().decode(State.self, from: d) else { return State() }
        return s
    }

    func write(_ s: State) {
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        guard let d = try? JSONEncoder().encode(s) else { return }
        try? d.write(to: url, options: .atomic)
    }

    /// 只熄掉「在等回复」，其余记账原样保留（app 侧 nudge / 发文本进去时调）。
    /// 读-改-写而不是整体覆盖：回合钩子在另一个进程里也写这个文件，不能把它刚记的
    /// 轮边界抹掉。
    func clearAwaitingQuestion() {
        var s = read()
        guard s.awaitingQuestion != nil else { return }
        s.awaitingQuestion = nil
        write(s)
    }
}
