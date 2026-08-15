#if os(macOS)
import Foundation

/// session 中途切换配置的一个档位。落到 claude PTY 就是一条斜杠命令。
enum SessionProfileKnob: String, Equatable {
    case model
    case effort

    /// 人话名（白板回执用）。
    var label: String { self == .model ? "模型" : "effort" }
}

/// 一次具体的切换动作（一个档位一条命令 —— model 与 effort 分两笔发、各自核对）。
struct SessionProfileSwitchCommand: Equatable {
    let knob: SessionProfileKnob
    let value: String

    /// 注入终端的整行（等价于人在 claude 里手打 `/model opus`）。
    var line: String { "/\(knob.rawValue) \(value)" }
    /// 回执里的短描述，如「模型→opus」。
    var summary: String { "\(knob.label)→\(value)" }
}

/// 切换结果。**只有 `.applied` 等于「真的换了」** —— 其余一律如实回执，
/// 绝不把「已提交」说成「已生效」（#544 的教训：假成功回执让机长以为切好了，
/// 继续用旧模型跑到撞额度上限）。
enum SessionProfileSwitchOutcome: Equatable {
    /// claude 回显确认生效（带回显原文片段）。
    case applied(String)
    /// claude 明确拒绝（未知模型 / effort 被 pin 或组织策略挡住）。
    case rejected(String)
    /// 注入了，但超时窗内没等到任何确认回显 —— 不能算成功。
    case noConfirmation
    /// 等不到可注入的空闲窗口（session 一直忙，或已退出）。
    case neverIdle
    /// 该 runner 没有中途切换通道（codex：model/effort 绑定 app-server thread 配置）。
    case unsupported

    var isApplied: Bool { if case .applied = self { return true }; return false }
}

/// 等空闲 / 注入 / 核对回显 的时间参数。默认值按实测调（见 `SessionProfileEchoVerdict`）。
struct SessionProfileSwitchPolicy {
    /// 连续这么久没有 PTY 输出才算「终端空闲、可以打斜杠命令」。claude 干活时
    /// TUI 每秒重绘计时器 → 有输出；2.5s 静默给足余量。
    var idleQuiet: TimeInterval = 2.5
    /// 最长等空闲多久。session 自己在回合中调本工具是常态，一轮活可能很久。
    var idleWait: TimeInterval = 900
    /// 注入后等回显多久。本地命令，通常 < 1s。
    var confirmWait: TimeInterval = 8
    /// 没等到回显时重试几次（撞上「刚好又忙起来」的窗口）。
    var attempts: Int = 3
    /// 轮询间隔。
    var poll: TimeInterval = 0.25
}

/// claude 斜杠命令回显的判定。
///
/// **为什么要去空白再比**：claude TUI 排版靠光标移动而不是空格 —— 实测
/// （claude 2.1.220，PTY 里注入 `/model haiku`）去 ANSI 后拿到的是
/// `⎿  SetmodeltoHaiku 4.5andsavedasyourdefaultfornewsessions`，
/// 词间空格根本没进字节流。所以匹配前把**两边**的空白全抹掉。
///
/// 短语全部来自本机 claude 2.1.220 二进制 strings 实测，非臆测。
enum SessionProfileEchoVerdict {

    /// 生效回显（去空白、小写后的子串）。
    static let appliedPhrases: [SessionProfileKnob: [String]] = [
        .model: ["setmodelto", "resetmodeltotheworkspacedefault"],
        .effort: ["seteffortlevelto", "effortlevelsettoauto"],
    ]

    /// 明确失败回显。注意 `notapplied:` 覆盖 claude 的两条 effort 拒绝
    /// （`CLAUDE_CODE_EFFORT_LEVEL` 覆盖 / launch-effort pin 顶住）。
    static let rejectedPhrases: [SessionProfileKnob: [String]] = [
        .model: ["unknownmodel", "failedtovalidatemodel", "keptmodelas"],
        .effort: ["failedtoseteffortlevel", "notapplied:", "invalidargument:",
                  "exceedsyourorganization'slimit"],
    ]

    /// 去掉全部空白 + 小写；同时留一张「新下标 → 原字符下标」的映射，
    /// 命中后能从**原文**截可读片段（回执里引用 claude 的原话，不引用压扁版）。
    static func squeeze(_ s: String) -> (text: String, origin: [String.Index]) {
        var text = ""
        var origin: [String.Index] = []
        var i = s.startIndex
        while i < s.endIndex {
            let c = s[i]
            if !c.isWhitespace {
                text.append(contentsOf: c.lowercased())
                // 小写化可能把 1 个字符变成多个（土耳其语等）——每个都指回同一原位。
                while origin.count < text.count { origin.append(i) }
            }
            i = s.index(after: i)
        }
        return (text, origin)
    }

    /// 判定一段去 ANSI 明文里有没有该档位的生效/失败回显。nil = 还没有结论。
    /// 失败优先（同窗里两者都出现时，宁可报失败也不谎报成功）。
    static func classify(_ plain: String, knob: SessionProfileKnob) -> SessionProfileSwitchOutcome? {
        let (text, origin) = squeeze(plain)
        if let quote = firstMatch(text, origin, in: plain,
                                  phrases: rejectedPhrases[knob] ?? []) {
            return .rejected(quote)
        }
        if let quote = firstMatch(text, origin, in: plain,
                                  phrases: appliedPhrases[knob] ?? []) {
            return .applied(quote)
        }
        return nil
    }

    /// 命中则回一段原文片段（从命中处起 ~120 字，单行化）。
    private static func firstMatch(
        _ text: String, _ origin: [String.Index], in plain: String, phrases: [String]
    ) -> String? {
        for phrase in phrases {
            guard let r = text.range(of: phrase) else { continue }
            let start = origin[text.distance(from: text.startIndex, to: r.lowerBound)]
            let excerpt = plain[start...].prefix(120)
                .split(whereSeparator: \.isNewline).first.map(String.init) ?? phrase
            return excerpt.trimmingCharacters(in: .whitespaces)
        }
        return nil
    }
}

/// 一次切换的回显扫描器：喂 PTY 原始字节，攒到结论就锁住（`outcome` 非 nil 后不再变）。
/// 生命周期只覆盖「注入 → 等回显」那几秒，用完即弃 —— 所以短语可以比健康扫描器宽松。
final class SessionProfileEchoScanner {
    private let stripper = AnsiPlainTextTail()
    private let knob: SessionProfileKnob
    private(set) var outcome: SessionProfileSwitchOutcome?

    init(knob: SessionProfileKnob) { self.knob = knob }

    @discardableResult
    func feed(_ bytes: ArraySlice<UInt8>) -> SessionProfileSwitchOutcome? {
        guard outcome == nil, stripper.feed(bytes) else { return outcome }
        outcome = SessionProfileEchoVerdict.classify(stripper.tail, knob: knob)
        return outcome
    }
}
#endif
