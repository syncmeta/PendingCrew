#if os(macOS)
import SwiftUI

/// Spec v2 §8.4 — "本机 agent = 完整登录用户权限" disclosure.
///
/// 在 PendingCrew **第一次**(每台机)启动时挡在前面,用户点"我明白了"后
/// 才允许进 RootView。状态写在 `UserDefaults`,key 是
/// `PendingCrew.firstLaunchDisclosureAccepted.v1`。
///
/// 为什么放在 `WindowGroup` 的 sheet 上而不是塞进 RootView:
/// - sheet 是 modal,用户没接受前点不到 Welcome / Login / 主界面
/// - 跟 macOS 系统的 "你想允许 X 访问 Y" 对话框心智一致
/// - 不写凭据 / 不写 disk,仅设 boolean —— 撤销很简单(`defaults delete`)
///
/// v1 后如果 disclosure 内容变了,bump suffix(`.v2`)让所有用户重看一次。
enum FirstLaunchDisclosure {
    static let acceptedKey = "PendingCrew.firstLaunchDisclosureAccepted.v1"

    static func isAccepted(defaults: UserDefaults = .standard) -> Bool {
        defaults.bool(forKey: acceptedKey)
    }

    static func markAccepted(defaults: UserDefaults = .standard) {
        defaults.set(true, forKey: acceptedKey)
    }
}

struct FirstLaunchDisclosureView: View {
    /// caller 把按"我明白了"后要跑的副作用塞进来(写 UserDefaults + 关 sheet)。
    let onAccept: () -> Void

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            body_
            Divider()
            footer
        }
        .frame(width: 520)
        .frame(minHeight: 420)
    }

    // MARK: - sections

    private var header: some View {
        HStack(spacing: 10) {
            Image(systemName: "exclamationmark.shield.fill")
                .font(.title)
                .foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                Text("启动前请阅读")
                    .font(.title3.weight(.semibold))
                Text("本机 agent 的权限模型")
                    .font(.callout)
                    .foregroundStyle(.secondary)
            }
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 16)
    }

    private var body_: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                paragraph(
                    title: "PendingCrew 不做沙盒",
                    body: "PendingCrew 在你这台 Mac 上起 Claude Code / Codex 子进程时, 不加任何系统级的沙盒、seatbelt、权限墙。这跟你自己打开终端跑 `claude` 或 `codex` 是同一回事。"
                )
                paragraph(
                    title: "= 你的完整登录用户权限",
                    body: "也就是说: agent 可以读你的 ~/.ssh、可以 `git push`、可以删文件、可以调任何你登录账户有权访问的 API。它能做的事 = 你能做的事。"
                )
                paragraph(
                    title: "权限管理由 agent 自己负责",
                    body: "细粒度的批准(\"是否允许这条命令\"、\"是否允许写这个文件\")由 Claude Code / Codex 自己的 permission 系统决定。PendingCrew 只是个遥控器, 不在系统层强行拦截。"
                )
                paragraph(
                    title: "PendingCrew 自己的凭据不外泄",
                    body: "子进程的环境变量是显式白名单(PATH/HOME/SHELL 等), PendingCrew 自己的 device grant token、Keychain 凭据**不会**透传给 agent。"
                )
            }
            .padding(20)
        }
    }

    private func paragraph(title: String, body: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(title)
                .font(.callout.weight(.semibold))
            Text(body)
                .font(.callout)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var footer: some View {
        HStack {
            Text("此提示仅展示一次。需重看可在终端跑 `defaults delete com.pendingname.pendingcrew \(FirstLaunchDisclosure.acceptedKey)`。")
                .font(.caption2)
                .foregroundStyle(.tertiary)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
            Button("我明白了") {
                onAccept()
            }
            .buttonStyle(.borderedProminent)
            .keyboardShortcut(.defaultAction)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }
}
#endif
