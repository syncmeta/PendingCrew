import XCTest


final class CodexRPCDispatcherTests: XCTestCase {

    func testResponseResolvesMatchingClientId() async throws {
        let d = CodexRPCDispatcher()
        async let r: Any? = d.awaitResponse(id: 10)
        try await Task.sleep(nanoseconds: 10_000_000)   // let the continuation register
        try await d.handle(.response(id: 10, result: ["ok": true], error: nil))
        let result = try await r as? [String: Any]
        XCTAssertEqual(result?["ok"] as? Bool, true)
    }

    func testServerRequestAndOurRequestShareIdZeroWithoutCollision() async throws {
        let d = CodexRPCDispatcher()
        let box = ServerReqBox()
        await d.setServerRequestHandler { id, method, _ in box.append((id, method)) }
        async let ours: Any? = d.awaitResponse(id: 0)
        try await Task.sleep(nanoseconds: 10_000_000)
        // A server-initiated request with id 0 must NOT resolve our pending id-0 continuation.
        try await d.handle(.serverRequest(id: 0, method: "item/fileChange/requestApproval", params: [:]))
        XCTAssertEqual(box.count, 1)
        XCTAssertEqual(box.first?.0, 0)
        // Now send a proper response for id 0 — this is what resolves our continuation.
        try await d.handle(.response(id: 0, result: ["done": 1], error: nil))
        let result = try await ours as? [String: Any]
        XCTAssertEqual(result?["done"] as? Int, 1)
    }

    func testNotificationForwarded() async throws {
        let d = CodexRPCDispatcher()
        let box = NoteBox()
        await d.setNotificationHandler { method, _ in box.append(method) }
        try await d.handle(.notification(method: "item/completed", params: [:]))
        XCTAssertEqual(box.all, ["item/completed"])
    }

    func testErrorResponseThrows() async {
        let d = CodexRPCDispatcher()
        async let r: Any? = d.awaitResponse(id: 5)
        try? await Task.sleep(nanoseconds: 10_000_000)
        try? await d.handle(.response(id: 5, result: nil, error: ["message": "boom"]))
        do { _ = try await r; XCTFail("expected throw") } catch { /* ok */ }
    }

    func testResponseArrivingBeforeAwaiterIsBuffered() async throws {
        let d = CodexRPCDispatcher()
        try await d.handle(.response(id: 7, result: ["v": 1], error: nil))   // arrives FIRST
        let result = try await d.awaitResponse(id: 7) as? [String: Any]      // awaiter comes after
        XCTAssertEqual(result?["v"] as? Int, 1)
    }
}

/// Tiny thread-safe boxes so handler closures can record without data-race warnings.
final class ServerReqBox: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [(Int, String)] = []
    func append(_ v: (Int, String)) { lock.lock(); items.append(v); lock.unlock() }
    var count: Int { lock.lock(); defer { lock.unlock() }; return items.count }
    var first: (Int, String)? { lock.lock(); defer { lock.unlock() }; return items.first }
}

final class NoteBox: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [String] = []
    func append(_ v: String) { lock.lock(); items.append(v); lock.unlock() }
    var all: [String] { lock.lock(); defer { lock.unlock() }; return items }
}
