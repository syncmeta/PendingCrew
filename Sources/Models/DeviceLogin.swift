import Foundation

/// PendingBot Edge `POST /v1/device-login/challenges` 的响应。
///
/// scaffold 阶段只用 `challengeId` + `secret` + `code` + `qrPayload` 几个
/// 字段；`expiresAt` 用来算超时。`status` 服务端固定返回 `"pending"`，
/// 但保留作为 schema 完整性。
struct DeviceLoginChallenge: Decodable, Equatable {
    let challengeId: String
    let secret: String
    let code: String
    let qrPayload: String
    let expiresAt: String   // ISO8601
    let status: String
}

/// `GET /v1/device-login/challenges/:id?secret=...` 的响应。
///
/// 在 pending / approved / consumed / expired / rejected 各状态间流转：
/// - `pending`：QR 还没人扫，继续 poll
/// - `approved`：PendingBot 用户已批准，**首次** GET 会 consume challenge
///   并返回 `deviceGrantToken`（之后再 GET 就 410 expired）
/// - `expired` / `rejected`：终态，UI 给出错误
///
/// `deviceGrantToken` 只在 approved 那一拍非空；其它状态都是 null。
///
/// `familyCredential`（`pfa_*`）同样只在 consume 那一拍随 grant 一起下发 ——
/// 存进共享 keychain 组（`FamilyCredentialStore`）供家族 app 静默 mint。
struct DeviceLoginPollResponse: Decodable, Equatable {
    let status: String
    let deviceGrantToken: String?
    let grantId: String?
    let subjectId: String?
    let grantKind: String?
    let scopes: [String]?
    let familyCredential: String?
    /// 批准者的展示名 / 头像 seed —— consume 一拍随凭据下发（旧 edge 无此
    /// 字段时 decode 不失败），存进家族凭据供侧栏身份区首帧显示真实身份。
    let displayName: String?
    let avatarSeed: String?
}
