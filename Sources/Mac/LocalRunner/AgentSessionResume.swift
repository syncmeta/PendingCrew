#if os(macOS)
import Foundation

/// 「session 关掉再点恢复，agent 自己的对话上下文也接上」的判定层（人类 Todo #28 / #68）。
///
/// 背景：`CrewSessionRunner.restartMember` 一直复用原 localSessionId（身份/成员登记/
/// 白板游标延续），但从不带 agent 侧会话号，所以 agent 每次重启都是新脑子。两条续接
/// 通道本来就有（claude `--resume <id>` / codex `thread/resume`），缺的只是把会话号记
/// 下来 + 重启时决定要不要用它。
///
/// claude 侧的干净做法：**会话号由我们指定**（`claude --session-id <uuid>`，已在
/// claude 2.1.226 实测：指定后 `--resume <同一个 uuid>` 能复述上一轮内容，且续跑写回
/// 同一个 `.jsonl`、id 不变），所以不用去猜 `~/.claude` 里哪个日志是它的。
///
/// ## Todo #68：不许再预测「claude 能不能续」（2026-08-26 实测，claude 2.1.246）
///
/// 这一层原先有一道门：按**工作目录**反推日志路径（`~/.claude/projects/<slug>/<id>.jsonl`），
/// 文件不在就判「续不上」。**那道门量的东西 claude 从来没在用。** 三组实验：
///
/// 1. 把日志文件挪到一个跟任何真实路径都对不上的目录（`…/projects/-tmp-无关-slug/`），
///    再换到第三个目录 `--resume <同一个 id>` → **接上了**，而且继续追加写进那个不相干
///    目录里的原文件；第三个目录的 slug 下一个 jsonl 都没生成。
/// 2. 把同一个文件挪到 `~/.claude/projects` **树外** → 当场报
///    `No conversation found with session ID: <id>`，exit 1，不静默新开、不留下文件。
/// 3. 官方 `--help` 在两者之间划了同一条界：`--continue` 写明 *in the current
///    directory*，`--resume` 一个字都没提目录。
///
/// 按旧判据，本机 339 条 claude 记录里有 69 条（20%，开过 isolation worktree 的成员 +
/// crew 搬过家的成员）被判成「续不上」而新开一轮 —— **那 69 条 claude 本来全都续得回来。**
///
/// > **能力判断要问那个有能力的系统。** 我们拿「文件在不在我们以为的位置」代替了
/// > 「claude 能不能续上」—— 而只有 claude 能回答后者。**代理量替代真观测**，这次的
/// > 特别之处是：**我们不只是量错了，我们量了一个 claude 从来没在用的东西。**
///
/// 所以现在的形状是 **不预判、真去试**（与 codex 那侧 `thread/resume` 失败→降级
/// `thread/start` 完全同形）：记了会话号就直接带 `--resume` 起；claude 自己拒了，
/// 再不带 `--resume` 重起一次，并把 **claude 的原话**如实带进白板与首轮 brief。
/// `locateClaudeTranscript` 保留，但**只作诊断**（降级之后多说一句「盘上有/没有」），
/// 不参与决策 —— 决定权归 claude。
///
/// 纯 Foundation + 可注入，判定层不碰文件系统（真去翻盘的那半收在 `diskLookup` 里，
/// 同样可替换），供单测直接跑。
enum AgentSessionResume {
    /// 重启一个既有成员时，agent 侧该怎么起。
    enum Decision: Equatable {
        /// 带着这个会话号续跑（claude `--resume` / codex `thread/resume`）。
        case resume(id: String)
        /// 接不回来 → 老老实实新起一轮，并把原因如实说出去（不装死）。
        case fresh(reason: FreshReason)
    }

    /// 为什么续不上 —— 每一种都要能对人说清楚，**且都必须有真凭据**。
    enum FreshReason: Equatable {
        /// 压根没记过会话号（这个成员是本特性上线前起的）。**这条我们确实知道** ——
        /// 账本是我们自己写的，查得准。
        case noRecord
        /// 带着会话号去起了，**agent 自己拒了**。`agentSaid` 是它的原话
        /// （claude：`No conversation found with session ID: <id>`）。
        ///
        /// 旧实现这里是 `transcriptMissing`，说的是**我们那道门的判断**、文案还写成
        /// 「原会话在本机已找不到」—— 那句话把下一个来查的人直接带去查「日志为什么被
        /// 删了」，而真相是我们找错了地方。现在这条只在 agent 真的拒了之后才产生。
        case agentRejectedResume(id: String, agentSaid: String, diagnosis: String?)
    }

    /// 决定重启时带不带会话号。**只看账本记没记**——「能不能续」不归我们判（见类型注释）。
    static func decide(recordedId: String?) -> Decision {
        guard let id = recordedId, !id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            return .fresh(reason: .noRecord)
        }
        return .resume(id: id.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// 我们自己指定给一个新 claude session 的会话号（小写 uuid，claude `--session-id` 要求）。
    static func newClaudeSessionId(_ uuid: UUID = UUID()) -> String {
        uuid.uuidString.lowercased()
    }

    // MARK: - claude 拒绝续跑的判据（吃它的原话，不自己造判断）

    /// claude 拒绝 `--resume` 时打在终端上的那句话的前缀（2.1.246 实测原文：
    /// `No conversation found with session ID: <id>`）。
    static let claudeNoConversationMarker = "No conversation found with session ID"

    /// 从终端画面里认出「claude 拒了这个会话号」。认出来 → 返回它那句原话（整行，
    /// 去掉首尾空白），认不出 → nil。
    ///
    /// **判据只认这一句 + 这个 id**，别放宽：CLI 没装、参数写错、额度用尽也会让进程
    /// 秒退，那些是真故障，吞掉它们去「降级重起」只会把故障藏起来（这也是为什么不拿
    /// 「退出码非零」单独当判据）。
    static func claudeResumeRejection(inScreenText text: String, resumedId: String) -> String? {
        let id = resumedId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else { return nil }
        for raw in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let line = raw.trimmingCharacters(in: .whitespacesAndNewlines)
            guard line.contains(claudeNoConversationMarker), line.contains(id) else { continue }
            return line
        }
        return nil
    }

    // MARK: - 找日志（只作诊断，不作决策）

    /// claude 的会话日志根目录。
    static func claudeProjectsDirectory(home: URL) -> URL {
        home.appendingPathComponent(".claude/projects", isDirectory: true)
    }

    /// 翻盘的两个探针 —— 判定层唯一接触现实的方式，单测直接喂假数据。
    struct TranscriptLookup {
        /// projects 根下的全部子目录。
        var projectDirectories: () -> [URL]
        var fileExists: (URL) -> Bool

        init(projectDirectories: @escaping () -> [URL],
             fileExists: @escaping (URL) -> Bool) {
            self.projectDirectories = projectDirectories
            self.fileExists = fileExists
        }
    }

    /// 真去翻 `~/.claude/projects` 的那份探针。
    static func diskLookup(home: URL, fileManager: FileManager = .default) -> TranscriptLookup {
        let root = claudeProjectsDirectory(home: home)
        return TranscriptLookup(
            projectDirectories: {
                (try? fileManager.contentsOfDirectory(
                    at: root, includingPropertiesForKeys: [.isDirectoryKey],
                    options: [.skipsHiddenFiles]))?
                    .filter { (try? $0.resourceValues(forKeys: [.isDirectoryKey]).isDirectory) == true }
                    ?? []
            },
            fileExists: { fileManager.fileExists(atPath: $0.path) })
    }

    /// 会话号 → 日志文件，**按会话号在整个 projects 树下找**（claude 自己就是这么找的，
    /// 见类型注释的实验 1/2）。**这个结果只用来在降级之后多说一句诊断**，不许拿去决定
    /// 要不要带 `--resume`。
    ///
    /// 本机实测：3523 个日志文件、3523 个不重复会话号，**一个 id 命中多个目录的 0 条**，
    /// 所以遍历命中第一个就是答案。
    static func locateClaudeTranscript(sessionId: String,
                                       lookup: TranscriptLookup) -> URL? {
        let id = sessionId.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !id.isEmpty else { return nil }
        let file = id + ".jsonl"
        for dir in lookup.projectDirectories() {
            let candidate = dir.appendingPathComponent(file)
            if lookup.fileExists(candidate) { return candidate }
        }
        return nil
    }

    /// 降级之后补的那句诊断 —— **它只是旁证**，主句永远是 claude 的原话。
    /// 盘上真没有 → 说没有；盘上有 → 也照说，那说明是别的原因（版本、权限、文件损坏），
    /// 别让「盘上有」这件事被藏起来。
    static func resumeRejectionDiagnosis(sessionId: String, lookup: TranscriptLookup) -> String {
        if let url = locateClaudeTranscript(sessionId: sessionId, lookup: lookup) {
            return "顺带：这个会话号的日志在盘上是有的（\(url.path)），"
                + "所以不是「日志没了」，得另查（claude 版本 / 文件权限 / 文件本身坏了）。"
        }
        return "顺带：这个会话号的日志在 ~/.claude/projects 下确实一个都找不到。"
    }

    /// claude 的会话日志路径：`~/.claude/projects/<工作目录 slug>/<会话号>.jsonl`。
    ///
    /// **这只是「新会话大概率会落在哪儿」，不是「这个会话在哪儿」** —— 见类型注释里的实测。
    static func claudeTranscriptURL(sessionId: String, workdir: String, home: URL) -> URL {
        claudeProjectsDirectory(home: home)
            .appendingPathComponent(projectSlug(forWorkdir: workdir), isDirectory: true)
            .appendingPathComponent(sessionId + ".jsonl")
    }

    /// 工作目录 → claude 项目目录名。
    ///
    /// claude 把路径里**每一个不是 ASCII 字母、数字、连字符的字符**都换成 `-`，不只是
    /// `/` 和 `.`（2026-08-26 实测：在 `…/scratchpad/e1_a` 起一个 session，落进
    /// `…-scratchpad-e1-a/`，下划线也变成了 `-`；本机 145 个项目目录无一例外只含
    /// `[A-Za-z0-9-]`）。旧实现只换 `/` 和 `.`，于是名字里带下划线的目录一个都对不上
    /// —— 当时 21 个 worktree 里有 4 个正是这种（`in_progress` / `respond_todo`…）。
    static func projectSlug(forWorkdir workdir: String) -> String {
        String(workdir.map { ch in
            (ch.isASCII && (ch.isLetter || ch.isNumber)) || ch == "-" ? ch : "-"
        })
    }

    // MARK: - 这一轮跑在哪个目录（与「日志在哪儿」是两件事）

    /// 「起来就死」的窗口（秒）—— 见 `CrewSessionRunner.retryWithoutResumeIfClaudeRefused`
    /// 判据 4。只作廉价护栏，真正的判据是 claude 的那句原话。
    static let claudeResumeRefusalWindow: TimeInterval = 5

    /// @ 唤醒一个退出的成员时，这一轮该跑在哪个目录（Todo #68）。
    ///
    /// 账本里记着它当初跑在哪儿（isolation worktree 的成员记的是 worktree 路径）——
    /// **那个目录还在就回去，不在了才回落 crew 共享目录**。旧行为是一律拉回共享目录，
    /// worker 醒来在别人的目录里干活。
    ///
    /// 「不在了」是常态不是例外：worktree 合完就删，本机 62 个记过的 worktree 只剩 10 个。
    /// 所以必须真去看一眼盘，不能拿账本里的字符串当准 —— 往一个不存在的目录里起进程
    /// 是起不来的。
    ///
    /// **这一条跟「日志在哪儿」无关**：claude 的 `--resume` 按会话号找全盘，换目录跑
    /// 照样接得回原对话（见类型注释的实验 1）。两个概念分开，别合并。
    static func restartDirectory(recorded: String?, crewDirectory: URL,
                                 isDirectory: (String) -> Bool = defaultIsDirectory) -> URL {
        guard let recorded else { return crewDirectory }
        let path = (recorded.trimmingCharacters(in: .whitespacesAndNewlines) as NSString)
            .expandingTildeInPath
        guard !path.isEmpty, isDirectory(path) else { return crewDirectory }
        return URL(fileURLWithPath: path)
    }

    static let defaultIsDirectory: (String) -> Bool = { path in
        var isDir: ObjCBool = false
        return FileManager.default.fileExists(atPath: path, isDirectory: &isDir) && isDir.boolValue
    }

    // MARK: - fail-loud（不装死）

    /// 续不上时塞进这轮 brief 的开头 —— 让 agent 自己知道「我不是接着上次，是新开的」，
    /// 别对着空脑子假装记得。能续上则 nil（不加噪音）。
    static func briefNotice(for decision: Decision) -> String? {
        guard case .fresh(let reason) = decision else { return nil }
        switch reason {
        case .noRecord:
            return "【注意】你和这个成员此前的对话上下文没能接回来（没有记录），"
                + "这是**新开的一轮**。群里的白板会自动注入，别假装记得终端里聊过的细节。"
        case .agentRejectedResume(let id, let agentSaid, _):
            return "【注意】原会话（\(id)）没能续上 —— claude 自己的原话是"
                + "「\(agentSaid)」，所以这是**新开的一轮**。"
                + "群里的白板会自动注入，别假装记得终端里聊过的细节。"
        }
    }

    /// 续不上时往群里说的那句（fail-loud，人能看见）。能续上则 nil。
    static func whiteboardNotice(memberName: String, decision: Decision) -> String? {
        guard case .fresh(let reason) = decision else { return nil }
        switch reason {
        case .noRecord:
            return "「\(memberName)」的原对话接不回来了（此前没记下它的会话号），"
                + "这一轮是**新开的**——群里的上下文它还在，但终端里聊过的细节要重讲。"
        case .agentRejectedResume(_, let agentSaid, let diagnosis):
            var text = "「\(memberName)」的原对话接不回来了 —— claude 拒了这个会话号，"
                + "原话：`\(agentSaid)`。这一轮是**新开的**——"
                + "群里的上下文它还在，但终端里聊过的细节要重讲。"
            if let diagnosis, !diagnosis.isEmpty { text += "\n" + diagnosis }
            return text
        }
    }
}
#endif
