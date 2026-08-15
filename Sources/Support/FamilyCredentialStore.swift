import Foundation
import Security

/// 家族 SSO 凭据（`pfa_*`）+ 它被批准时绑定的 subject。
///
/// `subjectId` 一起存是为了静默登录知道默认 mint 给哪个 subject ——
/// 凭据签发那一刻用户在 PendingBot 里 approve 的就是这个 subject。
struct FamilyCredential: Codable, Equatable {
    let token: String        // pfa_*
    let subjectId: String    // 默认 mint 目标（个人主体）
    /// 同发布者确认卡展示用 —— PendingBot 写入侧在登录时带上。可选：旧 payload
    /// 无此字段时 decode 不失败（确认卡降级到通用文案，mint 后再回填真实身份）。
    var displayName: String?
    /// 用户自有头像的 seed（pendingbot.users.custom_fields.avatar_seed），
    /// 驱动确认卡的 BotAvatar 字形。可选，同上降级。
    var avatarSeed: String?
}

/// 家族 SSO 凭据存储 —— 落在**共享 keychain access group**。
///
/// 与 `KeychainStore`（app 私有组、存本 app 的 device-grant token）不同，这里
/// 用 `M42BKJN82S.com.pendingname.shared` 共享组：同 team（M42BKJN82S）下声明了
/// 该组 entitlement 的 app（PendingBot / PendingCrew / 后续家族成员）都能读写
/// 同一条凭据 —— 一个 app 登录后，其它 app 用它调 `POST /v1/device-grant/mint`
/// 静默换自己的 scoped device grant，免重复扫码。
///
/// 所有查询必设 `kSecUseDataProtectionKeychain: true`（同 `KeychainStore` 的
/// 理由：macOS 默认 legacy 登录钥匙串会反复弹授权框；且共享 access group 本身
/// 就只在 data-protection 钥匙串里生效）。
///
/// ⚠️ headless / ad-hoc 签名构建下，`keychain-access-groups` entitlement 不被
/// 系统认可，所有调用会得 `errSecMissingEntitlement`（-34018）→ get 返回 nil、
/// set/clear 静默失败。这是已知的真机验证尾巴：共享组读写只能在 Apple
/// Development 正常签名（Xcode GUI / 有证书环境）下验证。
enum FamilyCredentialStore {
    static let sharedGroup = "M42BKJN82S.com.pendingname.shared"
    static let service = "com.pendingname.family-sso"

    static func get() -> FamilyCredential? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccessGroup as String: sharedGroup,
            kSecUseDataProtectionKeychain as String: true,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess,
              let data = item as? Data,
              let cred = try? JSONDecoder().decode(FamilyCredential.self, from: data) else {
            return nil
        }
        return cred
    }

    static func set(_ cred: FamilyCredential) {
        guard let data = try? JSONEncoder().encode(cred) else { return }
        // delete-then-add：比 SecItemUpdate 分支简单，且共享组下 update 的
        // 匹配语义跨 app 容易踩坑；这条凭据写入频率极低，不在乎两次调用。
        clear()
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccessGroup as String: sharedGroup,
            kSecUseDataProtectionKeychain as String: true,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        SecItemAdd(add as CFDictionary, nil)
    }

    static func clear() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccessGroup as String: sharedGroup,
            kSecUseDataProtectionKeychain as String: true
        ]
        SecItemDelete(query as CFDictionary)
    }
}
