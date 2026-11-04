import SwiftUI

struct CrewdChannelResponsibilityTarget: Identifiable {
    let crew: String
    let channel: String?
    let scope: UUID
    var id: UUID { scope }
}

struct CrewdCurrentTopicResponsibilityButton: View {
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject private var store: CrewdWorkStore
    @State private var target: CrewdChannelResponsibilityTarget?

    init(workspace: CrewdWorkspace) {
        self.workspace = workspace
        store = workspace.workStore
    }

    var body: some View {
        if store.supportsChannelResponsibility, store.selectedCrew == workspace.crewID,
           CrewdChannelResponsibilityPage.ownsActiveTopic(workspace.channels,crew:workspace.crewID,channel:workspace.channelID) {
            Button("本群工作归属") {
                target = .init(crew:workspace.crewID,channel:workspace.channelID,scope:store.readScope)
            }
            .font(.caption).padding(.horizontal,16).padding(.top,8)
            .frame(maxWidth:.infinity,alignment:.leading)
            .sheet(item:$target) { target in
                CrewdChannelResponsibilityView(workspace:workspace,store:store,target:target)
            }
            .onChange(of:workspace.channelID) { _,_ in target = nil }
            .onChange(of:store.readScope) { _,_ in target = nil }
        }
    }
}

/// A read-only observation with an explicit route to the existing Work form.
struct CrewdChannelResponsibilityView: View {
    private typealias Episode = CrewdChannelResponsibilityPage.Episode
    @ObservedObject var workspace: CrewdWorkspace
    @ObservedObject var store: CrewdWorkStore
    let target: CrewdChannelResponsibilityTarget
    @Environment(\.dismiss) private var dismiss
    @State private var active = false
    @State private var rows: [Episode] = []
    @State private var cursor: String?
    @State private var loaded = false
    @State private var requestID = UUID()
    @State private var reading: Task<Void,Never>?
    @State private var inspected: (episode: UUID, plan: CrewdPlan)?
    @State private var formScope = UUID()
    @State private var actionTarget: CrewdPlanTarget?
    @State private var error: String?
    @State private var refreshHint: String?

    private var current: Bool {
        active && store.supportsChannelResponsibility && workspace.client.readyForWork &&
            store.readScope == target.scope && store.selectedCrew == target.crew && workspace.crewID == target.crew &&
            (target.channel == nil || workspace.channelID == target.channel)
    }

    var body: some View {
        VStack(alignment:.leading,spacing:10) {
            HStack {
                Text(target.channel == nil ? "机组群工作归属" : "本群工作归属").font(.headline)
                Spacer()
                Button("重新读取") { read() }.disabled(!current || reading != nil)
                Button("关闭") { dismiss() }
            }
            if let refreshHint { Text(refreshHint).font(.caption).foregroundStyle(.secondary) }
            if let error { Text(error).font(.caption).foregroundStyle(.orange) }
            if reading != nil { ProgressView().controlSize(.small) }
            if current {
                ScrollView {
                    LazyVStack(alignment:.leading,spacing:14) {
                        if loaded && rows.isEmpty {
                            Text("当前范围暂无群工作归属记录。").foregroundStyle(.secondary)
                        }
                        ForEach(rows) { episode in facts(episode) }
                        if let cursor {
                            Button("读取下一页") { read(after:cursor) }.disabled(reading != nil)
                        }
                    }.frame(maxWidth:.infinity,alignment:.leading)
                }
            } else {
                Text("机组、会话或连接已变化，请从当前页面重新打开。").foregroundStyle(.secondary)
            }
        }
        .padding().modifier(CrewdAdaptiveFormFrame(width:570,height:465,scrolls:false))
        .onAppear { active = true; read() }
        .onDisappear { active = false; cancelRead(); invalidateForm() }
        .onChange(of:store.readScope) { _,_ in dismiss() }
        .onChange(of:workspace.channelID) { _,_ in if target.channel != nil { dismiss() } }
        .onReceive(workspace.authorityEvents) { event in changed(event.0,event.1) }
        .sheet(item:$actionTarget) { target in
            CrewdPlanActions(store:store,drafts:$workspace.planDrafts,target:target,
                            members:workspace.sessions,crews:workspace.crews)
        }
    }

    private func facts(_ episode: Episode) -> some View {
        VStack(alignment:.leading,spacing:5) {
            Text(verbatim:channelName(episode.channel)).font(.subheadline.weight(.semibold))
            Text(verbatim:episode.channel).font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            Text(phaseLabel(episode.phase)).fontWeight(.medium)
            Text("首次核实 " + time(episode.observedAt) + " · 最近记录 " + time(episode.updatedAt))
                .font(.caption).foregroundStyle(.secondary)
            Text("机长提醒 · " + noticeLabel(episode.notice)).font(.caption)
            if episode.phase == .open && episode.notice == .notQueued {
                Text("当前尚无可核对的机长提醒。").font(.caption).foregroundStyle(.secondary)
            }
            if let captain = episode.captain, let wake = episode.wake {
                Text(memberName(captain) + " · 通知 " + String(wake)).font(.caption).textSelection(.enabled)
            }
            if let evidence = episode.ownership {
                Text("当时核实 " + evidence.work.raw + " 的负责人为 " + memberName(evidence.owner))
                    .font(.caption.weight(.semibold)).textSelection(.enabled)
                Text("事件 " + evidence.event + " · " + time(evidence.at))
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
            if episode.phase == .open {
                ForEach(episode.work,id:\.self) { id in
                    Button("打开计划 " + id.raw) { inspect(id,episode:episode) }.disabled(reading != nil || !current)
                    if let inspected, inspected.episode == episode.id, inspected.plan.id == id {
                        planDetails(inspected.plan)
                    }
                }
            } else {
                Text("当时观察的计划 " + episode.work.map(\.raw).joined(separator:"、"))
                    .font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            }
        }.frame(maxWidth:.infinity,alignment:.leading)
    }

    private func planDetails(_ plan: CrewdPlan) -> some View {
        VStack(alignment:.leading,spacing:6) {
            Divider()
            Text(plan.id.raw + " · " + (plan.value["title"].string ?? "")).font(.subheadline.weight(.semibold))
            Text("待指派 · 当前核对版本 " + plan.revision).font(.caption).foregroundStyle(.secondary)
            if let goal = plan.value["goal"].string, !goal.isEmpty {
                Text(verbatim:goal).textSelection(.enabled)
            }
            if let acceptance = plan.value["acceptance"].string, !acceptance.isEmpty {
                Text("验收条件").font(.caption).foregroundStyle(.secondary)
                Text(verbatim:acceptance).textSelection(.enabled)
            }
            Text("指派前会再次核对这份计划的版本。群与计划的关联单独维护。")
                .font(.caption).foregroundStyle(.secondary)
            Button("打开计划操作") { openPlanForm(plan) }
                .disabled(reading != nil || !current || store.plans[plan.id]?.revision != plan.revision)
        }.frame(maxWidth:.infinity,alignment:.leading)
    }

    private func channelName(_ channel: String) -> String {
        workspace.channels.first(where:{ $0["id"] == .string(channel) })?["name"].string ?? "协作群"
    }
    private func memberName(_ member: String) -> String {
        guard let name = workspace.sessions.first(where:{ $0["id"] == .string(member) })?["displayName"].string,
              !name.isEmpty, name != member else { return member }
        return name + " (" + member + ")"
    }
    private func time(_ ms: Int) -> String {
        Date(timeIntervalSince1970:Double(ms) / 1_000).formatted(date:.abbreviated,time:.shortened)
    }
    private func phaseLabel(_ phase: Episode.Phase) -> String {
        switch phase {
        case .open: "已核实存在待指派计划"
        case .handled: "当时已核实负责人"
        case .ended: "本次归属提醒已结束"
        case .stale: "原观察已过期，请核对当前计划"
        }
    }
    private func noticeLabel(_ notice: Episode.Notice) -> String {
        switch notice {
        case .notQueued: "尚未排队"
        case .queued: "已排队"
        case .inflight: "正在投递"
        case .delivered: "已送达"
        case .dropped: "投递未完成"
        case .unknown: "送达结果未知"
        }
    }

    private func cancelRead() {
        requestID = UUID(); reading?.cancel(); reading = nil
    }

    private func invalidateForm() {
        formScope = UUID(); inspected = nil; actionTarget = nil
    }

    private func changed(_ name: String, _ data: CrewdValue) {
        guard current else { return }
        if name == "wake.changed" {
            let ids = Set(data["wakeIds"].array.compactMap(\.int))
            if rows.contains(where:{ $0.wake.map(ids.contains) ?? false }) { refreshHint = "提醒状态有更新，可重新读取。" }
            return
        }
        guard name.hasPrefix("work.") || name == "channel.workChanged" || name == "channel.changed" ||
                name == "crew.changed" || name == "events.lagged" else { return }
        if let crew = data["crewId"].string, crew != target.crew { return }
        if let channel = target.channel, let changed = data["channelId"].string, changed != channel { return }
        let wasReading = reading != nil
        cancelRead(); invalidateForm()
        if loaded || wasReading {
            rows = []; cursor = nil; loaded = false
            refreshHint = "相关工作或群关联已变化，请重新读取。"
        }
    }

    private func run(_ operation: @escaping @MainActor (UUID) async throws -> Void) {
        cancelRead(); error = nil
        let token = requestID
        reading = Task { @MainActor in
            defer { if requestID == token { reading = nil } }
            do { try await operation(token) }
            catch is CancellationError {}
            catch { if current && requestID == token { self.error = error.localizedDescription } }
        }
    }

    private func read(after: String? = nil) {
        guard current else { return }
        if after == nil { rows = []; cursor = nil; loaded = false; refreshHint = nil; invalidateForm() }
        run { token in
            let page = try await store.channelResponsibility(crew:target.crew,channel:target.channel,after:after)
            guard current, requestID == token else { return }
            let prior = Set(rows.map(\.channel))
            guard page.items.allSatisfy({ !prior.contains($0.channel) }) else { throw CrewdChannelResponsibilityPage.invalid() }
            rows.append(contentsOf:page.items); cursor = page.next; loaded = true
        }
    }

    private func inspect(_ id: CrewdWorkID, episode: Episode) {
        guard current, rows.contains(episode) else { return }
        invalidateForm()
        run { token in
            let plan = try await store.channelResponsibilityWork(id,episode:episode,crew:target.crew)
            guard current, requestID == token, rows.contains(episode) else { return }
            inspected = (episode.id,plan)
        }
    }

    private func openPlanForm(_ plan: CrewdPlan) {
        guard current, inspected?.plan.id == plan.id, store.plans[plan.id]?.revision == plan.revision else { return }
        let key = CrewdPlanDrafts.Key(crew:target.crew,object:plan.id.raw,source:"")
        if !workspace.planDrafts.hasDraft(key), !workspace.planDrafts.uncertain(key) {
            var initial = workspace.planDrafts.fields(key)
            initial.action = .assign
            workspace.planDrafts.edit(key,initial)
        }
        let captured = formScope
        actionTarget = CrewdPlanTarget(key:key,scopeIsCurrent:{
            current && formScope == captured && store.plans[plan.id]?.revision == plan.revision
        })
    }
}
