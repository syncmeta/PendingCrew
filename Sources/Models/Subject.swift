import Foundation

/// `GET /v1/me/subjects` 列表项。caller 当前可以代表 / 登录的 subject。
///
/// `kind` 当前是 `user_account` / `group_account` 二选一（subject_type
/// enum 见 spec v2 §4.2 + supabase 迁移 `subjects` 表）。client 用
/// 字符串透传，避免给 enum 加新 case 时旧 build 拒绝 decode。
struct UserSubject: Decodable, Identifiable, Equatable, Hashable {
    let id: String
    let kind: String
    let displayName: String
    /// 仅 `group_account` 行有值：`owner` / `admin`（spec v2 §4.3 限制只有
    /// 这两个角色能 "登录成群"）。
    let role: String?
    /// 登录这台机的 PendingCrew 用户(auth.users.id)。device grant 路径来自
    /// grant 的 `granted_by_user_id`,user-JWT 路径是 caller 自己。群聊用它把
    /// `CrewWhiteboardEntry.senderUserId`(= messages.user_id)等于本值的消息
    /// 右对齐成"自己的气泡"。BYOK 假 subject 没有真用户,留 nil。
    let userId: String?

    enum Kind: String {
        case userAccount = "user_account"
        case groupAccount = "group_account"
    }

    var kindEnum: Kind? { Kind(rawValue: kind) }
}
