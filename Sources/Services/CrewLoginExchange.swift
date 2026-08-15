import Foundation

/// 直接登录兑换：临时 session 的 access token → 家族凭据 pfa_ → mint pdg_ →
/// 写共享组 + 存 grant → 清 session。无论成败都 await teardown（守「不持久化全权 session」）。
struct CrewLoginExchange {
    struct FamilyResult {
        let token: String
        let subjectId: String
        let displayName: String?
        var avatarSeed: String? = nil
    }
    enum Failure: Error { case mintFailed }

    var issueFamily: (_ accessToken: String) async throws -> FamilyResult
    var mintGrant: (_ familyToken: String, _ subjectId: String) async throws -> String  // pdg_
    var saveGrant: (String) throws -> Void
    var saveFamily: (FamilyCredential) -> Void
    var teardown: () async -> Void

    func run(accessToken: String) async throws {
        do {
            let fam = try await issueFamily(accessToken)
            saveFamily(FamilyCredential(
                token: fam.token,
                subjectId: fam.subjectId,
                displayName: fam.displayName,
                avatarSeed: fam.avatarSeed
            ))
            let grant = try await mintGrant(fam.token, fam.subjectId)
            try saveGrant(grant)
            await teardown()
        } catch {
            await teardown()
            throw error
        }
    }
}
