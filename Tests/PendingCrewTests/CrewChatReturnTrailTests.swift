#if os(macOS)
import XCTest
// Sources compiled directly into the test bundle (see project.yml) — no module import needed.

/// 引用胶囊跳走之后回去的那条路（人类 Todo #132/#133 里人类点名的判据：
/// 「跳过去要能回来，别做成单程票」）。
final class CrewChatReturnTrailTests: XCTestCase {

    private func stop(_ crew: String, _ message: String) -> CrewChatReturnTrail.Stop {
        .init(crewId: crew, crewTitle: crew + " 群", messageId: message)
    }

    func test_没跳过就没有返回件() {
        XCTAssertTrue(CrewChatReturnTrail().isEmpty)
        XCTAssertNil(CrewChatReturnTrail().top)
    }

    func test_跳走再退回原来那条() {
        var trail = CrewChatReturnTrail()
        trail.push(stop("a", "m1"))
        XCTAssertEqual(trail.top, stop("a", "m1"))
        XCTAssertEqual(trail.pop(), stop("a", "m1"))
        XCTAssertTrue(trail.isEmpty)
    }

    func test_连着跳几层是一层层退回去() {
        var trail = CrewChatReturnTrail()
        trail.push(stop("a", "m1"))
        trail.push(stop("b", "m2"))
        XCTAssertEqual(trail.pop(), stop("b", "m2"))
        // 只留一个落点的实现在这里会把 a 丢掉 —— 那正是「退回去发现回不到最初」。
        XCTAssertEqual(trail.pop(), stop("a", "m1"))
    }

    func test_同一个落点连压两次只算一层() {
        var trail = CrewChatReturnTrail()
        trail.push(stop("a", "m1"))
        trail.push(stop("a", "m1"))
        XCTAssertEqual(trail.stops.count, 1)
        // 多出一层的症状是「按了返回没反应」—— 其实是退到了同一个地方。
        XCTAssertNotNil(trail.pop())
        XCTAssertNil(trail.pop())
    }

    func test_压过头只留最近的那些层() {
        var trail = CrewChatReturnTrail()
        for i in 0...(CrewChatReturnTrail.limit + 5) { trail.push(stop("a", "m\(i)")) }
        XCTAssertEqual(trail.stops.count, CrewChatReturnTrail.limit)
        XCTAssertEqual(trail.top, stop("a", "m\(CrewChatReturnTrail.limit + 5)"))
        // 掉的必须是最旧的那几层，不是最新的。
        XCTAssertEqual(trail.stops.first, stop("a", "m6"))
    }

    func test_人自己走开时整条路作废() {
        var trail = CrewChatReturnTrail()
        trail.push(stop("a", "m1"))
        trail.push(stop("b", "m2"))
        trail.clear()
        XCTAssertTrue(trail.isEmpty)
    }
}
#endif
