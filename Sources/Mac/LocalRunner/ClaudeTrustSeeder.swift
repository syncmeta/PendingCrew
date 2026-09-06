#if os(macOS)
import Foundation

/// `ClaudeTrustSeedPlan` 的**执行层** —— 真的去改 `~/.claude.json` 的信任位。
///
/// 三条纪律照 `WorkdirMigrationExecutor` 那套原样搬（那边迁移工作目录时早就这么干了，
/// **不发明第二种**）：
///
/// 1. **写前备份**：动 `~/.claude.json` 之前整份拷进调用方给的备份目录。备份不成，
///    一步都不走。
/// 2. **写完回读校验**：这份文件是全机 claude 共写的一份（每个 session 退出都会覆写
///    一遍，本机常年十几个在跑），我们刚写进去的**真的可能被别的进程覆盖掉**。
///    没读回来就**不算补上**，回执出警告 —— 绝不静默当成功。
/// 3. **不吞错**：读不到 / 解不开 / 备份失败，都如实进回执。
///
/// 与迁移那边**故意的两处不同**（都因为「创建」这条路没有源目录）：
/// - 只写 `hasTrustDialogAccepted` 一个键（理由见 `ClaudeTrustSeedPlan`）。
/// - `projects` 表不存在时**建表**，而不是拒绝。迁移要从源条目里取值，没有表就没有源，
///   拒绝是对的；补种没有源，没有表只说明「这台机器还没记过任何目录」。
enum ClaudeTrustSeeder {

    // MARK: - 文件出入口（可注入 —— 单测要模拟「写进去又被别的进程覆盖回去」）

    struct IO {
        var read: () throws -> Data
        var write: (Data) throws -> Void

        init(read: @escaping () throws -> Data, write: @escaping (Data) throws -> Void) {
            self.read = read
            self.write = write
        }

        /// 真家伙：原子写 + **按原样恢复权限**。`~/.claude.json` 是 600（里面有账号），
        /// 原子替换会带默认权限，不恢复等于把凭证类文件放宽。
        static func real(home: URL, fileManager fm: FileManager = .default) -> IO {
            let url = WorkdirMigrationExecutor.claudeJSONURL(home: home)
            return IO(
                read: { try Data(contentsOf: url) },
                write: { data in
                    let perms = (try? fm.attributesOfItem(atPath: url.path))?[.posixPermissions]
                    try data.write(to: url, options: .atomic)
                    if let perms {
                        try? fm.setAttributes([.posixPermissions: perms], ofItemAtPath: url.path)
                    }
                })
        }
    }

    // MARK: - 回执

    struct Receipt: Equatable {
        /// 归一后的目标路径（skip 到连路径都算不出来时为空）。
        var path: String = ""
        /// **确认落住**的键。没读回来就不许进这里。
        var seededKeys: [String] = []
        /// 没做的原因（做了就是 nil）。
        var skip: ClaudeTrustSeedPlan.Skip?
        /// 这次的备份目录（没写就没有备份）。
        var backupPath: String?
        /// 做了、但没能确认落住的事 —— 人得知道第一次进这个目录可能还要手点一次信任框。
        var warnings: [String] = []
        /// 非 nil = 中途停了。
        var failure: String?

        var succeeded: Bool { failure == nil }
        /// 真写进去并确认落住了。
        var seeded: Bool { !seededKeys.isEmpty }
    }

    // MARK: - 执行

    /// 给 `workdir` 补种 claude 的信任位。
    ///
    /// - Parameters:
    ///   - authorization: **人的授权**。`.notGranted` 时一个字都不写（红线：替人写信任位
    ///     等于替人做了他没授权的事）。什么动作算授权由调用方定，不在这一层。
    ///   - backupDirectory: 这次的备份目录（调用方带时间戳建，见 `backupDirectory(base:now:)`）。
    ///   - io: 只为单测可注入；生产用 `.real(home:)`。
    @discardableResult
    static func seed(workdir: String,
                     authorization: ClaudeTrustSeedPlan.Authorization,
                     home: URL,
                     backupDirectory: URL,
                     fileManager fm: FileManager = .default,
                     io injected: IO? = nil,
                     attempts: Int = 3,
                     waitBetween: TimeInterval = 0.35) -> Receipt {
        let io = injected ?? .real(home: home, fileManager: fm)

        // ── 1. 读现状。读不到先记着 —— 只有在「本来要写」时它才算失败
        //    （没授权 / 早就信任过的情况下，读不到也不该报错吓人）。
        var readFailure: String?
        var existing = WorkdirMigrationPlan.ClaudeProjectSettings()
        do {
            existing = try snapshot(of: workdir, data: io.read())
        } catch {
            readFailure = "读不到 ~/.claude.json：\(error.localizedDescription)"
        }

        // ── 2. 判定（纯函数）。
        let decision = ClaudeTrustSeedPlan.decide(
            .init(workdir: workdir, authorization: authorization, existing: existing))
        guard case .seed(let path, let keys) = decision else {
            if case .skip(let skip) = decision {
                return Receipt(path: pathOf(skip) ?? "", skip: skip)
            }
            return Receipt()
        }
        if let readFailure { return Receipt(path: path, failure: readFailure) }

        var receipt = Receipt(path: path)

        // ── 3. 写前备份。备份不成，一步都不走。
        do {
            try fm.createDirectory(at: backupDirectory, withIntermediateDirectories: true)
            let src = WorkdirMigrationExecutor.claudeJSONURL(home: home)
            if fm.fileExists(atPath: src.path) {
                let dst = backupDirectory.appendingPathComponent(src.lastPathComponent)
                if fm.fileExists(atPath: dst.path) { try fm.removeItem(at: dst) }
                try fm.copyItem(at: src, to: dst)
            }
            receipt.backupPath = backupDirectory.path
        } catch {
            receipt.failure = "备份失败，没有改动任何文件：\(error.localizedDescription)"
            return receipt
        }

        // ── 4. 写 + 回读校验。
        do {
            let (confirmed, used) = try writeVerified(
                path: path, keys: keys, io: io, attempts: attempts, waitBetween: waitBetween)
            receipt.seededKeys = confirmed
            let lost = keys.filter { !confirmed.contains($0) }
            if !lost.isEmpty {
                receipt.warnings.append(
                    "claude 的目录信任位没落住（写了 \(used) 次，读回来还是没有 —— "
                    + "`~/.claude.json` 被别的 claude 进程覆盖了）。"
                    + "第一次进 `\(path)` 可能还要人手点一次「信任这个文件夹」。")
            }
        } catch {
            receipt.failure = "写 ~/.claude.json 失败：\(error.localizedDescription)"
        }
        return receipt
    }

    // MARK: - 读 / 写

    /// 一份 `~/.claude.json` 字节里，这个路径的条目快照（与迁移那边同一把尺子）。
    static func snapshot(of workdir: String, data: Data)
        throws -> WorkdirMigrationPlan.ClaudeProjectSettings {
        let path = WorkdirMigrationPlan.normalize(workdir)
        guard let root = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw SeedError("~/.claude.json 不是一个 JSON 对象。")
        }
        guard let entry = (root["projects"] as? [String: Any])?[path] as? [String: Any] else {
            return WorkdirMigrationPlan.ClaudeProjectSettings()
        }
        let keys = WorkdirMigrationPlan.claudeSettingsKeys
            .filter { WorkdirMigrationPlan.isMeaningful(entry[$0]) }
        return WorkdirMigrationPlan.ClaudeProjectSettings(exists: true, meaningfulKeys: Set(keys))
    }

    /// 写 + 读回校验，没落住就重试。返回**真正确认落住**的键。
    static func writeVerified(path: String, keys: [String], io: IO,
                              attempts: Int, waitBetween: TimeInterval)
        throws -> (confirmed: [String], attemptsUsed: Int) {
        let rounds = max(1, attempts)
        for attempt in 1...rounds {
            try writeOnce(path: path, keys: keys, io: io)
            let landed = try snapshot(of: path, data: io.read())
            let confirmed = keys.filter { landed.meaningfulKeys.contains($0) }
            if confirmed.count == keys.count { return (confirmed, attempt) }
            if attempt < rounds, waitBetween > 0 { Thread.sleep(forTimeInterval: waitBetween) }
        }
        let landed = try snapshot(of: path, data: io.read())
        return (keys.filter { landed.meaningfulKeys.contains($0) }, rounds)
    }

    /// 读—改—写一次：把 `keys` 置真，**别的一个字不动**（同条目里的其它键、别人的条目、
    /// `projects` 以外的设置，全部原样留着）。
    static func writeOnce(path: String, keys: [String], io: IO) throws {
        guard var root = try JSONSerialization.jsonObject(with: try io.read()) as? [String: Any]
        else { throw SeedError("~/.claude.json 不是一个 JSON 对象，拒绝改写。") }
        // 表不在就建 —— 与迁移那边故意的不同，理由见类型注释。
        var projects = (root["projects"] as? [String: Any]) ?? [:]
        var entry = (projects[path] as? [String: Any]) ?? [:]
        for key in keys { entry[key] = true }
        projects[path] = entry
        root["projects"] = projects
        let out = try JSONSerialization.data(
            withJSONObject: root, options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
        try io.write(out)
    }

    // MARK: - 小工具

    /// 这次补种的备份目录（照 `WorkdirChangeCommand` 的时间戳口径，别造第二种命名）。
    static func backupDirectory(base: URL, now: Date = Date()) -> URL {
        let stamp = ISO8601DateFormatter().string(from: now)
            .replacingOccurrences(of: ":", with: "-")
        return base.appendingPathComponent("backups/claude-trust-seed-\(stamp)", isDirectory: true)
    }

    private static func pathOf(_ skip: ClaudeTrustSeedPlan.Skip) -> String? {
        switch skip {
        case .emptyWorkdir: return nil
        case .alreadyTrusted(let path), .notAuthorized(let path): return path
        }
    }

    struct SeedError: LocalizedError, Equatable {
        let message: String
        init(_ message: String) { self.message = message }
        var errorDescription: String? { message }
    }
}
#endif
