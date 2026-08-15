import Foundation
import Security

/// PendingCrew 凭据存储。
///
/// **每个查询必设 `kSecUseDataProtectionKeychain: true`**（对齐 PendingBot
/// `DataProtectionKeychainStorage`）。原生 macOS(非 Catalyst)的 `SecItem` 默认落
/// legacy 登录钥匙串,ACL 绑签名二进制,每次重构二进制变 → 系统反复弹"PendingCrew
/// 想使用…裡儲存的機密資訊"要输登录钥匙串密码。data-protection 钥匙串是
/// entitlement-gated(靠 `keychain-access-groups`,默认落进 app 自己的
/// `$(AppIdentifierPrefix)com.pendingname.pendingcrew` 组),静默、跨重构稳定。
/// iOS/iPad 本就用它故从不弹;macOS 切过去会弃用一次旧登录钥匙串条目(重登录一次),
/// 之后静默。
enum KeychainStore {
    private static let service = "com.pendingname.pendingcrew"

    static func set(_ value: String, account: String) throws {
        let data = Data(value.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecUseDataProtectionKeychain as String: true
        ]
        let attrs: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        ]
        let status = SecItemUpdate(query as CFDictionary, attrs as CFDictionary)
        if status == errSecItemNotFound {
            var add = query
            add.merge(attrs) { _, new in new }
            let addStatus = SecItemAdd(add as CFDictionary, nil)
            if addStatus != errSecSuccess { throw KeychainError(status: addStatus) }
        } else if status != errSecSuccess {
            throw KeychainError(status: status)
        }
    }

    static func get(account: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecUseDataProtectionKeychain as String: true,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess,
              let data = item as? Data,
              let value = String(data: data, encoding: .utf8) else {
            return nil
        }
        return value
    }

    static func delete(account: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecUseDataProtectionKeychain as String: true
        ]
        SecItemDelete(query as CFDictionary)
    }
}

struct KeychainError: LocalizedError {
    let status: OSStatus

    var errorDescription: String? {
        let message = SecCopyErrorMessageString(status, nil) as String? ?? "unknown"
        return "Keychain error \(status): \(message)"
    }
}
