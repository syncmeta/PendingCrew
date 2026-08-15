import XCTest


final class CodexRPCMessageTests: XCTestCase {
    func testEncodeRequestOmitsJsonrpcAndIsSingleLine() throws {
        let line = try CodexRPCMessage.encodeRequest(id: 10, method: "thread/start", params: ["model": "gpt-5.5"])
        XCTAssertFalse(line.contains("\n"))
        XCTAssertFalse(line.contains("jsonrpc"))
        let obj = try JSONSerialization.jsonObject(with: Data(line.utf8)) as! [String: Any]
        XCTAssertEqual(obj["id"] as? Int, 10)
        XCTAssertEqual(obj["method"] as? String, "thread/start")
    }
    func testClassifyResponse() throws {
        let msg = try CodexRPCMessage.classify(line: #"{"id":10,"result":{"thread":{"id":"thr_1"}}}"#)
        guard case let .response(id, result, error) = msg else { return XCTFail("expected response") }
        XCTAssertEqual(id, 10); XCTAssertNil(error); XCTAssertNotNil(result)
    }
    func testClassifyServerRequest() throws {
        let msg = try CodexRPCMessage.classify(line: #"{"id":0,"method":"item/commandExecution/requestApproval","params":{}}"#)
        guard case let .serverRequest(id, method, _) = msg else { return XCTFail("expected serverRequest") }
        XCTAssertEqual(id, 0); XCTAssertEqual(method, "item/commandExecution/requestApproval")
    }
    func testClassifyNotification() throws {
        let msg = try CodexRPCMessage.classify(line: #"{"method":"turn/started","params":{}}"#)
        guard case let .notification(method, _) = msg else { return XCTFail("expected notification") }
        XCTAssertEqual(method, "turn/started")
    }
}
