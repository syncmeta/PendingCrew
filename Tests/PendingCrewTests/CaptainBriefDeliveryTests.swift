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
        XCTAssertEqual(prompt, "检查机长自知力并落地修复")
        XCTAssertFalse(prompt.contains("先用 post_to_crew"))
        XCTAssertFalse(prompt.contains("有人在群里 @ 你"))
    }

    func testTaskAndWakeOriginalBytesStayAheadOfAutomaticInstructions() {
        let task="  检查 Todo #42，保留原任务。\n"
        let wake="\n@机长 请核对原文  "
        XCTAssertEqual(CaptainBriefDelivery.openingPrompt(brief:task,wakeText:nil),task)
        XCTAssertEqual(CaptainBriefDelivery.openingPrompt(brief:task,wakeText:wake),task+"\n群聊消息：\n"+wake)
        XCTAssertEqual(CaptainBriefDelivery.openingPrompt(brief:nil,wakeText:wake),wake)
        XCTAssertEqual(CaptainBriefDelivery.openingPrompt(brief:"你是用户手写的原任务",wakeText:nil),"你是用户手写的原任务")
        XCTAssertFalse(CaptainBriefDelivery.openingPrompt(brief:nil,wakeText:nil).hasPrefix("你是"))
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
