import XCTest
// CrewComposerMentions + CrewModels 直接编进 PendingCrewTests target，无需 import。

/// composer @-mention 纯逻辑单测(Phase 6 单元 1/4):候选构建+过滤、
/// 活动 @token 检测、token 插入、staged 撤销对账、reply 自动@推导。
final class CrewComposerMentionsTests: XCTestCase {

    // MARK: - fixtures

    private func member(
        id: String,
        kind: String,
        userId: String? = nil,
        sessionId: String? = nil,
        name: String?
    ) -> CrewMember {
        CrewMember(
            id: id, memberKind: kind, userId: userId, botId: nil,
            codeSessionId: sessionId, displayName: name, role: nil,
            status: "active", representsCrewId: nil, sessionStatus: nil)
    }

    private func entry(
        id: String = "e1",
        senderKind: String,
        sessionId: String? = nil,
        userId: String? = nil,
        text: String = "hi",
        displayName: String? = nil,
        inReplyTo: String? = nil
    ) -> CrewWhiteboardEntry {
        CrewWhiteboardEntry(
            id: id, senderKind: senderKind, senderSessionId: sessionId,
            senderUserId: userId, senderBotId: nil, messageKind: "instruction",
            summary: text, createdAt: "2026-06-15T00:00:00Z",
            payload: nil, attachments: nil, relay: nil,
            senderDisplayName: displayName, senderMemberId: nil, inReplyTo: inReplyTo,
            mentions: nil)
    }

    // MARK: - candidates

    func testCandidatesIncludeBroadcastCaptainSessionsHumansInOrder() {
        let members = [
            member(id: "s1", kind: "code_session", sessionId: "sess-aaa", name: "小绿"),
            member(id: "h1", kind: "human", userId: "u1", name: "Alice"),
        ]
        let cands = crewMentionCandidates(
            members: members, captainBotId: "bot-cap", prefix: "", selfMemberId: nil)
        XCTAssertEqual(cands.map(\.kind),
                       [.broadcast, .captain, .session, .human])
        // broadcast / captain mentions
        XCTAssertEqual(cands[0].mention, .broadcast)
        XCTAssertEqual(cands[1].mention, .captain)
        XCTAssertEqual(cands[2].mention, .session("sess-aaa"))
        XCTAssertEqual(cands[3].mention, CrewMention(kind: "human", targetId: "u1"))
    }

    func testCandidatesDropCaptainWhenNoneAndSkipSelfHuman() {
        let members = [
            member(id: "me", kind: "human", userId: "u-me", name: "我"),
            member(id: "h1", kind: "human", userId: "u1", name: "Alice"),
        ]
        let cands = crewMentionCandidates(
            members: members, captainBotId: nil, prefix: "", selfMemberId: "me")
        // no captain candidate; self human skipped → broadcast + Alice
        XCTAssertEqual(cands.map(\.label), ["全体", "Alice"])
    }

    func testCandidatePrefixFilterIsCaseInsensitiveSubstring() {
        let members = [
            member(id: "s1", kind: "code_session", sessionId: "sess-aaa", name: "小绿"),
            member(id: "h1", kind: "human", userId: "u1", name: "Alice"),
        ]
        // prefix "al" matches "Alice" (case-insensitive), not 小绿/全体/机长.
        let cands = crewMentionCandidates(
            members: members, captainBotId: "bot", prefix: "al", selfMemberId: nil)
        XCTAssertEqual(cands.map(\.label), ["Alice"])
    }

    func testCandidateTokenIsAtLabel() {
        let members = [member(id: "s1", kind: "code_session", sessionId: "sess-aaa", name: "小绿")]
        let cands = crewMentionCandidates(
            members: members, captainBotId: nil, prefix: "小", selfMemberId: nil)
        XCTAssertEqual(cands.first?.token, "@小绿")
    }

    // MARK: - activeQuery detection

    func testActiveQueryAtTail() {
        let q = CrewComposerMentionParser.activeQuery(in: "帮我看下 @小")
        XCTAssertEqual(q?.prefix, "小")
    }

    func testActiveQueryEmptyPrefixRightAfterAt() {
        let q = CrewComposerMentionParser.activeQuery(in: "hi @")
        XCTAssertEqual(q?.prefix, "")
    }

    func testActiveQueryClosedBySpaceIsNil() {
        XCTAssertNil(CrewComposerMentionParser.activeQuery(in: "hi @小绿 在吗"))
    }

    func testActiveQueryMidWordAtIsNotMention() {
        // email-like "a@b" — the @ doesn't follow whitespace → not a mention.
        XCTAssertNil(CrewComposerMentionParser.activeQuery(in: "mail a@b"))
    }

    func testActiveQueryAtStartOfDraft() {
        let q = CrewComposerMentionParser.activeQuery(in: "@cap")
        XCTAssertEqual(q?.prefix, "cap")
        XCTAssertEqual(q?.atOffset, 0)
    }

    // MARK: - insert

    func testInsertReplacesOpenTokenAndStages() {
        let cand = CrewMentionCandidate(
            id: "session:sess-aaa", kind: .session, label: "小绿",
            mention: .session("sess-aaa"))
        let ins = CrewComposerMentionParser.insert(candidate: cand, into: "帮我看下 @小")
        XCTAssertEqual(ins?.newDraft, "帮我看下 @小绿 ")
        XCTAssertEqual(ins?.staged, CrewStagedMention(token: "@小绿", mention: .session("sess-aaa")))
    }

    func testInsertWithNoOpenTokenReturnsNil() {
        let cand = CrewMentionCandidate(
            id: "broadcast", kind: .broadcast, label: "全体", mention: .broadcast)
        XCTAssertNil(CrewComposerMentionParser.insert(candidate: cand, into: "已经发完了 "))
    }

    // MARK: - reconcile (staged 撤销)

    func testReconcileKeepsPresentTokensDropsDeleted() {
        let staged = [
            CrewStagedMention(token: "@小绿", mention: .session("s1")),
            CrewStagedMention(token: "@机长", mention: .captain),
        ]
        // user deleted "@机长" from the draft.
        let kept = CrewComposerMentionParser.reconcile(staged: staged, draft: "@小绿 跑测试")
        XCTAssertEqual(kept, [CrewStagedMention(token: "@小绿", mention: .session("s1"))])
    }

    func testReconcileDuplicateTokensMatchedByCount() {
        let staged = [
            CrewStagedMention(token: "@小绿", mention: .session("s1")),
            CrewStagedMention(token: "@小绿", mention: .session("s1")),
        ]
        // only one "@小绿" left in the draft → keep exactly one staged entry.
        let kept = CrewComposerMentionParser.reconcile(staged: staged, draft: "@小绿 在吗")
        XCTAssertEqual(kept.count, 1)
    }

    // MARK: - appendMention (右键头像 → @，无 open token)

    func testAppendMentionToEmptyDraft() {
        let staged = CrewStagedMention(token: "@机长", mention: .captain)
        let r = CrewComposerMentionParser.appendMention(staged, to: "", existing: [])
        XCTAssertEqual(r.newDraft, "@机长 ")
        XCTAssertEqual(r.staged, [staged])
    }

    func testAppendMentionInsertsLeadingSpaceWhenNeeded() {
        let staged = CrewStagedMention(token: "@小绿", mention: .session("s1"))
        let r = CrewComposerMentionParser.appendMention(staged, to: "帮我看下", existing: [])
        XCTAssertEqual(r.newDraft, "帮我看下 @小绿 ")
        XCTAssertEqual(r.staged, [staged])
    }

    func testAppendMentionNoDoubleSpaceAfterTrailingSpace() {
        let staged = CrewStagedMention(token: "@小绿", mention: .session("s1"))
        let r = CrewComposerMentionParser.appendMention(staged, to: "帮我看下 ", existing: [])
        XCTAssertEqual(r.newDraft, "帮我看下 @小绿 ")
    }

    func testAppendMentionDedupsSameTargetToNoOp() {
        let staged = CrewStagedMention(token: "@小绿", mention: .session("s1"))
        // 已经 @ 过同一 session（token 文本可不同）→ 原样返回，不叠加。
        let existing = [CrewStagedMention(token: "@小绿 (会话)", mention: .session("s1"))]
        let r = CrewComposerMentionParser.appendMention(staged, to: "@小绿 (会话) ", existing: existing)
        XCTAssertEqual(r.newDraft, "@小绿 (会话) ")
        XCTAssertEqual(r.staged, existing)
    }

    func testAppendMentionReclaimsHalfOpenAt() {
        // 先敲 @ 触发 picker（draft 末尾是裸 @），再右键头像 → 不该出 "@ @机长"。
        let staged = CrewStagedMention(token: "@机长", mention: .captain)
        let r = CrewComposerMentionParser.appendMention(staged, to: "帮我看下 @", existing: [])
        XCTAssertEqual(r.newDraft, "帮我看下 @机长 ")
        XCTAssertEqual(r.staged, [staged])
    }

    func testAppendMentionReclaimsHalfOpenAtWithPrefix() {
        // 末尾是半开 "@ji"（打了几个字母）→ 截掉再拼，不该出 "@ji @机长"。
        let staged = CrewStagedMention(token: "@机长", mention: .captain)
        let r = CrewComposerMentionParser.appendMention(staged, to: "@ji", existing: [])
        XCTAssertEqual(r.newDraft, "@机长 ")
        XCTAssertEqual(r.staged, [staged])
    }

    func testAppendMentionNormalizesDoubleAtFromSelfPrefixedName() {
        // 作者名自带前导 @（如 relay 远端名）→ token 归一化到单个 @，不出 "@@Name"。
        let staged = CrewStagedMention(token: "@@Name", mention: .session("s1"))
        let r = CrewComposerMentionParser.appendMention(staged, to: "", existing: [])
        XCTAssertEqual(r.newDraft, "@Name ")
        XCTAssertEqual(r.staged.first?.token, "@Name")
    }

    func testMentionsToSendDeduplicates() {
        let staged = [
            CrewStagedMention(token: "@小绿", mention: .session("s1")),
            CrewStagedMention(token: "@小绿", mention: .session("s1")),
            CrewStagedMention(token: "@全体", mention: .broadcast),
        ]
        let out = CrewComposerMentionParser.mentionsToSend(staged)
        XCTAssertEqual(out, [.session("s1"), .broadcast])
    }

    // MARK: - reply target derivation

    func testReplyToSessionAutoMentionsThatSession() {
        let e = entry(id: "msg-7", senderKind: "session", sessionId: "sess-xyz", text: "我跑完了")
        let t = CrewReplyTargetBuilder.make(entry: e, senderName: "小绿")
        XCTAssertEqual(t.mention, .session("sess-xyz"))
        XCTAssertEqual(t.replyToId, "msg-7")
        XCTAssertEqual(t.quotedSender, "小绿")
        XCTAssertEqual(t.quotedSnippet, "我跑完了")
        XCTAssertEqual(t.staged, CrewStagedMention(token: "@小绿", mention: .session("sess-xyz")))
    }

    func testReplyToHumanAutoMentionsHuman() {
        let e = entry(senderKind: "user", userId: "u-bob", text: "看下这个")
        let t = CrewReplyTargetBuilder.make(entry: e, senderName: "Bob")
        XCTAssertEqual(t.mention, CrewMention(kind: "human", targetId: "u-bob"))
    }

    func testReplyToBotHasNoAutoMentionButKeepsReplyTo() {
        let e = entry(id: "msg-b", senderKind: "bot", text: "机长发言")
        let t = CrewReplyTargetBuilder.make(entry: e, senderName: "机长")
        XCTAssertNil(t.mention)
        XCTAssertNil(t.staged)
        XCTAssertEqual(t.replyToId, "msg-b")
    }

    func testReplySnippetCollapsesAndTruncates() {
        let long = String(repeating: "字", count: 200)
        let e = entry(senderKind: "user", userId: "u1", text: "第一行\n第二行 " + long)
        let t = CrewReplyTargetBuilder.make(entry: e, senderName: "X")
        XCTAssertFalse(t.quotedSnippet.contains("\n"))
        XCTAssertTrue(t.quotedSnippet.hasSuffix("…"))
        XCTAssertLessThanOrEqual(t.quotedSnippet.count, CrewReplyTargetBuilder.snippetLimit + 1)
    }

    // MARK: - reply reference resolution (#377 — 气泡里渲染被回复引用)

    func testReplyReferenceResolvesSenderAndSnippetFromLoadedEntries() {
        let original = entry(id: "orig", senderKind: "session", sessionId: "sess-xyz",
                             text: "我先开了个头")
        let reply = entry(id: "rep", senderKind: "user", userId: "u-me",
                          text: "收到，我接着干", inReplyTo: "orig")
        let members = [
            member(id: "s1", kind: "code_session", sessionId: "sess-xyz", name: "小绿"),
        ]
        let ref = CrewReplyReferenceResolver.resolve(
            for: reply, in: [original, reply],
            members: members, captainBotId: nil, localUserId: "u-me")
        XCTAssertNotNil(ref)
        XCTAssertTrue(ref!.found)
        XCTAssertEqual(ref!.senderName, "小绿")
        XCTAssertEqual(ref!.snippet, "我先开了个头")
    }

    func testReplyReferenceNilWhenEntryIsNotAReply() {
        let e = entry(id: "e", senderKind: "user", userId: "u1", text: "普通消息")
        let ref = CrewReplyReferenceResolver.resolve(
            for: e, in: [e], members: [], captainBotId: nil, localUserId: nil)
        XCTAssertNil(ref)
    }

    func testReplyReferenceDegradesWhenTargetNotLoaded() {
        // inReplyTo points outside the loaded window → placeholder, never crash.
        let reply = entry(id: "rep", senderKind: "user", userId: "u-me",
                          text: "回复一条没加载的", inReplyTo: "off-window")
        let ref = CrewReplyReferenceResolver.resolve(
            for: reply, in: [reply], members: [], captainBotId: nil, localUserId: "u-me")
        XCTAssertNotNil(ref)
        XCTAssertFalse(ref!.found)
        XCTAssertEqual(ref, CrewReplyReference.notLoaded)
        XCTAssertEqual(ref!.snippet, "回复了一条消息")
    }

    func testReplyReferenceSnippetCollapsesAndTruncates() {
        let long = String(repeating: "字", count: 200)
        let original = entry(id: "orig", senderKind: "user", userId: "u1",
                             text: "头一行\n第二行 " + long)
        let reply = entry(id: "rep", senderKind: "user", userId: "u2",
                          text: "ok", inReplyTo: "orig")
        let ref = CrewReplyReferenceResolver.resolve(
            for: reply, in: [original, reply],
            members: [], captainBotId: nil, localUserId: nil)!
        XCTAssertFalse(ref.snippet.contains("\n"))
        XCTAssertTrue(ref.snippet.hasSuffix("…"))
        XCTAssertLessThanOrEqual(ref.snippet.count, CrewReplyReferenceResolver.snippetLimit + 1)
    }
}
