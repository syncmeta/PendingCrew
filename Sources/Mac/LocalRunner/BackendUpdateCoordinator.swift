#if os(macOS)
import Foundation

/// 「前端更新了，本机后端跟着换代」的执行层。判定在 `BackendUpdatePlan`（有测试），
/// 这里只做三件有副作用的事：问版本、告诉受影响的 crew、停掉旧的。
///
/// **起新的不在这里** —— `ViewerSessionClient.launchAndRace()` 本来就在做这件事
/// （连不上就拉起、并行等握手与子进程终止）。再写一条拉起路径就是第二种做法，
/// 而两条拉起路径迟早会分叉。这里只负责把旧的停掉，剩下的交回给已有的那条。
///
/// 放在 `LocalRunner` 而不是 `Services`：它用到的东西（探针 / stopper / registry /
/// 白板）**全都进得了 test bundle**，所以「停不掉时不许假装换过了」那条分支
/// 可以真的被测到。判定有测试而接线没有，是今天全组抓到最多的形状之一。
@MainActor
enum BackendUpdateCoordinator {

    /// 本机后端现在什么情况。**三态，不压成两态**：
    /// 「没有后端」和「有后端但问不出版本」对下一步的意义完全不同 ——
    /// 前者什么都不用做，后者必须按兵不动并且喊出来。
    static func probeState(paths: PendingCrewDaemonPaths = .standard())
        -> BackendUpdatePlan.BackendState {
        guard SessionDaemonControl.runningDaemonPid(paths: paths) != nil else { return .none }
        do {
            let snapshot = try SessionDaemonStatusProbe.query(paths: paths)
            return .running(build: snapshot.hello.daemonBuild,
                            pid: snapshot.hello.pid,
                            sessionCount: snapshot.hello.sessionCount)
        } catch {
            return .undecidable(error.localizedDescription)
        }
    }

    /// 该换就换。返回判定本身，调用方据此决定要不要接着问人恢复。
    ///
    /// - Parameters:
    ///   - log: 每一条判定都要落进去。**`leaveAlone` 也要落** —— 一个永远换不了代的
    ///     后台跟一个每次启动都被打断的后台一样糟，只是它安静。
    ///   - announce: 往某个 crew 的群里说一句（注入以便不碰真白板）。
    @discardableResult
    static func runIfNeeded(
        appBuild: String = SessionDaemonHost.currentBuild,
        paths: PendingCrewDaemonPaths = .standard(),
        log: (String) -> Void,
        probe: (() -> BackendUpdatePlan.BackendState)? = nil,
        stop: (() -> DaemonStopOutcome)? = nil,
        affected: (() -> [String])? = nil,
        announce: (String, String) -> Void = { crewId, text in
            LocalWhiteboardStore.shared.appendSessionMessage(
                crewId: crewId, sessionId: "system", text: text,
                category: "progress", senderName: "系统")
        }
    ) -> BackendUpdatePlan.Decision {
        let state = probe?() ?? probeState(paths: paths)
        let decision = BackendUpdatePlan.decide(backend: state, appBuild: appBuild)
        switch decision {
        case let .leaveAlone(why):
            log("后端换代：\(why)")
        case let .replace(oldBuild, newBuild, sessionCount):
            log("后端换代：本机后端 \(oldBuild) → \(newBuild)，先告诉受影响的 crew，再停它。")
            // **先说再停。** 反过来的话，人先看到 session 全断、几秒后才看到解释。
            let text = BackendUpdatePlan.announcement(
                oldBuild: oldBuild, newBuild: newBuild, sessionCount: sessionCount)
            let crews = affected?() ?? affectedCrewIds(paths: paths)
            // **「有几个」和「通知谁」来自两个不同的源**：数目来自 daemon 的握手，
            // 名单来自盘上的 registry。它们会不一致 —— registry 读不动、或者刚被清过，
            // 名单就是空的，而 session 照断不误。**那种时候必须喊出来**，
            // 否则就是「打断了一批人，一个都没通知」，而且没有任何痕迹。
            if sessionCount > 0 && crews.isEmpty {
                log("后端换代：要打断 \(sessionCount) 个 session，"
                    + "但 registry 里读不出它们属于哪些 crew —— **没有人会收到通知**。")
            }
            for crewId in crews { announce(crewId, text) }

            let outcome = stop?()
                ?? DaemonStopper(dataRoot: paths.lock.deletingLastPathComponent()).stop()
            log("后端换代：\(outcome.text)")
            // 停不掉时**不要假装换过了** —— 调用方会据此去问人恢复，而根本没断。
            if !outcome.isSuccess {
                return .leaveAlone(why: "想换代但停不掉旧后端：\(outcome.text)")
            }
        }
        return decision
    }

    /// 哪些 crew 会被这次换代打断 —— 就是 registry 里还记着进程的那些。
    /// **不新造第二本账**：收尸逻辑遍历的就是这一份。
    static func affectedCrewIds(paths: PendingCrewDaemonPaths = .standard()) -> [String] {
        guard let data = try? Data(contentsOf: paths.registry),
              let registry = try? JSONDecoder().decode(SessionProcessRegistry.self, from: data)
        else { return [] }
        var seen = Set<String>()
        return registry.entries.map(\.crewId).filter { seen.insert($0).inserted }
    }
}
#endif
