import Foundation

/// 每个 crew 一份可直接编辑的机长派活原则，另存一份自动复盘快照。
/// 两者分开：自动刷新绝不覆盖人或机长写的原则。目录与白板相同，app、
/// Claude hook 和 Codex turn provider 都能读到同一份内容。
struct CaptainDelegationPolicyStore {
    enum Route: String, Codable {
        case localWorker
        case newChildCrew
        case existingChildCrew
        case otherCrew

        var label: String {
            switch self {
            case .localWorker: return "本组 worker"
            case .newChildCrew: return "新子组"
            case .existingChildCrew: return "已有子组"
            case .otherCrew: return "其他 crew"
            }
        }
    }

    struct Prepared {
        let text: String
        fileprivate let crewId: String
        fileprivate let sessionId: String
        fileprivate let revision: String
    }

    private struct Event: Codable {
        let route: Route
        let at: Date
    }

    private struct Snapshot: Codable {
        let refreshedAt: Date
        let text: String
    }

    static let refreshInterval: TimeInterval = 24 * 60 * 60
    static let historyWindow: TimeInterval = 14 * 24 * 60 * 60
    private static let defaultPolicy = """
    你负责把任务送到最合适的人或 crew，并验收结果。接新活先看现有成员和本机 crew：
    - 已有执行 crew 与主题吻合时，联系其机长交接；同组已有合适成员时复用它。
    - 独立且会长期演进的主题，可建子 crew；一次性明确任务可交 worker。小事和紧密耦合的短步骤无需层层转派。
    - 派活写清目标、边界、完成证据和回报点；不要只发一句话就把责任放掉。交出去后跟进、验收、处理阻碍。
    - 每次复盘看最近的完成、返工、等待和协调成本。某条路线效果差就调整下一次选择；不要追求固定派活比例，也不要为了避免自己动手而多养一个机长。
    - 机长自己保留澄清、拆解、协调、验收及很小的即时动作。成块的执行工作交给合适的执行者，避免全部沉在机长会话。
    这份原则可由人编辑，或由机长用 update_delegation_policy 修订。周期快照只提供证据和当前侧重点，不覆盖这份原则。
    """
    private static let chiefPolicy = """
    你是总机组协调层。只识别、拆解、投递、跟进和汇总，不在总机组运行 worker，也不亲自编辑项目、跑项目诊断或部署。
    接任务先看已有执行 crew 和汇报线；主题匹配时用 contact 或下行消息交给其机长，缺执行 crew 时建顶层执行 crew。交接写清目标、范围、完成证据和回报点。
    跟进已投递任务的接收、进展、阻碍与验收，避免因为对方慢就把执行工作收回自己会话。跨组联系过密时先检查职责边界和信息粒度，不按固定比例建组。
    周期复盘结合实际完成、返工和等待来修订这份原则。人可直接编辑文件；机长也可用 update_delegation_policy 修订。自动快照只提供证据，不覆盖原则。
    """

    let directory: URL

    init(directory: URL = LocalWhiteboardStore.defaultDirectory) {
        self.directory = directory
    }

    func policyURL(crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).captain-delegation.md")
    }

    func ensurePolicy(crewId: String) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = policyURL(crewId: crewId)
        // 检查与写入持同一把锁；并发起机长也不覆盖用户刚修改的内容。
        try MultiProcessJSONStore.withFileLock(policyLockURL(crewId)) {
            guard !FileManager.default.fileExists(atPath: url.path) else { return }
            let initial = crewId == LocalCrew.chiefCrewId
                ? Self.chiefPolicy : Self.defaultPolicy
            try MultiProcessJSONStore.writeStaged(Data(initial.utf8), to: url)
        }
    }

    func readPolicy(crewId: String) throws -> String {
        try String(contentsOf: policyURL(crewId: crewId), encoding: .utf8)
    }

    func updatePolicy(crewId: String, text: String) throws {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { throw CocoaError(.fileWriteInvalidFileName) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try MultiProcessJSONStore.withFileLock(policyLockURL(crewId)) {
            _ = try MultiProcessJSONStore.writeStaged(
                Data(trimmed.utf8), to: policyURL(crewId: crewId))
        }
    }

    /// 只记已经被工具接受的路由动作，不宣称任务已执行或完成。事件账与策略文件分开。
    @discardableResult
    func record(crewId: String, route: Route, now: Date = Date()) -> Bool {
        guard (try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true)) != nil else { return false }
        let url = eventsURL(crewId)
        return MultiProcessJSONStore.withFileLock(lockURL(crewId)) {
            let old: [Event]
            if FileManager.default.fileExists(atPath: url.path) {
                guard let data = try? Data(contentsOf: url),
                      let decoded = try? JSONDecoder().decode([Event].self, from: data)
                else { return false } // 损坏时保留原账，不拿空数组覆盖。
                old = decoded
            } else {
                old = []
            }
            let keepFrom = now.addingTimeInterval(-90 * 24 * 60 * 60)
            let rows = old.filter { $0.at >= keepFrom } + [Event(route: route, at: now)]
            guard let data = try? JSONEncoder().encode(rows) else { return false }
            do {
                _ = try MultiProcessJSONStore.writeStaged(data, to: url)
                return true
            } catch {
                return false
            }
        }
    }

    /// 首次、策略被修改、或 24 小时后的第一次活跃轮次才注入；其余轮次不重复
    /// 喂整份策略。快照重算也只在活跃轮次发生，空闲时不额外唤醒机长耗额度。
    func prepare(crewId: String, sessionId: String, now: Date = Date()) -> Prepared? {
        guard let policy = try? readPolicy(crewId: crewId) else { return nil }
        let snapshot = refreshedSnapshot(crewId: crewId, now: now)
        let content = """
        【机长常驻派活策略】（可编辑：\(policyURL(crewId: crewId).path)）
        \(policy)

        \(snapshot.text)
        """
        let revision = String(content.utf8.reduce(UInt64(14695981039346656037)) {
            ($0 ^ UInt64($1)) &* UInt64(1099511628211)
        }, radix: 16)
        let seen = (try? String(contentsOf: seenURL(crewId, sessionId), encoding: .utf8))?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard revision != seen else { return nil }
        return Prepared(text: content, crewId: crewId, sessionId: sessionId, revision: revision)
    }

    func commit(_ prepared: Prepared) {
        _ = try? MultiProcessJSONStore.writeStaged(
            Data(prepared.revision.utf8),
            to: seenURL(prepared.crewId, prepared.sessionId))
    }

    private func refreshedSnapshot(crewId: String, now: Date) -> Snapshot {
        let url = snapshotURL(crewId)
        if let data = try? Data(contentsOf: url),
           let existing = try? JSONDecoder().decode(Snapshot.self, from: data),
           now.timeIntervalSince(existing.refreshedAt) >= 0,
           now.timeIntervalSince(existing.refreshedAt) < Self.refreshInterval {
            return existing
        }
        let events: [Event]? = MultiProcessJSONStore.withFileLock(lockURL(crewId)) {
            let url = eventsURL(crewId)
            guard FileManager.default.fileExists(atPath: url.path) else { return [] }
            guard let data = try? Data(contentsOf: url) else { return nil }
            return try? JSONDecoder().decode([Event].self, from: data)
        }
        let recent = (events ?? []).filter {
            let age = now.timeIntervalSince($0.at)
            return age >= 0 && age <= Self.historyWindow
        }
        // 旧版本没有路由事件账，白板仍能提供一部分既有历史。这里只数发言
        // 和跨组联系回执，绝不从发言量推断「任务完成」或「机长亲自干活」。
        let messages = LocalWhiteboardStore(directory: directory).list(crewId: crewId)
        let historyAvailable = LocalWhiteboardStore.readFailure(in: messages) == nil
        let history = historyAvailable
            ? messages.filter { row in
                guard let at = ISO8601DateFormatter().date(from: row.createdAt) else {
                    return false
                }
                let age = now.timeIntervalSince(at)
                return age >= 0 && age <= Self.historyWindow
            }
            : []
        let captainNotes = history.filter {
            $0.senderKind == "captain" || $0.senderSessionId?.hasPrefix("captain-") == true
        }.count
        let workerMessages = history.filter {
            $0.senderKind == "session"
                && $0.senderSessionId?.hasPrefix("captain-") != true
                && $0.category != "contact_receipt"
        }
        let workerNotes = workerMessages.count
        let workerQuestions = workerMessages.filter { $0.category == "question" }.count
        let workerMilestones = workerMessages.filter { $0.category == "milestone" }.count
        let crossReceipts = history.filter { $0.category == "contact_receipt" }.count
        func count(_ route: Route) -> Int { recent.filter { $0.route == route }.count }
        let local = count(.localWorker)
        let child = count(.newChildCrew) + count(.existingChildCrew)
        let other = count(.otherCrew)
        let focus: String
        if events == nil || !historyAvailable {
            focus = "历史账本有读不出的部分，先查数据目录和白板；这次不要据零计数改变派活方式。"
        } else if crewId == LocalCrew.chiefCrewId {
            focus = other == 0 && child == 0
                ? "下一件执行任务先核对已有执行 crew 并投递；总机组只负责协调和验收。"
                : "核对已联系的执行 crew 是否接住、反馈和完成；需要新领域时再建执行 crew。"
        } else if workerQuestions > 0 && workerMilestones == 0 {
            focus = "近期有 worker 提问、没有里程碑记录。先核对问题是否已回应及任务是否接住，再决定继续派活；分类计数不能证明仍被阻塞。"
        } else if recent.isEmpty && captainNotes > 0 && workerNotes == 0 {
            focus = "旧白板里机长有发言、worker 无发言。核对是否有成块执行留在机长会话；发言数本身不能证明工作归属。"
        } else if recent.isEmpty {
            focus = "路由记录尚少；先按任务契合度和现有成员情况判断，随后用实际完成与等待情况修订。"
        } else if local > 0 && other == 0 && child == 0 {
            focus = "近期有本组派活、没有跨组路由记录。遇到独立主题，先查现有 crew 是否已有合适负责人；不要仅凭这个计数强行拆组。"
        } else if child > local + other {
            focus = "近期子组路由较多。下一次复查是否有短任务可交现有 worker，及子组的协调开销。"
        } else if other > local + child {
            focus = "近期跨组联系较多。下一次核对对方是否接住和回报；紧密耦合的小任务可留在本组。"
        } else {
            focus = "近期使用了多种路由。继续按完成质量、等待与协调成本选下一次负责人。"
        }
        let routeLine = events == nil
            ? "路由事件账读不出来"
            : "本组 worker \(local) 次；已有/新建子组 \(child) 次；其他 crew 联系 \(other) 次"
        let historyLine = historyAvailable
            ? "机长发言 \(captainNotes) 条、worker 发言 \(workerNotes) 条（提问 \(workerQuestions)、里程碑 \(workerMilestones)）、跨组联系回执 \(crossReceipts) 条"
            : "白板读不出来"
        let text = """
        派活复盘（最近 14 天，\(ISO8601DateFormatter().string(from: now)) 刷新）：\(routeLine)。
        既有白板：\(historyLine)。
        当前侧重点：\(focus)
        路由计数仅是新版工具接受的动作；白板可覆盖部分旧历史。两者均不能证明送达、完成或机长自己在 shell 做了什么。复盘时查看实际产出，不要把计数当绩效。
        """
        let snapshot = Snapshot(refreshedAt: now, text: text)
        if let data = try? JSONEncoder().encode(snapshot) {
            _ = try? MultiProcessJSONStore.writeStaged(data, to: url)
        }
        return snapshot
    }

    private func eventsURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).captain-delegation-events.json")
    }
    private func snapshotURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).captain-delegation-snapshot.json")
    }
    private func lockURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).captain-delegation-events.lock")
    }
    private func policyLockURL(_ crewId: String) -> URL {
        directory.appendingPathComponent("\(crewId).captain-delegation-policy.lock")
    }
    private func seenURL(_ crewId: String, _ sessionId: String) -> URL {
        directory.appendingPathComponent("\(crewId).\(sessionId).captain-delegation-seen")
    }
}
