#if os(macOS)
import Foundation
import TOMLKit

/// 「这个工作目录，claude / codex 信任过没有」—— **只读**的检测，加**唯一一份**提示文案。
///
/// ## 为什么只读
///
/// `~/.claude.json` 的 `hasTrustDialogAccepted`、`~/.codex/config.toml` 的 `trust_level`，
/// 记的是**人对某一个路径点过的那一下头**。信任的单位是路径 —— 那是 claude / codex
/// 两家定的规矩，不是我们定的。人对 `/a` 点的头，我们复制到 `/b`，`/b` 那份授权就是
/// 我们签的，不是他签的：「搬」这个动词听起来像守恒，其实凭空多了一份。
///
/// 所以产品这一侧**一个信任键都不写**：只读，读到没信任就把该跑的命令原样交给人。
/// 这不是「还没做」，是选的 —— 迁移那条路同样不写（见
/// `WorkdirMigrationPlan.neverWrittenClaudeKeys`）。
///
/// ## 文案里不许出现的东西
///
/// 那个信任框长什么样（有没有编号、选项什么顺序、默认高亮在哪）**由上游说了算，
/// 而且改过** —— 2026 年 8 月底到 9 月初的 11 天里就整个换过一版。任何写死一版形状的
/// 指导语，都会在某天变成一句会让人按错的假话。所以这里只说「去终端跑这条命令，
/// 按它当场给出的提示信任这个目录」，一个字都不描述那个框。
///
/// 命令本身**从真实路径算出来**（`command(for:workdir:)`），不拼死字符串。
///
/// ## 只有一份
///
/// 新建 crew（`CreateCrewSheet`）和迁移完成（`ChangeWorkingDirectorySheet`）弹的是
/// **同一个东西**，不是两份长得像的文案 —— 写两遍的那天，两份会各自漂，而漂了没有
/// 任何读数会报警。
enum WorkdirTrustPrompt {

    /// `~/.claude.json` 里记这件事的键。**只读它，从不写它。**
    static let claudeTrustKey = "hasTrustDialogAccepted"
    /// `~/.codex/config.toml` 的 `trust_level` 取这个值才算信任过。
    static let codexTrustedLevel = "trusted"

    // MARK: - 检测（只读）

    /// 一次读到的现状。两项都来自真文件，读不到就是「没信任」（保守方向：宁可多提示一次）。
    struct State: Equatable {
        var claudeTrusted: Bool = false
        var codexTrustLevel: String?

        init(claudeTrusted: Bool = false, codexTrustLevel: String? = nil) {
            self.claudeTrusted = claudeTrusted
            self.codexTrustLevel = codexTrustLevel
        }

        var codexTrusted: Bool { codexTrustLevel == WorkdirTrustPrompt.codexTrustedLevel }

        func isTrusted(_ kind: LocalCodingAgentKind) -> Bool {
            switch kind {
            case .claudeCode: return claudeTrusted
            case .codex: return codexTrusted
            case .terminal: return true // 普通终端没有目录信任这回事。
            }
        }
    }

    /// 读真文件。**全程只读**：打不开 / 解不开都当「没信任」，不写、不建、不修。
    static func read(workdir: String, home: URL) -> State {
        let path = WorkdirMigrationPlan.normalize(workdir)
        guard !path.isEmpty else { return State() }
        return State(claudeTrusted: readClaudeTrusted(path: path, home: home),
                     codexTrustLevel: readCodexTrustLevel(path: path, home: home))
    }

    /// `~/.claude.json` → `projects["<绝对路径>"].hasTrustDialogAccepted`。
    static func readClaudeTrusted(path: String, home: URL) -> Bool {
        let url = home.appendingPathComponent(".claude.json")
        guard let data = try? Data(contentsOf: url),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let entry = (root["projects"] as? [String: Any])?[path] as? [String: Any]
        else { return false }
        // 踩坑现场的形状是「条目在、值是 `false`」，不是「条目缺失」—— 用迁移那边
        // 同一把「有实质值」的尺子，`false` 进不来。
        return WorkdirMigrationPlan.isMeaningful(entry[claudeTrustKey])
    }

    /// `~/.codex/config.toml` → `[projects."<绝对路径>"] trust_level`（没有 → nil）。
    static func readCodexTrustLevel(path: String, home: URL) -> String? {
        let url = home.appendingPathComponent(".codex/config.toml")
        guard let text = try? String(contentsOf: url, encoding: .utf8),
              let table = try? TOMLTable(string: text),
              let projects = table["projects"]?.table
        else { return nil }
        return projects[path]?.table?["trust_level"]?.string
    }

    // MARK: - 提示

    /// 要提示的那一份。`runners` 非空是它存在的前提 —— 没有要说的就没有这个类型。
    struct Prompt: Equatable, Identifiable {
        /// 归一后的绝对路径（命令里出现的就是它）。
        var path: String
        /// 还没信任这个目录的 runner，按 `LocalCodingAgentKind.allCases` 的顺序。
        var runners: [LocalCodingAgentKind]

        var id: String { path + "|" + runners.map(\.rawValue).joined(separator: ",") }

        /// 每个 runner 一条，已经带上真实路径，复制粘贴进 Terminal 就能跑。
        var commands: [String] {
            runners.map { WorkdirTrustPrompt.command(for: $0, workdir: path) }
        }

        /// 整段复制用的那一块。
        var commandBlock: String { commands.joined(separator: "\n") }
    }

    /// 未信任的 runner → 一份提示；全都信任过（或都没装）→ nil，不打扰人。
    ///
    /// - Parameter installed: 本机真装了的 agent。没装的不提示 —— 让人去跑一个他机器上
    ///   没有的命令，比不提示更糟。
    static func prompt(workdir: String,
                       state: State,
                       installed: [LocalCodingAgentKind]) -> Prompt? {
        let path = WorkdirMigrationPlan.normalize(workdir)
        guard !path.isEmpty else { return nil }
        let untrusted = LocalCodingAgentKind.allCases.filter {
            $0.isAgent && installed.contains($0) && !state.isTrusted($0)
        }
        guard !untrusted.isEmpty else { return nil }
        return Prompt(path: path, runners: untrusted)
    }

    /// 读真文件那条路（生产用）。
    static func prompt(workdir: String,
                       home: URL,
                       installed: [LocalCodingAgentKind]
                           = LocalCodingAgentExecutable.discoverAvailable()) -> Prompt? {
        prompt(workdir: workdir,
               state: read(workdir: workdir, home: home),
               installed: installed)
    }

    /// 让人去终端跑的那一条。**从真实路径算出来**，不是拼死的字符串。
    static func command(for kind: LocalCodingAgentKind, workdir: String) -> String {
        "cd \(shellQuoted(workdir)) && \(kind.binaryName)"
    }

    /// POSIX shell 单引号转义 —— 路径里真的会有空格，也可能有 `'`
    /// （`~/CrewGround/…` 是我们造的，但人自己选的目录什么都可能有）。
    static func shellQuoted(_ raw: String) -> String {
        "'" + raw.replacingOccurrences(of: "'", with: #"'\''"#) + "'"
    }

    // MARK: - 文案（一份，两处共用）

    static let title = "这个目录还没被信任"

    /// 这份提示的**全部句子**。对话框和群聊消息都从这里取 —— 只有一份，
    /// 两处的差别只剩「命令块要不要包代码围栏」。
    ///
    /// 曾经的病正是这个形状：同一句话散在守则、当场提示、渲染三处，改了两处漏了一处，
    /// 而漂了没有任何读数会报警。
    static func sentences(_ p: Prompt) -> [String] {
        let names = p.runners.map(\.displayName).joined(separator: " 和 ")
        return [
            "\(names) 还没信任过这个目录：\(p.path)",
            "没被信任的目录下，它们起来会先停在自己的确认上等人 —— 进程活着、不吐一个字，"
                + "外面看着就像一直空闲。",
            "去 Terminal 里把下面\(p.commands.count > 1 ? "每条各" : "这条")跑一次，"
                + "按它当场给出的提示信任这个目录，然后退出就行：",
            "这条记录我们不替你写 —— 那是你对这个目录的授权，得由你自己点。",
        ]
    }

    /// 对话框正文（纯文本）。
    static func body(_ p: Prompt) -> String {
        let s = sentences(p)
        return (s.dropLast() + [p.commandBlock] + s.suffix(1)).joined(separator: "\n\n")
    }

    /// 发进群聊的那条（同一批句子，命令块包成代码块好复制）。
    static func chatMessage(_ p: Prompt) -> String {
        let s = sentences(p)
        let block = "```sh\n" + p.commandBlock + "\n```"
        return (["**" + title + "**"] + s.dropLast() + [block] + s.suffix(1))
            .joined(separator: "\n\n")
    }
}
#endif
