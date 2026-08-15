import Foundation

/// 未捕获 `NSException` 的**留痕**（fail-loud）。
///
/// 病根（2026-07-26 17:24 闪退）：AppKit 在布局阶段抛 `NSGenericException`
///（"The window has been marked as needing another Update Constraints in Window
/// pass, but it has already had more Update Constraints in Window passes than
/// there are views in the window"），`+[NSApplication _crashOnException:]` 直接
/// 把进程打死。崩溃报告里**只有 AppKit 栈、没有一行本仓代码**，异常的 name/reason
/// 也不在 .ips 里（`asi` 为空）——现场只能靠翻统一日志才拼得出来，成本极高。
///
/// 这里做的事很小但关键：进程级挂一个未捕获异常处理器，把 name / reason /
/// userInfo / 调用栈落到 `Application Support/PendingCrew/crashes/<时间戳>.log`，
/// 并 NSLog 一行摘要。下次再崩，第一手材料就在盘上，不用去 log show 里捞。
///
/// **边界（别高估它）**：
/// - 它只保证「异常有留痕」，**不阻止崩溃**，也不修任何布局问题。
/// - AppKit 的 crash-on-exception 路径是否一定走到未捕获处理器，Apple 没有承诺；
///   走不到时仍要靠统一日志（`log show --predicate 'process == "PendingCrew"'`
///   搜 `Update Constraints in Window` / `layoutSubtreeIfNeeded ... iterations`）。
///   两条路都留着，见 `docs/tech-debt.md` 同日条目里的排查手顺。
/// - Swift 的 `fatalError` / 数组越界不是 NSException，走不到这里。
enum UncaughtExceptionLog {

    /// 进程入口调用一次（`PendingCrewEntry.main`）。重复调用安全（后者覆盖前者）。
    static func install() {
        NSSetUncaughtExceptionHandler { exception in
            // 处理器在崩溃路径上跑，能少做事就少做事：拼字符串 + 一次同步写。
            let stamp = ISO8601DateFormatter().string(from: Date())
            var text = "PendingCrew uncaught NSException @ \(stamp)\n"
            text += "name: \(exception.name.rawValue)\n"
            text += "reason: \(exception.reason ?? "(nil)")\n"
            if let info = exception.userInfo, !info.isEmpty { text += "userInfo: \(info)\n" }
            text += "\ncallStack:\n" + exception.callStackSymbols.joined(separator: "\n") + "\n"

            NSLog("[PendingCrew] 未捕获异常 %@: %@",
                  exception.name.rawValue, exception.reason ?? "(nil)")

            guard let dir = UncaughtExceptionLog.crashDirectory() else { return }
            let file = dir.appendingPathComponent("\(stamp.replacingOccurrences(of: ":", with: "-")).log")
            try? text.write(to: file, atomically: true, encoding: .utf8)
            NSLog("[PendingCrew] 异常详情已写入 %@", file.path)
        }
    }

    /// `Application Support/PendingCrew/crashes/`；建不出来就返回 nil（崩溃路径上不再抛）。
    private static func crashDirectory() -> URL? {
        guard let support = try? FileManager.default.url(
            for: .applicationSupportDirectory, in: .userDomainMask,
            appropriateFor: nil, create: true) else { return nil }
        let dir = support
            .appendingPathComponent("PendingCrew", isDirectory: true)
            .appendingPathComponent("crashes", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        } catch {
            return nil
        }
        return dir
    }
}
