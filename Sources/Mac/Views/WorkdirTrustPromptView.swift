#if os(macOS)
import SwiftUI
import AppKit

/// 「这个目录还没被信任」的那个提示框。**全 app 只有这一个** ——
/// 建 crew（`CreateCrewSheet`）和迁移完成（`ChangeWorkingDirectorySheet`）弹的是它，
/// 不是两份长得像的东西：写两遍的那天，两份会各自漂，而漂了没有任何读数会报警。
///
/// 它只做两件事：把 `WorkdirTrustPrompt` 算出来的那几句话摆出来，
/// 把带着真实路径的命令给人一键复制。**它自己不判断、不写任何文件。**
struct WorkdirTrustPromptView: View {
    let prompt: WorkdirTrustPrompt.Prompt
    let onDone: () -> Void

    @State private var copied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label(WorkdirTrustPrompt.title, systemImage: "lock.open.trianglebadge.exclamationmark")
                .font(.headline)

            ForEach(Array(WorkdirTrustPrompt.sentences(prompt).dropLast().enumerated()),
                    id: \.offset) { _, line in
                Text(line).font(.callout).fixedSize(horizontal: false, vertical: true)
            }

            GroupBox {
                HStack(alignment: .top, spacing: 10) {
                    Text(prompt.commandBlock)
                        .font(.system(.callout, design: .monospaced))
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Button(copied ? "已复制" : "复制") { copy() }
                        .buttonStyle(.borderless)
                }
                .padding(4)
            }

            if let tail = WorkdirTrustPrompt.sentences(prompt).last {
                Text(tail).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            HStack {
                Spacer()
                Button("知道了") { onDone() }
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(20)
        .frame(width: 520)
    }

    private func copy() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(prompt.commandBlock, forType: .string)
        copied = true
    }
}
#endif
