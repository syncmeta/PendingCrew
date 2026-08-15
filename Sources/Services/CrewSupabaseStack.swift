import Foundation
import Supabase

/// supabase-swift AuthLocalStorage 的内存实现 —— session 永不落 keychain/磁盘。
struct InMemoryAuthStorage: AuthLocalStorage {
    let backing: InMemoryKeyValueStore
    func store(key: String, value: Data) throws { backing.set(key, value) }
    func retrieve(key: String) throws -> Data? { backing.get(key) }
    func remove(key: String) throws { backing.remove(key) }
}

/// PendingCrew 专用 Supabase client —— 内存态 auth storage，session 不持久化。
/// 仅用于直接登录路径（Apple/Google/邮箱）拿一次性 session 调
/// /v1/me/family-credential，用完即 signOut(.local)。
enum CrewSupabaseStack {
    static let shared: SupabaseClient = {
        SupabaseClient(
            supabaseURL: CrewHostedConfig.supabaseURL,
            supabaseKey: CrewHostedConfig.supabasePublishableKey,
            options: SupabaseClientOptions(
                db: SupabaseClientOptions.DatabaseOptions(schema: "pendingbot"),
                auth: SupabaseClientOptions.AuthOptions(
                    storage: InMemoryAuthStorage(backing: InMemoryKeyValueStore())
                )
            )
        )
    }()
}
