import Foundation

/// 群聊消息**折叠**的纯判定层（人类 Todo #104）。
///
/// 出处是人类的原话：「现在我就是希望总机长和消息折叠这两个先做好，**因为现在消息
/// 太多太乱了**」。**「乱」不是「长」** —— 治的是条数占地，不是单条字数，所以默认
/// 收起、而不是默认展开。
///
/// ## 三条约束（来自 4-1，逐条落在这里）
///
/// 1. **默认收起。** 判定返回非 nil 就意味着渲染层收起它，不需要谁去点。
/// 2. **收起态要能一眼看出这条是什么 —— 摘要不是截断前 N 字。**
///    截出来的头一句常常是「收到，我看看」，正好是最没信息的部分。所以摘要只从
///    **作者自己写下的结论**里取：显式给的 > 粗体 > 标题。**一个都没有就不折**
///    （见 `fold` 的返回 nil 分支）—— 宁可让一堵墙照原样占着地方，也不拿截断冒充摘要。
/// 3. **注入面不跟着折。** 这个类型只被渲染层调用；`HookEmitter` 那条路一个字都不改。
///    有 `CrewMessageFoldTests.test_折叠只活在渲染层_注入面拿到的仍是全文` 钉着。
///
/// ## 为什么是「粗体」而不是新语法
///
/// 因为它**已经是**大家的写法，不用靠谁记得加标记（靠记性的规矩这条线上今天栽过
/// 好几次）。实测全机 47 个群 11272 条消息：超过阈值、真会被折的那批里 **99%**
/// 已经有现成的粗体或标题可以直接当摘要；首段粗体长度中位 26 字、p75 41 字，正好
/// 是一行摘要的尺寸。
enum CrewMessageFold {

    /// 超过这么多行才折。**改这一个数就换档**（人类 Todo #2 在拍 4 / 8 / 15）。
    ///
    /// 选 8 的依据是实测的拐点，不是拍的：
    /// - `> 8` 行 → 折掉 31.6% 的消息，其中 **99.0%** 有现成摘要；全群总行数 −74%。
    /// - `> 4` 行 → 折掉 50.9%，但只有 73.5% 有摘要，剩下的只能摊开，看着不齐。
    /// - `>15` 行 → 只折 18.6%，−58%。
    static let lineThreshold = 8

    /// 摘要最长多少**半角宽**；超了截断加省略号。**这不是「截断前 N 字」** ——
    /// 被截的是一句作者自己写的结论，不是消息开头。
    ///
    /// ## 为什么是宽度不是字符数（2026-09-12 改）
    ///
    /// 原来这里数的是 `Character`。可收起态那一行是**被宽度约束**的，
    /// 而一个汉字占的横向空间约等于两个拉丁字母 —— 于是同一个「60」对中文和英文
    /// 是两个完全不同的长度。实际后果是**这道闸对中文几乎从不生效**：
    /// 60 个汉字远超任何气泡宽度，真正在截的是视图那层 `lineLimit(2)`，
    /// 而模型这边那个「…」根本没机会出现。
    ///
    /// **一道从不生效的闸，和没有这道闸，看起来一模一样。**（同族：`explicitSummary`
    /// 那条从没跑过的「最可靠的一级」。）
    ///
    /// 所以改成按**显示宽度**算：全角/CJK 记 2，其余记 1。60 ≈ 30 个汉字
    /// ≈ 60 个字母 —— 两种文字终于是同一个长度了。
    static let summaryCap = 60

    struct Folded: Equatable {
        /// 收起态显示的那一行（已去掉 markdown 强调标记、换行压平）。
        let summary: String
        /// 这条消息一共多少行。**把它显示在收起条上是故意的**：一堵墙折起来仍
        /// 看得出是墙，免得大家因为「反正折着」写得更长。
        ///
        /// 语义是「整条有多少行」而不是「还剩多少行」—— 收起态只显示摘要那一行，
        /// 正文一行不露，所以「剩下」和「全部」是同一个数，取不含歧义的那个说法。
        let lineCount: Int
    }

    /// 这条该折吗？`nil` = 不折（太短，或没有能当摘要的东西）。
    ///
    /// - Parameter explicitSummary: 发送者显式给的摘要（`post_to_crew` 的 `summary`）。
    ///   给了就用它，最可靠的一级。
    static func fold(_ text: String,
                     threshold: Int = lineThreshold,
                     explicitSummary: String? = nil) -> Folded? {
        let lines = text.components(separatedBy: "\n").count
        guard lines > threshold else { return nil }
        guard let raw = explicitSummary?.trimmingCharacters(in: .whitespacesAndNewlines)
                .nilIfEmpty ?? derivedSummary(text)
        else { return nil }
        return Folded(summary: clip(flatten(raw)), lineCount: lines)
    }

    /// 渲染层这一拍折不折、折起来显示哪一行。
    ///
    /// 它是「作者写的结论」那根管子的**最后一节**：前面几节（工具参数 → 落盘 →
    /// 白板条目 → 气泡模型）各有用例钉着，而这一节原来长在
    /// `CrewFoldableMessageText` 的 `private var` 里 —— **那个 View 不进 test
    /// bundle**，所以谁把 `explicitSummary:` 删掉，前面那些用例照样全绿。
    /// **一条链最后一节没有尺子，等于整条链没有尺子。**
    ///
    /// - Parameter isStreaming: 正在流式吐字的不折 —— 折一个还在长的东西，
    ///   人会以为它写完了。
    static func decideForRender(text: String, headline: String?, isStreaming: Bool) -> Folded? {
        isStreaming ? nil : fold(text, explicitSummary: headline)
    }

    /// 发出去之后回执里该不该提醒一句「这条的标题是猜的」（人类 Todo #143）。
    ///
    /// **schema 里的说明只在写之前被读到一次，而且多半没读。** 真正教得会人的是
    /// **出错那一刻的那句话** —— 所以这条消息够长会被折、作者又没给 `headline` 时，
    /// 回执里把**猜出来的那一行原样摆给他看**：他一眼就看出那不是他的结论。
    ///
    /// - Returns: 提醒文案；不该提醒（太短不折 / 已经给了 / 猜都猜不出来）时 nil。
    static func receiptHintIfGuessed(text: String, headline: String?) -> String? {
        guard (headline?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
        else { return nil }
        guard let folded = fold(text) else { return nil }
        return "⚠️ 这条 \(folded.lineCount) 行，群里会**默认收起**，而你没给 `headline` ——"
            + "收起态那一行现在是**猜**的，猜出来是：「\(folded.summary)」。"
            + "\n不是你要的结论就下次带上 `headline`（一句话说清结果是什么）。"
    }

    // MARK: - 摘要推导

    /// 从正文里找一句作者自己写下的结论：**先粗体、后标题**，且只在**前 3 段**里找。
    ///
    /// 限前 3 段是量出来的：最近 3 天的长消息里，第一个粗体落在第 0/1/2 段的分别是
    /// 327 / 90 / 4 条 —— 再往后找就会捞到正文中段某个强调词，那不是摘要。
    static func derivedSummary(_ text: String) -> String? {
        let head = paragraphs(withoutFences: text).prefix(3).joined(separator: "\n")
        if let bold = firstBold(in: head) { return bold }
        if let heading = firstHeading(in: head) { return heading }
        return nil
    }

    /// 切段，并且**把围栏代码块整段丢掉** —— 代码里的 `**` 不是强调，日志里的
    /// `# 注释` 也不是标题。不丢的话，一条贴了脚本的消息会拿脚本里的字当摘要。
    private static func paragraphs(withoutFences text: String) -> [String] {
        var kept: [String] = []
        var inFence = false
        for line in text.components(separatedBy: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            if t.hasPrefix("```") || t.hasPrefix("~~~") { inFence.toggle(); continue }
            kept.append(inFence ? "" : line)
        }
        return kept.joined(separator: "\n")
            .components(separatedBy: "\n\n")
            .map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }
            .filter { !$0.isEmpty }
    }

    private static func firstBold(in s: String) -> String? {
        // 非贪婪配对 `**…**`，允许跨行（我们的消息里粗体经常跨一行）。
        guard let open = s.range(of: "**"),
              let close = s.range(of: "**", range: open.upperBound..<s.endIndex)
        else { return nil }
        return String(s[open.upperBound..<close.lowerBound])
            .trimmingCharacters(in: .whitespacesAndNewlines).nilIfEmpty
    }

    private static func firstHeading(in s: String) -> String? {
        for line in s.components(separatedBy: "\n") {
            let t = line.trimmingCharacters(in: .whitespaces)
            guard t.hasPrefix("#") else { continue }
            let body = t.drop(while: { $0 == "#" })
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if let v = body.nilIfEmpty { return v }
        }
        return nil
    }

    // MARK: - 收拾成一行

    /// 压平成单行，并去掉摘要自身里的 markdown 强调/行内代码标记 —— 收起条是一行
    /// 纯文本，留着 `**` 只会看见星号。
    private static func flatten(_ s: String) -> String {
        var out = s.replacingOccurrences(of: "\n", with: " ")
        for token in ["**", "__", "`", "*", "_"] {
            out = out.replacingOccurrences(of: token, with: "")
        }
        return out.split(separator: " ").joined(separator: " ")
    }

    /// 一个字符占几个「半角宽」。
    ///
    /// 判据取 Unicode 的东亚宽度性质：**全角（F）/ 宽（W）记 2，其余记 1**。
    /// 这不是精确排版（真实宽度还看字体和字距），是一个**比字符数近得多的近似** ——
    /// 目的只是让「60」对中英文意味着差不多长的一行。
    static func displayWidth(_ c: Character) -> Int {
        for scalar in c.unicodeScalars {
            // 绘文字用 Unicode 自己的性质判，别手搓范围表 —— 手搓的那张表
            // 第一版就漏了 ✅（U+2705 在 Dingbats 里，不在任何 CJK 段）。
            if scalar.properties.isEmojiPresentation { return 2 }
            switch scalar.value {
            // CJK 统一表意文字（含扩展 A）、兼容表意文字
            case 0x1100...0x115F,           // 韩文字母 Jamo
                 0x2E80...0x303E,           // CJK 部首补充 · 康熙部首 · CJK 符号与标点
                 0x3041...0x33FF,           // 假名 · 韩文兼容字母 · CJK 方块
                 0x3400...0x4DBF,           // 扩展 A
                 0x4E00...0x9FFF,           // 基本区
                 0xA000...0xA4CF,           // 彝文
                 0xAC00...0xD7A3,           // 韩文音节
                 0xF900...0xFAFF,           // 兼容表意
                 0xFE10...0xFE19,           // 竖排标点
                 0xFE30...0xFE6F,           // CJK 兼容形式
                 0xFF00...0xFF60,           // 全角 ASCII 与标点
                 0xFFE0...0xFFE6,           // 全角符号
                 0x20000...0x3FFFD:         // 扩展 B 及以后
                return 2
            default:
                continue
            }
        }
        return 1
    }

    /// 这一串有多宽（半角为 1）。
    static func displayWidth(_ s: String) -> Int {
        s.reduce(0) { $0 + displayWidth($1) }
    }

    /// 按显示宽度截断。**截在字符边界上** —— 不许把一个字切一半，
    /// 所以中文串的实际宽度可能比 cap 少 1（宁可短一格，不许出半个字）。
    private static func clip(_ s: String) -> String {
        guard displayWidth(s) > summaryCap else { return s }
        var out = ""
        var width = 0
        for c in s {
            let w = displayWidth(c)
            if width + w > summaryCap { break }
            out.append(c)
            width += w
        }
        return out + "…"
    }
}

private extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}
