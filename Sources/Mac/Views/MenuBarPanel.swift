#if os(macOS)
import AppKit
import SwiftUI

/// 菜单栏点开之后的那一小块（P5b·B，人类 Todo #7）。
///
/// 人类给的用途很窄，照着做别扩：**不开主窗口也能看到有没有事在等他拍板，点一下进去。**
/// 所以这里只有两样东西：等你的是哪几件、一个「打开 PendingCrew」。
/// 不放列表、不放操作 —— 真要处理，进主窗口。
///
/// （曾经还有一个「开机自启」开关，2026-09-07 人类明确否掉常驻方向后一并删了。）
struct MenuBarPanel: View {
    @ObservedObject var attention: MenuBarAttentionModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if attention.count.isQuiet {
                Text("没有等你拍板的事")
                    .foregroundStyle(.secondary)
            } else {
                ForEach(attention.count.lines, id: \.self) { line in
                    Text(line)
                }
            }

            if let staleReason = attention.staleReason {
                // **读不动的时候必须说出来。** 数字还是上一次的 —— 不说的话，
                // 人看到的是一个看起来很新、其实早就停住的数字。
                Divider()
                Text(staleAgeText())
                    .font(.caption)
                    .foregroundStyle(.orange)
                Text(staleReason)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }

            Divider()

            Button("打开 PendingCrew") {
                NSApp.activate(ignoringOtherApps: true)
                for window in NSApp.windows where window.canBecomeMain {
                    window.makeKeyAndOrderFront(nil)
                    break
                }
            }

            Divider()
            Button("退出 PendingCrew") { NSApp.terminate(nil) }
        }
        .padding(12)
        .frame(width: 300, alignment: .leading)
    }

    private func staleAgeText() -> String {
        guard let at = attention.lastGoodAt else { return "还没读到过账 —— 下面这个数不作数。" }
        let seconds = Int(Date().timeIntervalSince(at))
        return "读不到账了，下面这个数是 \(seconds) 秒前的。"
    }

}
#endif
