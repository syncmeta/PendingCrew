#if os(macOS)
import Foundation
import Security

/// Reads this running process's actual OS code identity. Displayed metadata
/// selects a bootstrap request; only the daemon's kernel/SecCode gate grants it.
struct CrewdLocalCodeIdentity {
    let identifier: String
    let cdHash: String
    let team: String?
    let hardened: Bool
    let publisherEligible: Bool
    static func current() throws -> Self {
        var code: SecCode?
        guard SecCodeCopySelf(SecCSFlags(rawValue: 0), &code) == errSecSuccess, let code else { throw failure() }
        var staticCode: SecStaticCode?
        guard SecCodeCopyStaticCode(code, SecCSFlags(rawValue: 0), &staticCode) == errSecSuccess, let staticCode else { throw failure() }
        var raw: CFDictionary?
        guard SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &raw) == errSecSuccess,
              let info = raw as? [String: Any],
              let identifier = info[kSecCodeInfoIdentifier as String] as? String,
              let hash = info[kSecCodeInfoUnique as String] as? Data, !hash.isEmpty else { throw failure() }
        let team = info[kSecCodeInfoTeamIdentifier as String] as? String
        let flags = (info[kSecCodeInfoFlags as String] as? NSNumber)?.uint32Value ?? 0
        let hardened = flags & 0x10000 != 0
        let entitlements = info[kSecCodeInfoEntitlementsDict as String] as? [String: Any] ?? [:]
        var eligible = false
        if identifier == "com.pendingname.pendingcrew", hardened,
           entitlements["com.apple.security.get-task-allow"] as? Bool != true,
           let team, !team.isEmpty, team.allSatisfy({ $0.isASCII && ($0.isNumber || $0.isUppercase) }) {
            let rule = "anchor apple generic and identifier \"com.pendingname.pendingcrew\" and certificate leaf[subject.OU] = \"\(team)\" and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
            var requirement: SecRequirement?
            if SecRequirementCreateWithString(rule as CFString, SecCSFlags(rawValue: 0), &requirement) == errSecSuccess,
               let requirement {
                eligible = SecCodeCheckValidity(code, SecCSFlags(rawValue: 0), requirement) == errSecSuccess
            }
        }
        return .init(identifier: identifier, cdHash: hash.map { String(format: "%02x", $0) }.joined(),
                     team: team, hardened: hardened, publisherEligible: eligible)
    }
    private static func failure() -> CrewdClientError { .init(message: "未能读取正在运行的App签名身份；未申请重连授权。", kind: .notSubmitted) }
}
#endif
