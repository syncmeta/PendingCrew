import Foundation

/// 中栏时间线「这一帧到底渲染哪些条目」的**纯判定**，以及它的记忆盒（人类 Todo #140 ③）。
///
/// ## 为什么要把它从 `CrewChatView.timelineEntries` 里抽出来
///
/// 那是个计算属性，而一次 body 求值会读它**八次**（空态 overlay、`onChange(of:count)`、
/// 「上面还有 N 条」、`windowedEntries`、空态出路判定、`expandEarlier`、跳转定位……
/// 逐处见 `CrewChatView`）。每一次都把全部条目重新筛一遍 —— 本 crew 现状 2618 条、
/// 正文 965 KB，于是**一帧里那 965 KB 被扫八遍**。源码注释自己写着「这个属性每次访问
/// 都重算，所以判定必须廉价」（#443 的口径），在这个量级上它已经不够廉价了。
///
/// 抽出来之后有两个好处：判定能脱离 SwiftUI 单测（成本也能被钉住），以及它有了一个
/// 可以挂缓存的地方。
enum CrewTimelineFilter {

    /// 一次筛选的全部输入。**缓存键就是它本身**（逐字段 `==`），不是什么指纹或条数 ——
    /// 指纹那种便宜代理平时都对，只在「条数没变但内容变了」（撤回一条又来一条、
    /// 订阅重放）那一下错，而那一下正是缓存必须失效的时刻。
    struct Inputs: Equatable {
        let entries: [CrewWhiteboardEntry]
        /// 「只看 @ 我的」开关。
        let onlyMentions: Bool
        let roster: CrewMentionFilter.Roster
        /// 非 nil 时「自己发的」一并留下（口径见 `CrewMentionFilter.onlyHumanMentions`）。
        let localUserId: String?
        /// **已 trim 过**的搜索词；空串 = 没在搜索。调用方负责 trim（`CrewChatView.searchText`
        /// 本来就是 trim 过的那一份），这里不再 trim，免得同一个判据有两处口径。
        let searchText: String
        let crewId: String
        let crewTitle: String

        init(
            entries: [CrewWhiteboardEntry], onlyMentions: Bool,
            roster: CrewMentionFilter.Roster, localUserId: String?,
            searchText: String, crewId: String, crewTitle: String
        ) {
            self.entries = entries
            self.onlyMentions = onlyMentions
            self.roster = roster
            self.localUserId = localUserId
            self.searchText = searchText
            self.crewId = crewId
            self.crewTitle = crewTitle
        }
    }

    /// 与抽出来之前的 `timelineEntries` **逐字等价**：先筛 @、再按搜索词回滤，保持源序。
    static func resolve(_ inputs: Inputs) -> [CrewWhiteboardEntry] {
        CrewChatCostCounters.note(.timelineFilterRun)
        let mentionFiltered = inputs.onlyMentions
            ? CrewMentionFilter.onlyHumanMentions(
                inputs.entries, roster: inputs.roster, includingFrom: inputs.localUserId)
            : inputs.entries
        guard !inputs.searchText.isEmpty else { return mentionFiltered }

        // 核心统一按「最新优先、最多 200」选出结果；聊天时间线仍按原来的时间正序
        // 展示，所以最后用 id 集合回滤 source order。
        let documents = mentionFiltered.map {
            CrewMessageSearchAdapters.entry($0, crewId: inputs.crewId, crewTitle: inputs.crewTitle)
        }
        let ids = Set(CrewMessageSearch.search(
            documents, query: inputs.searchText, limit: CrewMessageSearch.maximumLimit,
            order: .newestFirst).map(\.document.messageId))
        return mentionFiltered.filter { ids.contains($0.id) }
    }
}

/// 上面那个判定的**单槽记忆盒**：输入一个字没变就还上一次的结果。
///
/// ## 为什么是引用型、而且**绝不能**往 `@State` 重新赋值
///
/// 它在 `CrewChatView` 里的住法是 `@State private var ... = CrewTimelineFilterCache()` ——
/// 和同文件里的 `selectionOwner` / `scrollPhaseBox` / `topAnchorBox` 一套做法：取一个跨
/// body 稳定的实例，**从头到尾没人给这个 `@State` 赋过值**。
///
/// 不这么做的唯一替代是「把筛好的数组存进 `@State`」，而那条路 `CrewChatView.refresh()`
/// 的注释里已经写死了为什么不行：往 `@State` 重新赋一个数组，哪怕内容一模一样，SwiftUI
/// 也会让整个 body 失效，`LazyVStack` 于是把整条消息列表重新测量一遍（#443 已经为此
/// 吃过一次亏）。**缓存是为了省钱的，不能反过来引发一次全表重排。**
///
/// ## 单槽够不够
///
/// 够，而且比多槽对。一帧里那八次读的输入是同一份，所以命中率是 7/8；输入真变了
/// （新消息、改筛选、敲搜索词）就该重算一遍 —— 那不是未命中，那是有新东西要算。
/// 多槽只会在「两组输入来回切」时多省一点，代价是要定淘汰策略，还要多留一份 2618 条
/// 的数组在内存里。本机现在最缺的恰恰是内存（现场读数：free 0.9 GB、交换只剩 798 MB）。
///
/// ## 线程
///
/// 只从主线程（body 求值）用。`@unchecked Sendable` + 锁是为了「万一被别处读到也不会
/// 撕裂」，不是为了支持并发写；锁里不调用 `resolve`，所以不会有人在锁上等着算 2618 条。
final class CrewTimelineFilterCache: @unchecked Sendable {
    private let lock = NSLock()
    private var cachedInputs: CrewTimelineFilter.Inputs?
    private var cachedOutput: [CrewWhiteboardEntry] = []

    init() {}

    func entries(for inputs: CrewTimelineFilter.Inputs) -> [CrewWhiteboardEntry] {
        lock.lock()
        if let cachedInputs, cachedInputs == inputs {
            let hit = cachedOutput
            lock.unlock()
            return hit
        }
        lock.unlock()

        // 刻意在锁外算：这一下可能要扫 2618 条、965 KB 正文。
        let fresh = CrewTimelineFilter.resolve(inputs)

        lock.lock()
        cachedInputs = inputs
        cachedOutput = fresh
        lock.unlock()
        return fresh
    }
}
