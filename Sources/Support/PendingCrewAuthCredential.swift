import Foundation

/// PendingCrew 认证凭据。
///
/// `deviceGrant`：登录态，扫码后从 PendingBot Edge 拿到的 device grant token。
/// token 前缀 `pdg_`。走 unified billing → subject 钱包。
struct PendingCrewAuthCredential: Equatable {
    enum Kind: Equatable {
        case deviceGrant
    }

    let kind: Kind
    let token: String

    static func resolveDeviceGrant(_ token: String?) -> PendingCrewAuthCredential? {
        guard let token = clean(token), token.hasPrefix("pdg_") else { return nil }
        return PendingCrewAuthCredential(kind: .deviceGrant, token: token)
    }

    private static func clean(_ value: String?) -> String? {
        guard let value else { return nil }
        let token = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return token.isEmpty ? nil : token
    }
}
