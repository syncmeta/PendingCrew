import XCTest

final class CaptainBriefDeliveryTests: XCTestCase {
    func testBriefTravelsInAutostartPayloadAndOpeningPrompt() {
        let request = CaptainAutostartRequest(
            id: UUID(uuidString: "00000000-0000-0000-0000-000000000041")!,
            crewId: "child-1",
            brief: "检查机长自知力并落地修复",
            sourceCrewId: "parent-1",
            childTitle: "机长自知力")

        XCTAssertEqual(request.brief, "检查机长自知力并落地修复")
        let prompt = CaptainBriefDelivery.openingPrompt(
            brief: request.brief, wakeText: nil)
        XCTAssertTrue(prompt.contains("父机长交给你的开场任务"))
        XCTAssertTrue(prompt.contains("检查机长自知力并落地修复"))
        XCTAssertTrue(prompt.contains("立即推进"))
    }

    func testNonemptyBriefStartFailureProducesParentReceipt() {
        let request = CaptainAutostartRequest(
            crewId: "child-1",
            brief: "修复更新链",
            sourceCrewId: "parent-1",
            childTitle: "应用自动更新")

        let receipt = request.deliveryFailureReceipt(reason: "子机长启动失败：未找到 codex")
        XCTAssertEqual(receipt?.crewId, "parent-1")
        XCTAssertEqual(
            receipt?.text,
            "子 crew「应用自动更新」已建出来，但开场任务没送到（子机长启动失败：未找到 codex）。请手动转达。")
    }

    func testOrdinaryCrewAutostartDoesNotInventParentFailureReceipt() {
        let request = CaptainAutostartRequest(
            crewId: "manual-1", brief: nil, sourceCrewId: nil, childTitle: "手建 crew")
        XCTAssertNil(request.deliveryFailureReceipt(reason: "启动失败"))
    }
}
