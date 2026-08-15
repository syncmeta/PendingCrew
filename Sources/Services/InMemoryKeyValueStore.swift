import Foundation

/// 纯内存 KV —— 可单测，不依赖 Supabase。供 InMemoryAuthStorage 承载 session 数据，
/// 进程退出即蒸发。
final class InMemoryKeyValueStore: @unchecked Sendable {
    private var dict: [String: Data] = [:]
    private let lock = NSLock()
    func get(_ key: String) -> Data? { lock.lock(); defer { lock.unlock() }; return dict[key] }
    func set(_ key: String, _ value: Data) { lock.lock(); defer { lock.unlock() }; dict[key] = value }
    func remove(_ key: String) { lock.lock(); defer { lock.unlock() }; dict[key] = nil }
}
