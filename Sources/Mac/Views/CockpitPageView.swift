#if os(macOS)
import SwiftUI

/// 期望页正文 —— 驾驶舱右栏那一半。
///
/// 原来这是「期望」段自己的右栏（左 handbook 树 / 右正文）。期望段并进路线段后
/// （人类原话「期望和 roadmap 合二为一，以 roadmap 为主」），正文这半抽出来复用：
/// 左边是路线地图，点到哪条期望页，右边就地渲哪一页，可就地编辑；现状那本以只读
/// 备注块跟在正文后（不再有独立的现状段）。
///
/// 编辑：保存原子写回磁盘真实文件，失败亮红字不吞错、草稿不丢；不自动 commit。
struct CockpitPageView: View {
    let data: CockpitData
    /// 要渲的期望页 relpath（无扩展名，同 CockpitTopic.expectationRelpath 坐标系）；
    /// `nil` = 还没选中任何条目（渲根 README 当「全书序」）。
    /// 带 `orphanPrefix` 前缀 = 未挂账现状（只有备注、没有期望页）。
    let relpath: String?
    let nav: (CockpitNav) -> Void

    /// 未挂账现状行的 id 前缀（handbook relpath 来自文件路径，不会撞）。
    static let orphanPrefix = "§orphan:"

    @State private var editingRelpath: String?
    @State private var draft = ""
    @State private var saveError: String?

    var body: some View {
        detail
            // 切页 = 放弃编辑（等同取消）：编辑器锚定进入时的文档，不跟随新选择。
            .onChange(of: relpath) { _, _ in
                editingRelpath = nil
                saveError = nil
            }
    }

    @ViewBuilder private var detail: some View {
        if let editing = editingRelpath {
            editorView(relpath: editing)
        } else if let id = relpath, id.hasPrefix(Self.orphanPrefix) {
            orphanBody(String(id.dropFirst(Self.orphanPrefix.count)))
        } else if let id = relpath {
            pageBody(relpath: id)
        } else if let body = CockpitLoader.readExpectation(handbookDir: data.handbookDir, relpath: "README") {
            // 全书序：根 README.md；没有就退回引导文案。
            article { editBar("README"); MarkdownText(text: body, variant: .article) }
        } else {
            placeholder("从左边的路线地图点开一条，正文渲在这里")
        }
    }

    /// 期望页正文 + 跟在后面的现状备注（现状不是独立段，以从属备注形式挂在这里，只读）。
    private func pageBody(relpath id: String) -> some View {
        article {
            if let body = CockpitLoader.readExpectation(handbookDir: data.handbookDir, relpath: id) {
                editBar(id)
                MarkdownText(text: body, variant: .article)
            } else {
                Text("这条路线上挂着 docs/handbook/\(id).md，但文件不存在 —— 期望页还没写。")
                    .font(Theme.Fonts.footnote)
                    .foregroundStyle(Theme.Palette.danger)
            }
            if let topic = data.topics.first(where: { $0.expectationRelpath == id }) {
                stateNote(topic)
            }
        }
    }

    /// 未挂账现状：有现状条、没有期望页（缺页本身就是要看见的信号）。
    @ViewBuilder private func orphanBody(_ topicID: String) -> some View {
        if let topic = data.topics.first(where: { $0.id == topicID }) {
            article {
                stateNote(topic)
                Text("这条现状没有对应的期望页（docs/handbook/\(topic.expectationRelpath).md 不存在）——未挂账，期望页补上后它会自动归位。")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
            }
        } else {
            placeholder("这条未挂账现状已不存在（刷新一下）")
        }
    }

    /// 现状备注块：status + 现状一句话 + 证据 + 关联 task 卡（跳任务段）。
    /// 从属视觉：灰底小字，不喧宾夺主；编辑只针对期望正文，这块始终只读。
    ///
    /// **「生 session 补差」按钮已撤**（人类 Todo #31 拍板）：它把这条现状的描述当 brief
    /// 直接派活，而现状账 `docs/state` 自 2026-07-26 起停更了两周 —— 等于拿两周前的描述
    /// 去派真活。撤掉不是不要这个能力，是**等现状账重新跟得上代码再接回来**；恢复参照
    /// 撤除前的实现（本文件 + `CockpitRoadmapView` 条目行的 ⚡ + `CockpitView.spawnBrief`）。
    private func stateNote(_ topic: CockpitTopic) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                Text("现状").font(.caption.weight(.semibold)).foregroundStyle(Theme.Palette.inkMuted)
                StatusBadge(raw: topic.status)
                Spacer(minLength: 0)
            }
            Text(topic.summary).font(.caption).foregroundStyle(Theme.Palette.ink)
            if !topic.evidence.isEmpty, topic.evidence != "—" {
                Text("证据：\(topic.evidence)").font(.caption2).foregroundStyle(Theme.Palette.inkMuted)
            }
            let tasks = topic.taskIds.compactMap { data.task($0) }
            if !tasks.isEmpty {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(tasks) { task in
                        Button { nav(.task(task.id)) } label: { CockpitTaskMiniCard(task: task) }
                            .buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(10)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.Palette.surfaceMuted, in: RoundedRectangle(cornerRadius: 8))
    }

    // MARK: 就地编辑

    /// 阅读态顶部的编辑入口行。只在文件真实存在的分支出现。
    private func editBar(_ relpath: String) -> some View {
        HStack {
            Spacer(minLength: 0)
            Button { beginEdit(relpath) } label: {
                Label("编辑", systemImage: "pencil").font(Theme.Fonts.caption)
            }
            .buttonStyle(.borderless)
            .foregroundStyle(Theme.Palette.inkMuted)
            .help("编辑 docs/handbook/\(relpath).md（保存写回文件，不自动 commit）")
        }
    }

    /// 编辑态：整个右栏换成 markdown 源文编辑器。
    private func editorView(relpath: String) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Text("编辑 docs/handbook/\(relpath).md")
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(Theme.Palette.inkMuted)
                    .lineLimit(1)
                    .truncationMode(.middle)
                Spacer(minLength: 8)
                Button("取消") {
                    editingRelpath = nil
                    saveError = nil
                }
                .controlSize(.small)
                Button("保存") { save(relpath) }
                    .buttonStyle(.borderedProminent)
                    .controlSize(.small)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
            if let saveError {
                Text(saveError)
                    .font(Theme.Fonts.caption)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 6)
                    .textSelection(.enabled)
            }
            Divider()
            TextEditor(text: $draft)
                .font(.system(size: 12.5, design: .monospaced))
                .scrollContentBackground(.hidden)
                .padding(10)
        }
        .background(Theme.Palette.canvas)
    }

    private func beginEdit(_ relpath: String) {
        guard let body = CockpitLoader.readExpectation(
            handbookDir: data.handbookDir, relpath: relpath) else {
            saveError = "读不到 docs/handbook/\(relpath).md，无法编辑。"
            return
        }
        draft = body
        saveError = nil
        editingRelpath = relpath
    }

    private func save(_ relpath: String) {
        let url = data.handbookDir.appendingPathComponent("\(relpath).md")
        do {
            try draft.write(to: url, atomically: true, encoding: .utf8)
            editingRelpath = nil
            saveError = nil
        } catch {
            // fail-loud：写盘失败留在编辑态亮红字，草稿不丢。
            saveError = "保存失败：\(error.localizedDescription)"
        }
    }

    private func article<Content: View>(@ViewBuilder _ content: () -> Content) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) { content() }
                .padding(18)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .background(Theme.Palette.canvas)
    }

    private func placeholder(_ t: String) -> some View {
        VStack { Spacer(); Text(t).foregroundStyle(Theme.Palette.inkMuted); Spacer() }
            .frame(maxWidth: .infinity, maxHeight: .infinity).background(Theme.Palette.canvas)
    }
}
#endif
