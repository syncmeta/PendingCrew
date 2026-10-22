import Foundation
struct Payload: Codable, Equatable, Sendable { let text: String; let stagedMentions: [String]; let replyTarget: String? }
func expect(_ condition: @autoclosure () -> Bool, _ label: String) throws { if !condition() { throw NSError(domain: label, code: 1) }; print("PASS " + label) }
actor MemoryCloud: AppleBranchTransport {
    var partitions: [AppleSyncAccount: [UUID: AppleBranch]] = [:]
    var paused = false
    var gate: CheckedContinuation<Void, Never>?
    var entered = false
    func pause() { paused = true; entered = false }
    func release() { paused = false; gate?.resume(); gate = nil }
    func isEntered() -> Bool { entered }
    func count(_ account: AppleSyncAccount) -> Int { partitions[account]?.count ?? 0 }
    func exchange(account: AppleSyncAccount, collection: String, branches: [AppleBranch]) async throws -> [AppleBranch] {
        entered = true
        if paused { await withCheckedContinuation { gate = $0 } }
        var rows = partitions[account] ?? [:]
        for row in branches { rows[row.id] = try rows[row.id].map { try AppleBranch.merge($0, row) } ?? row }
        partitions[account] = rows
        return Array(rows.values)
    }
    func cancel() async { release() }
}
@main struct Checks {
    static func main() async throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1]); try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        func dir(_ name: String) -> URL { root.appendingPathComponent(name) }
        let a = AppleSyncAccount(container: "iCloud.test.explicit", environment: .development, userRecordID: "real-user-A")
        let b = AppleSyncAccount(container: a.container, environment: .production, userRecordID: "real-user-B")
        let key = AppleDraftKey(authorityID: "transport-authority", databaseID: "db/opaque", crewID: "crew", destination: .channel("channel"), kind: "message")
        let p = Payload(text: "原文", stagedMentions: ["@green"], replyTarget: "message-123")
        let q = Payload(text: "iPhone并发", stagedMentions: ["@blue"], replyTarget: nil)
        let mac = try AppleBranchStore<Payload>(root: dir("mac"), deviceID: "mac", account: a)
        let phone = try AppleBranchStore<Payload>(root: dir("phone"), deviceID: "phone", account: a)
        let cloud = MemoryCloud()
        let mid = try await mac.checkpoint(key: key, payload: p)
        _ = try await phone.checkpoint(key: key, payload: q)
        let reopened = try AppleBranchStore<Payload>(root: dir("mac"), deviceID: "mac", account: a)
        let read = await reopened.active(key: key)
        try expect(read.first?.payload == p, "offline reopen preserves structured payload")
        try await mac.synchronize(using: cloud); try await phone.synchronize(using: cloud); try await mac.synchronize(using: cloud)
        for i in 0..<32 { _ = try await phone.checkpoint(key: key, payload: Payload(text: "edit-\(i)", stagedMentions: [], replyTarget: nil)) }
        _ = try await phone.checkpoint(key: key, payload: q)
        try await phone.synchronize(using: cloud)
        let cloudCount = await cloud.count(a)
        try expect(cloudCount == 2, "checkpoints coalesce without permanent keystroke history")
        let both = await mac.active(key: key)
        try expect(Set(both.map(\.payload.text)) == Set([p.text, q.text]), "two devices preserve concurrent originals")
        let merged = Payload(text: "用户显式合并", stagedMentions: p.stagedMentions, replyTarget: p.replyTarget)
        _ = try await mac.resolve(key: key, payload: merged, retiring: Set(both.map(\.branchID)))
        try await mac.synchronize(using: cloud)
        // Phone still holds the old original, edits offline before seeing retirement.
        _ = try await phone.checkpoint(key: key, payload: Payload(text: "离线旧分支后续原文", stagedMentions: [], replyTarget: nil))
        try await phone.synchronize(using: cloud)
        let visible = await phone.active(key: key), recovered = await phone.recoveredOriginals()
        try expect(visible.count == 1 && visible[0].payload == merged, "tombstone prevents offline resurrection")
        try expect(recovered.values.contains(where: { $0.text == "离线旧分支后续原文" }), "retired offline original retained locally")
        let tombstoneReopen = try AppleBranchStore<Payload>(root: dir("phone"), deviceID: "phone", account: a)
        let recoveryRead = await tombstoneReopen.recoveredOriginals()
        try expect(!recoveryRead.isEmpty, "recovery and tombstones durable")
        await cloud.pause()
        let flight = Task { try await mac.synchronize(using: cloud) }
        while !(await cloud.isEntered()) { await Task.yield() }
        try await mac.selectAccount(b)
        do { try await flight.value; throw NSError(domain: "accepted late account response", code: 1) } catch AppleSyncFailure.accountChanged { print("PASS account switch rejects in-flight response") }
        let newAccount = await mac.active(key: key)
        try expect(newAccount.isEmpty, "new account and environment isolate old branches")
        try await mac.selectAccount(a)
        let originalAccount = await mac.active(key: key)
        try expect(originalAccount.first?.payload == merged, "old account local partition restored")
        let noCloud = try AppleBranchStore<Payload>(root: dir("local"), deviceID: "offline")
        _ = try await noCloud.checkpoint(key: key, payload: p)
        try await noCloud.synchronize(using: cloud)
        let localStatus = await noCloud.status
        try expect(localStatus == .localOnly, "no iCloud remains local only")
        let small = try AppleBranchStore<Payload>(root: dir("capacity"), deviceID: "mac", byteLimit: 128)
        do { _ = try await small.checkpoint(key: key, payload: p); throw NSError(domain: "capacity expected", code: 1) } catch AppleSyncFailure.capacity { }
        let retained = await small.active(key: key), error = await small.storageError
        try expect(retained.first?.payload == p && error != nil, "capacity retains original and visible failure")
        do { try await small.selectAccount(a); throw NSError(domain: "unsafe account switch", code: 1) } catch AppleSyncFailure.storage { print("PASS unsaved input blocks account discard") }
        let corruptRoot = dir("corrupt")
        let seed = try AppleBranchStore<Payload>(root: corruptRoot, deviceID: "mac", account: a)
        _ = try await seed.checkpoint(key: key, payload: p)
        let damaged = try FileManager.default.contentsOfDirectory(at: corruptRoot, includingPropertiesForKeys: nil).first!
        try Data("corrupt-original".utf8).write(to: damaged)
        let corrupt = try AppleBranchStore<Payload>(root: corruptRoot, deviceID: "mac", account: a)
        do { _ = try await corrupt.checkpoint(key: key, payload: q); throw NSError(domain: "corrupt expected", code: 1) } catch AppleSyncFailure.corrupt { }
        let preserved = try String(contentsOf: damaged, encoding: .utf8)
        try expect(preserved == "corrupt-original", "corrupt file never overwritten")
        let spki = String(repeating: "a", count: 64)
        for raw in ["wss://core.example.com:7443", "tls://core.example.com", "tls://core.example.com:0", "tls://localhost:7443", "tls://127.0.0.1:7443", "tls://[::1]:7443", "tls://0.0.0.0:7443", "tls://[::]:7443", "tls://[::ffff:127.0.0.1]:7443", "unix:///tmp/core.sock", "tls://user:secret@core.example.com:7443", "tls://core.example.com:7443/?token=secret", "tls://core.example.com:7443/private"] {
            do { _ = try ApplePortableHost(displayName: "host", endpoint: URL(string: raw)!, transportAuthorityID: "a", databaseID: "d", verifiedSPKI: spki); throw NSError(domain: "accepted " + raw, code: 1) } catch AppleSyncFailure.invalid { }
        }
        for raw in ["tls://154.36.143.228:7443", "tls://192.168.1.2:7443", "tls://[2001:db8::12]:7443", "tls://[fd12::1]:7443", "tls://mac.local:7443", "tls://macbook:7443"] {
            _ = try ApplePortableHost(displayName: "host", endpoint: URL(string: raw)!, transportAuthorityID: "a", databaseID: "d", verifiedSPKI: spki)
        }
        print("PASS portable directory permits public IP and explicit LAN hints")
        print("PASS portable directory rejects local paths and credentials")
        let host = try ApplePortableHost(displayName: "host", endpoint: URL(string: "tls://core.example.com:8443")!, transportAuthorityID: "a", databaseID: "d", verifiedSPKI: spki)
        var object = try JSONSerialization.jsonObject(with: appleSyncEncode(host)) as! [String: Any]
        object["privateKey"] = "secret"
        do { _ = try JSONDecoder().decode(ApplePortableHost.self, from: JSONSerialization.data(withJSONObject: object)); throw NSError(domain: "accepted unknown secret field", code: 1) } catch AppleSyncFailure.invalid { print("PASS directory rejects unknown private fields") }
        _ = mid
        print("ALL CHECKS PASSED; simulated transport only, no iCloud acceptance")
    }
}
