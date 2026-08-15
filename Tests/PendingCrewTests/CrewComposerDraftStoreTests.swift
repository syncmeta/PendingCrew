import XCTest
import Foundation
// 待测源码直接编进 test bundle（见 project.yml），不 import app module。

/// 每 crew 草稿（Todo #45 的配套）：切 crew 视图整个重建，未发出的字得有地方待着。
@MainActor
final class CrewComposerDraftStoreTests: XCTestCase {

    private func target(_ id: String, sender: String = "机长") -> CrewReplyTarget {
        CrewReplyTarget(mention: CrewMention.captain,
                        replyToId: id, quotedSender: sender, quotedSnippet: "原话…")
    }

    /// 没记过的 crew 取出来是空草稿，不是 nil 崩溃、也不是别人的字。
    func testUnknownCrewYieldsAnEmptyDraft() {
        let store = CrewComposerDraftStore()
        XCTAssertEqual(store.draft(for: "crew-a"), CrewComposerDraftStore.Draft())
        XCTAssertTrue(store.draft(for: "crew-a").isEmpty)
    }

    /// 核心：A 群打一半 → 去 B 群 → 回 A 群，字还在，且**没串到 B 群**。
    /// （不重建视图时这两件事一起坏：B 群里出现 A 群的字，可能误发。）
    func testDraftsAreKeptPerCrewAndDoNotLeakAcrossThem() {
        let store = CrewComposerDraftStore()
        store.save(.init(text: "写了一半的话"), for: "crew-a")
        XCTAssertEqual(store.draft(for: "crew-b").text, "", "别的群不该看到 A 群的草稿")
        XCTAssertEqual(store.draft(for: "crew-a").text, "写了一半的话", "切回来原样在")
    }

    /// 文本、已挂上的 @、正在回复谁是一体的：只留文本会让恢复出来的 `@某人` 变成一串
    /// 没有目标的字（看着 @ 了，发出去谁也没 @ 到）。
    func testDraftKeepsMentionsAndReplyTargetTogetherWithTheText() {
        let store = CrewComposerDraftStore()
        let staged = [CrewStagedMention(token: "@机长",
                                        mention: CrewMention.captain)]
        store.save(.init(text: "@机长 帮我看下", stagedMentions: staged,
                         replyTarget: target("msg-1")), for: "crew-a")
        let back = store.draft(for: "crew-a")
        XCTAssertEqual(back.text, "@机长 帮我看下")
        XCTAssertEqual(back.stagedMentions, staged)
        XCTAssertEqual(back.replyTarget?.replyToId, "msg-1")
    }

    /// 只挂着一个回复目标、一个字没打，也算有草稿 —— 引用条得跟着回来。
    func testAReplyTargetAloneStillCountsAsADraft() {
        let store = CrewComposerDraftStore()
        store.save(.init(replyTarget: target("msg-9")), for: "crew-a")
        XCTAssertFalse(store.draft(for: "crew-a").isEmpty)
        XCTAssertEqual(store.draft(for: "crew-a").replyTarget?.replyToId, "msg-9")
    }

    /// 打了又删光 = 没有草稿，别让那个 crew 永久占着一格。
    func testTypingThenClearingLeavesNothingBehind() {
        let store = CrewComposerDraftStore()
        store.save(.init(text: "临时想说的"), for: "crew-a")
        store.save(.init(text: "   \n "), for: "crew-a")
        XCTAssertTrue(store.draft(for: "crew-a").isEmpty, "只剩空白 = 没打算说什么")
    }

    /// 发送成功后清掉 —— 发出去了就不是草稿，切回来不该又冒出来一遍。
    func testSendingClearsTheDraft() {
        let store = CrewComposerDraftStore()
        store.save(.init(text: "发出去了", replyTarget: target("msg-1")), for: "crew-a")
        store.clear("crew-a")
        XCTAssertTrue(store.draft(for: "crew-a").isEmpty)
    }
}
