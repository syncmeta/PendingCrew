import Foundation

/// 确认卡展示模型 —— 从共享 keychain 的家族凭据派生。profile 缺失时降级到
/// 通用文案 + 用 subjectId 当头像 seed（mint 成功后 CrewStore.refreshSubjects
/// 会用 /v1/me/subject 的真实 display_name 回填真实身份）。
struct CrewLoginIdentity: Equatable {
    let title: String
    let avatarSeed: String
    let isGeneric: Bool

    init(credential: FamilyCredential) {
        if let name = credential.displayName, !name.isEmpty {
            self.title = name
            self.avatarSeed = credential.avatarSeed ?? credential.subjectId
            self.isGeneric = false
        } else {
            self.title = "本机 PendingBot 账号"
            self.avatarSeed = credential.avatarSeed ?? credential.subjectId
            self.isGeneric = true
        }
    }

    /// nil = 共享组无凭据（不显示确认卡）。
    static func current() -> CrewLoginIdentity? {
        FamilyCredentialStore.get().map(CrewLoginIdentity.init)
    }
}
