import Foundation

/// 气泡下面那一排里的**一颗**引用胶囊（人类 Todo #132 / #133）。
struct CrewMessageReferencePill: Equatable, Identifiable {

    /// 点下去到底做什么。**每一种都对应一个已经存在、且此刻真的能落地的入口** ——
    /// 没有「打开某个面板然后人自己找」这种半路动作。
    enum Action: Equatable {
        /// 开 Todo 详细窗口，落在这本账的这一条上（`CrewTodoDetailWindowPresenter`）。
        case todo(ledger: TodoLedger, number: Int)
        /// 开驾驶舱，落在这条计划上（`CockpitPresentation.focus`）。
        case plan(number: Int)
        /// 在本群时间线上滚到这条消息。
        case message(id: String)
        /// 右栏切到这个 session 的终端。
        case session(sessionId: String)
        /// 切到那个机组；能定位到具体分机时右栏一并切到它。
        case crew(crewId: String, sessionId: String?)
    }

    let label: String
    let symbol: String
    let action: Action

    /// `ForEach` 用。同一条消息上不会有两颗动作相同的胶囊（见 `pills` 的去重）。
    var id: String { "\(action)" }
}

/// `[CrewMessageReference]` → 这一刻**真的点得动**的那几颗胶囊。**纯函数，不碰 IO。**
///
/// ## 为什么判「点不点得动」要在这一层，而不是点下去再说
///
/// 硬口径是「点了没反应的胶囊，比不长这颗胶囊更糟」。一颗跳不到任何地方的胶囊
/// 不报错、不变灰 —— 它是一颗**看起来正常**的死件，人只会以为自己没点准。所以
/// 可达性要在**渲染之前**判掉：目标不可达 → 那一颗根本不长出来。
///
/// 判据全部由调用方从活的状态里取（已加载的消息、在跑的 session、解析得出的号码），
/// 这一层只做拼装 —— 于是「什么情况下不长」是可以直接写单测的。
enum CrewMessageReferencePills {

    /// 一个通讯录号码指到哪儿（调用方用 `CrewDirectory.resolve` 解出来）。
    struct Target: Equatable {
        var crewId: String
        /// 显示名（`7` 是机组名，`7-1` / `7-3` 是「机组名 · 成员名」）。
        var title: String
        /// 指到具体分机时的 session id；整个机组（`7`）为 nil。
        var sessionId: String?

        init(crewId: String, title: String, sessionId: String? = nil) {
            self.crewId = crewId
            self.title = title
            self.sessionId = sessionId
        }
    }

    /// 渲染这一刻的可达性事实。
    struct Context: Equatable {
        /// 这条消息自己的 id。指向自己的引用不长胶囊（跳到原地等于没反应）。
        var selfMessageId: String?
        /// 本群此刻**已经加载进来**的消息 id。不在里面的滚不到。
        var loadedMessageIds: Set<String>
        /// 这台机器上打得开的 session：id → 显示名。打不开的不长。
        var openableSessions: [String: String]
        /// 解析得出的号码：号码文本（`7` / `7-1`）→ 目标。查无此号的不长。
        var crewTargets: [String: Target]

        init(selfMessageId: String? = nil,
             loadedMessageIds: Set<String> = [],
             openableSessions: [String: String] = [:],
             crewTargets: [String: Target] = [:]) {
            self.selfMessageId = selfMessageId
            self.loadedMessageIds = loadedMessageIds
            self.openableSessions = openableSessions
            self.crewTargets = crewTargets
        }
    }

    static func pills(_ references: [CrewMessageReference]?,
                      in context: Context) -> [CrewMessageReferencePill] {
        var out: [CrewMessageReferencePill] = []
        for reference in references ?? [] {
            // 认不出的种类只丢**这一颗** —— 同一条消息上其它胶囊照长。
            guard let kind = reference.resolvedKind,
                  let pill = pill(kind: kind, target: reference.targetId, in: context)
            else { continue }
            guard !out.contains(where: { $0.action == pill.action }) else { continue }
            out.append(pill)
        }
        return out
    }

    // MARK: - 一颗

    private static func pill(kind: CrewMessageReference.Kind, target: String,
                             in context: Context) -> CrewMessageReferencePill? {
        switch kind {
        case .humanTodo:
            guard let n = number(target) else { return nil }
            return .init(label: "人类 Todo #\(n)", symbol: "person.crop.square",
                         action: .todo(ledger: .human, number: n))
        case .agentTodo:
            guard let n = number(target) else { return nil }
            return .init(label: "Todo #\(n)", symbol: "checklist",
                         action: .todo(ledger: .agent, number: n))
        case .plan:
            guard let n = number(target) else { return nil }
            return .init(label: "计划 #\(n)", symbol: "list.bullet.rectangle",
                         action: .plan(number: n))
        case .message:
            // 滚不到 = 点了没反应；指向自己 = 跳到原地，同样是没反应。
            guard context.loadedMessageIds.contains(target),
                  target != context.selfMessageId else { return nil }
            return .init(label: "那条消息", symbol: "text.bubble",
                         action: .message(id: target))
        case .session:
            guard let name = context.openableSessions[target] else { return nil }
            return .init(label: name, symbol: "terminal",
                         action: .session(sessionId: target))
        case .crew:
            guard let t = context.crewTargets[target] else { return nil }
            return .init(label: "\(target) · \(t.title)", symbol: "person.2",
                         action: .crew(crewId: t.crewId, sessionId: t.sessionId))
        }
    }

    /// `#N` 的 N。**必须是纯十进制正整数** —— `"0"` / `"-1"` / `"1.5"` / `"12x"`
    /// 都不是「没给」，是给错了，让它长出一颗点进去什么也没有的胶囊比不长更糟。
    private static func number(_ raw: String) -> Int? {
        guard !raw.isEmpty, raw.allSatisfy({ $0.isASCII && $0.isNumber }),
              let n = Int(raw), n > 0 else { return nil }
        return n
    }
}
