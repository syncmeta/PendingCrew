#if os(macOS)
import Foundation

/// One JSON-RPC line for the codex app-server, classified by shape. `jsonrpc`
/// is omitted on the wire. Routing: has `method`+`id` → server request (reply
/// required); has `method`, no `id` → notification; has `id`, no `method` →
/// response to one of our requests.
enum CodexRPCMessage {
    case response(id: Int, result: Any?, error: [String: Any]?)
    case serverRequest(id: Int, method: String, params: [String: Any])
    case notification(method: String, params: [String: Any])

    static func encodeRequest(id: Int, method: String, params: [String: Any]) throws -> String {
        try encode(["id": id, "method": method, "params": params])
    }
    static func encodeNotification(method: String, params: [String: Any]) throws -> String {
        try encode(["method": method, "params": params])
    }
    static func encodeResponse(id: Int, result: [String: Any]) throws -> String {
        try encode(["id": id, "result": result])
    }
    /// Error reply to a server-initiated request we don't model — keeps codex from
    /// blocking the turn waiting on a response we'd otherwise never send.
    static func encodeError(id: Int, code: Int, message: String) throws -> String {
        try encode(["id": id, "error": ["code": code, "message": message]])
    }
    private static func encode(_ obj: [String: Any]) throws -> String {
        let data = try JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
        return String(decoding: data, as: UTF8.self)
    }
    static func classify(line: String) throws -> CodexRPCMessage {
        let obj = (try JSONSerialization.jsonObject(with: Data(line.utf8))) as? [String: Any] ?? [:]
        let method = obj["method"] as? String
        let id = obj["id"] as? Int
        let params = obj["params"] as? [String: Any] ?? [:]
        if let method, let id { return .serverRequest(id: id, method: method, params: params) }
        if let method { return .notification(method: method, params: params) }
        if let id { return .response(id: id, result: obj["result"], error: obj["error"] as? [String: Any]) }
        throw CodexRPCError.malformed(line)
    }
}

enum CodexRPCError: Error, Equatable { case malformed(String) }
#endif
