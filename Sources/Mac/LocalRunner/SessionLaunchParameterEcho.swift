#if os(macOS)
import Foundation

/// 启动参数**没被 CLI 接受**的首屏回显判定（人类 Todo #36 的核心那半）。
///
/// # 病根
///
/// `start_session` 传的 model / effort 是纯字符串透传给 CLI 的。传错时的实际表现
/// （2026-08-09 PTY 实测，claude 2.1.226）**不是崩、不是退出**：
///
/// - 坏 model → 首屏打
///   `"totally-bogus-model" is not a model this version of Claude Code recognizes, so
///   auto-compact will keep this session within 200k tokens …`，然后**照常跑下去**。
/// - 坏 effort → 首屏打
///   `Warning: Unknown --effort value 'bogus-effort' — ignoring it and using the default
///   effort. Valid values: low, medium, high, xhigh, max.`
///   —— CLI 明说了「忽略你的值、用默认档」，**这就是人类说的静默降级**。
///
/// 两种情况进程都活得好好的、也在吐字，所以 `SessionLaunchProbe` 判 `.alive`，
/// 整条链路一个字都不会报。人只能从「它自报的型号不对 / 跑得不像那个档」反推回
/// 「当初 model 名写错了」—— 这正是 Todo #36 要消灭的体验。
///
/// # 判据为什么可靠（不是瞎猜短语）
///
/// 除了匹配短语，还**要求 CLI 引用的那个值 == 我们实际传出去的值**：两条消息都会把
/// 出问题的值原样引出来（`"X" is not a model…` / `Unknown --effort value 'X'`）。
/// 于是 agent 自己在终端里聊到这些字眼（比如正在读这段注释）不会触发误报。
/// 再叠一层时间窗（只在拉起后 `window` 秒内扫），把长回合里的偶然重现也挡掉。
///
/// # 边界（**不是**「这个值非法」的证据）
///
/// `is not a model this version of Claude Code recognizes` 说的是**本地二进制不认识**，
/// 用来决定 auto-compact 的上下文窗口 —— 一个刚发布、CLI 还没跟上的模型也会触发它，
/// 而它在 API 侧完全可用。所以这条回执只说「CLI 不认识你填的值、可能没按你想的生效」，
/// **不下「非法」的结论、不拦、不改写**（与 `AgentModelCheck` 同一条纪律）。
enum SessionLaunchParameterProblem: Equatable {
    /// CLI 不认识这个 model（可能是打错，也可能是 CLI 版本还没跟上的新模型）。
    case modelUnrecognized(value: String, quote: String)
    /// CLI 明确忽略了这个 effort、改用默认档 —— 确定性的静默降级。
    case effortIgnored(value: String, quote: String)

    /// 白板 fail-loud 的正文（人话 + 下一步动作）。
    var detail: String {
        switch self {
        case let .modelUnrecognized(value, quote):
            return "起 session 时传的 model =「\(value)」**这个版本的 CLI 不认识**，"
                + "它照常起来了但很可能没按你要的模型跑。CLI 原话：\(quote)"
                + "\n（也可能是 CLI 版本还没跟上的新模型 —— 不代表非法，但请核实一下。）"
        case let .effortIgnored(value, quote):
            return "起 session 时传的 effort =「\(value)」**被 CLI 忽略了，已静默降级成默认档**。"
                + "CLI 原话：\(quote)"
                + "\n（claude 的启动参数 `--effort` 与运行时 `/effort` 认的不是一套 —— "
                + "`auto` 这类只能跑起来之后用 set_session_profile 切。）"
        }
    }

    /// 去重键（同一类只报一次）。
    var knobKey: String {
        switch self {
        case .modelUnrecognized: return "model"
        case .effortIgnored: return "effort"
        }
    }
}

/// 首屏回显里找「参数没被接受」。纯逻辑（喂字符串即可测），PTY 那半在
/// `SessionLaunchParameterScanner`。
enum SessionLaunchParameterVerdict {
    /// 短语来自本机 claude 2.1.226 的真实输出（见类型头部）。去空白+小写后匹配 ——
    /// claude 的 TUI 排版靠光标移动，词间空格可能根本没进字节流（同
    /// `SessionProfileEchoVerdict.squeeze` 的理由）。
    static let modelPhrase = "isnotamodelthisversionofclaudecoderecognizes"
    static let effortPhrase = "unknown--effortvalue"

    /// 出问题的值出现在短语的哪一侧。
    ///
    /// **为什么必须钉死位置、不能只判「附近有没有出现」**：坏 effort 那句警告
    /// 自带 `Valid values: low, medium, high, xhigh, max.` —— 如果只看「值在不在
    /// 这段窗口里」，那么正常传 `high` 的 session 一旦屏幕上因为别的原因出现这句
    /// 警告，就会被误报成「你的 high 被忽略了」。值必须紧贴在 CLI 引用它的那个位置上。
    private enum ValueSide { case before, after }

    /// - Parameters:
    ///   - plain: 去 ANSI 后的首屏文本。
    ///   - model / effort: **我们实际传出去的**值（nil = 没传，那一类不检）。
    /// - Returns: 命中的问题（可能两条都有）。
    static func classify(_ plain: String, model: String?, effort: String?)
        -> [SessionLaunchParameterProblem] {
        let (text, origin) = SessionProfileEchoVerdict.squeeze(plain)
        var out: [SessionLaunchParameterProblem] = []
        // model：`"X" is not a model this version…` —— 值在短语**前**。
        if let model, !model.isEmpty,
           let quote = excerpt(text, origin, in: plain, phrase: modelPhrase,
                               value: model, side: .before) {
            out.append(.modelUnrecognized(value: model, quote: quote))
        }
        // effort：`Unknown --effort value 'X' — ignoring it…` —— 值在短语**后**。
        if let effort, !effort.isEmpty,
           let quote = excerpt(text, origin, in: plain, phrase: effortPhrase,
                               value: effort, side: .after) {
            out.append(.effortIgnored(value: effort, quote: quote))
        }
        return out
    }

    /// 命中短语、**且我们传的那个值紧贴在 CLI 引用它的位置上** → 回一段原文片段；
    /// 否则 nil。那道位置闸门是防误报的主力（见 `ValueSide`）。
    private static func excerpt(
        _ text: String, _ origin: [String.Index], in plain: String,
        phrase: String, value: String, side: ValueSide
    ) -> String? {
        guard let r = text.range(of: phrase) else { return nil }
        let hit = text.distance(from: text.startIndex, to: r.lowerBound)
        let end = text.distance(from: text.startIndex, to: r.upperBound)
        let (squeezedValue, _) = SessionProfileEchoVerdict.squeeze(value)
        guard !squeezedValue.isEmpty else { return nil }
        let quoteChars = CharacterSet(charactersIn: "\"'`“”‘’")

        switch side {
        case .before:
            // 短语之前那一小段，剥掉尾部引号后应当正好以我们的值收尾。
            let head = String(text.prefix(hit))
            let trimmed = head.trimmingCharacters(in: quoteChars)
            guard trimmed.hasSuffix(squeezedValue) else { return nil }
        case .after:
            // 短语之后那一小段，剥掉头部引号后应当正好以我们的值开头。
            let tail = String(text.dropFirst(end))
            var i = tail.startIndex
            while i < tail.endIndex, tail[i].unicodeScalars.allSatisfy(quoteChars.contains) {
                i = tail.index(after: i)
            }
            guard tail[i...].hasPrefix(squeezedValue) else { return nil }
        }

        // 判定过了才去取给人看的原文片段（前后各留一段，覆盖两种摆放）。
        let from = max(0, hit - 120)
        let to = min(origin.count, hit + 200)
        guard from < to else { return nil }
        let window = String(plain[origin[from]...origin[to - 1]])
        return window.split(whereSeparator: \.isNewline)
            .map { $0.trimmingCharacters(in: .whitespaces) }
            .filter { !$0.isEmpty }
            .joined(separator: " ")
            .prefix(240)
            .trimmingCharacters(in: .whitespaces)
    }
}

/// 拉起窗口内的启动参数扫描器：喂 PTY 原始字节，命中即回（每类只回一次）。
///
/// 只在拉起后 `window` 秒内活着 —— 之后 `isExpired` 为真，调用方把它丢掉，
/// 不再为长回合里偶然出现的同款字眼付出误报代价。
final class SessionLaunchParameterScanner {
    /// 扫多久。claude 这两条都在**第一屏**（进程起来一两秒内）就打完了，
    /// 45s 是给冷启动/大仓库索引留的余量。
    static let window: TimeInterval = 45

    private let stripper = AnsiPlainTextTail(tailLimit: 8192)
    private let model: String?
    private let effort: String?
    private let startedAt: Date
    private var reported: Set<String> = []

    /// `model` / `effort` 传 nil 表示这次拉起没显式指定它 —— 那一类不检
    /// （没传就谈不上「传错」）。两个都 nil 时 `isIdle` 为真，调用方可以根本不建。
    init(model: String?, effort: String?, startedAt: Date = Date()) {
        self.model = model.flatMap { $0.isEmpty ? nil : $0 }
        self.effort = effort.flatMap { $0.isEmpty ? nil : $0 }
        self.startedAt = startedAt
    }

    /// 两个旋钮都没显式传 → 没什么可检的。
    var isIdle: Bool { model == nil && effort == nil }

    func isExpired(now: Date = Date()) -> Bool {
        now.timeIntervalSince(startedAt) > Self.window
    }

    /// 喂一笔 PTY 字节，回本次**新**命中的问题（已报过的不再回）。
    @discardableResult
    func feed(_ bytes: ArraySlice<UInt8>, now: Date = Date()) -> [SessionLaunchParameterProblem] {
        guard !isIdle, !isExpired(now: now), reported.count < 2 else { return [] }
        guard stripper.feed(bytes) else { return [] }
        let hits = SessionLaunchParameterVerdict.classify(
            stripper.tail, model: model, effort: effort)
        var fresh: [SessionLaunchParameterProblem] = []
        for hit in hits where !reported.contains(hit.knobKey) {
            reported.insert(hit.knobKey)
            fresh.append(hit)
        }
        return fresh
    }
}
#endif
