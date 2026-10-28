import SwiftUI

/// Browses only the captured authority. Server paths never pass through local URL APIs.
@MainActor
final class CrewdDirectoryBrowser: ObservableObject {
    struct Entry: Identifiable {
        let name: String
        let path: String
        var id: String { name }
    }
    @Published private(set) var path: String?
    @Published private(set) var parentPath: String?
    @Published private(set) var entries: [Entry] = []
    @Published private(set) var nextAfter: String?
    @Published private(set) var loading = false
    @Published private(set) var failure: String?
    let workspace: CrewdWorkspace
    private let epoch: UUID
    private let generation: UUID
    private var requestID = UUID()
    private var task: Task<Void, Never>?

    init(workspace: CrewdWorkspace) {
        self.workspace = workspace
        epoch = workspace.client.connectionEpoch
        generation = workspace.observationGeneration
    }
    var current: Bool {
        workspace.client.connectionEpoch == epoch && workspace.acceptsObservation(generation)
    }
    var canChoose: Bool { current && !loading && failure == nil && path != nil }
    func cancel() { requestID = UUID(); task?.cancel(); task = nil; loading = false }
    func load(_ requestedPath: String? = nil, more: Bool = false) {
        cancel()
        guard current else { failure = "运行环境已变化，请重新选择目录。"; return }
        guard workspace.client.capabilities.contains("hostDirectoryBrowse") else {
            failure = "这台电脑的后台尚不支持目录浏览，请更新后台，或取消后手动填写完整路径。"
            return
        }
        let token = requestID
        var params: [String: CrewdValue] = [:]
        if let requestedPath { params["path"] = .string(requestedPath) }
        if more, let nextAfter { params["after"] = .string(nextAfter) }
        if !more { path = nil; parentPath = nil; entries = []; nextAfter = nil }
        loading = true; failure = nil
        task = Task {
            do {
                let value = try await workspace.client.call("host.directoryList", .object(params))
                guard !Task.isCancelled, token == requestID, current else { return }
                guard let canonical = value["path"].string, canonical.hasPrefix("/") else {
                    throw CrewdClientError(message: "后台未返回有效的目录路径。", kind: .notSubmitted)
                }
                let rows = value["entries"].array.compactMap { item -> Entry? in
                    guard let name = item["name"].string, let path = item["path"].string else { return nil }
                    return Entry(name: name, path: path)
                }
                path = canonical; parentPath = value["parentPath"].string
                entries = more ? entries + rows : rows
                nextAfter = value["nextAfter"].string
                loading = false
            } catch {
                guard !Task.isCancelled, token == requestID, current else { return }
                loading = false; failure = error.localizedDescription
            }
        }
    }
}

struct CrewdDirectoryPicker: View {
    @Environment(\.dismiss) private var dismiss
    @StateObject private var browser: CrewdDirectoryBrowser
    @State private var manualPath = ""
    let onChoose: (String) -> Void

    init(workspace: CrewdWorkspace, onChoose: @escaping (String) -> Void) {
        _browser = StateObject(wrappedValue: CrewdDirectoryBrowser(workspace: workspace))
        self.onChoose = onChoose
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("选择工作目录").font(.headline)
            Text("浏览当前运行环境中的文件夹").font(.caption).foregroundStyle(.secondary)
            HStack {
                Button { browser.load() } label: { Label("主目录", systemImage: "house") }
                Button { if let parent = browser.parentPath { browser.load(parent) } } label: {
                    Label("上级", systemImage: "arrow.up")
                }.disabled(browser.parentPath == nil || browser.loading)
                Spacer()
                if browser.loading { ProgressView().controlSize(.small) }
            }
            HStack {
                TextField("完整目录路径", text: $manualPath).onSubmit { navigate() }
                    #if os(iOS)
                    .textInputAutocapitalization(.never).autocorrectionDisabled()
                    #endif
                Button("前往") { navigate() }.disabled(manualPath.isEmpty)
            }
            if let failure = browser.failure {
                Text(failure).font(.caption).foregroundStyle(.orange).textSelection(.enabled)
            }
            List {
                ForEach(browser.entries) { entry in
                    Button { browser.load(entry.path) } label: {
                        HStack { Label(entry.name, systemImage: "folder"); Spacer(); Image(systemName: "chevron.right").foregroundStyle(.secondary) }
                            .contentShape(Rectangle())
                    }.buttonStyle(.plain).disabled(browser.loading)
                }
                if browser.nextAfter != nil {
                    Button("载入更多文件夹") { browser.load(browser.path, more: true) }.disabled(browser.loading)
                }
                if !browser.loading && browser.failure == nil && browser.entries.isEmpty {
                    Text("此目录没有子文件夹").foregroundStyle(.secondary)
                }
            }.listStyle(.plain)
            Text(browser.path ?? "尚未选择目录").font(.caption).foregroundStyle(.secondary).textSelection(.enabled)
            HStack {
                Spacer()
                Button("取消") { browser.cancel(); dismiss() }.keyboardShortcut(.cancelAction)
                Button("选择此目录") {
                    guard browser.canChoose, let path = browser.path else { return }
                    onChoose(path); browser.cancel(); dismiss()
                }.keyboardShortcut(.defaultAction).disabled(!browser.canChoose)
            }
        }.padding(20)
        #if os(macOS)
        .frame(width: 560, height: 480)
        #endif
        .onAppear { browser.load() }
        .onDisappear { browser.cancel() }
        .onChange(of: browser.path) { _, path in if let path { manualPath = path } }
        .onReceive(browser.workspace.client.$readyForWork) { ready in
            if !ready { browser.cancel(); dismiss() }
        }
    }
    private func navigate() {
        guard !manualPath.isEmpty else { return }
        browser.load(manualPath)
    }
}
