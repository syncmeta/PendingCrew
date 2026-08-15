import Foundation

// Headless swiftc harness for LocalCrewControlStore — mirrors the XCTest cases in
// LocalCrewControlStoreTests.swift as `precondition` checks so a failing check exits
// nonzero. Exists because the app's XCTest bundle has a known @testable-import build-order
// issue; this file + scripts/swiftc-check-controlstore.sh is the authoritative RED/GREEN
// evidence for this file's TDD cycle. Kept in lockstep with the XCTest file by hand.

func tempDir() -> URL {
    let d = FileManager.default.temporaryDirectory.appendingPathComponent("crewctl-\(UUID().uuidString)")
    try? FileManager.default.createDirectory(at: d, withIntermediateDirectories: true)
    return d
}

func checkRequestThenPeek() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.requestRename(crewId: "local-abc", name: "鉴权重构")
    precondition(s.pendingRename(crewId: "local-abc") == "鉴权重构", "requestThenPeek")
}

func checkRequestTrimsAndRejectsEmpty() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.requestRename(crewId: "c", name: "  深色模式  ")
    precondition(s.pendingRename(crewId: "c") == "深色模式", "trims")
    s.requestRename(crewId: "blank", name: "   \n  ")
    precondition(s.pendingRename(crewId: "blank") == nil, "rejectsEmpty")
}

func checkDrainReturnsAndDeletes() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.requestRename(crewId: "local-abc", name: "语音重连")
    let drained = s.drainRenames()
    precondition(drained.count == 1, "drainCount")
    precondition(drained[0].crewId == "local-abc", "drainCrewId")
    precondition(drained[0].title == "语音重连", "drainTitle")
    precondition(s.pendingRename(crewId: "local-abc") == nil, "drainDeletesPeek")
    precondition(s.drainRenames().isEmpty, "drainDeletesDrain")
}

func checkLastWriteWins() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.requestRename(crewId: "c", name: "旧名")
    s.requestRename(crewId: "c", name: "新名")
    let drained = s.drainRenames()
    precondition(drained.count == 1, "lastWriteWinsCount")
    precondition(drained[0].title == "新名", "lastWriteWinsTitle")
}

func checkCrewsIsolated() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.requestRename(crewId: "local-a", name: "甲")
    s.requestRename(crewId: "local-b", name: "乙")
    let map = Dictionary(uniqueKeysWithValues: s.drainRenames().map { ($0.crewId, $0.title) })
    precondition(map["local-a"] == "甲", "isolatedA")
    precondition(map["local-b"] == "乙", "isolatedB")
}

func checkPersistsAcrossInstances() {
    let dir = tempDir()
    LocalCrewControlStore(directory: dir).requestRename(crewId: "c", name: "持久")
    precondition(LocalCrewControlStore(directory: dir).pendingRename(crewId: "c") == "持久", "persists")
}

func checkEnqueueStartSessionThenDrain() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.enqueueStartSession(crewId: "local-a", brief: "修登录", runner: "claude", isolation: true)
    let cmds = s.drainCommands()
    precondition(cmds.count == 1, "enqueueStartSessionCount")
    precondition(cmds[0].kind == "start_session", "enqueueStartSessionKind")
    precondition(cmds[0].crewId == "local-a", "enqueueStartSessionCrewId")
    precondition(cmds[0].brief == "修登录", "enqueueStartSessionBrief")
    precondition(cmds[0].runner == "claude", "enqueueStartSessionRunner")
    precondition(cmds[0].isolation == true, "enqueueStartSessionIsolation")
    precondition(s.drainCommands().isEmpty, "enqueueStartSessionDrainedAgain")
}

func checkEnqueueMultipleCommandsAllDrained() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.enqueueStartSession(crewId: "c", brief: "活一", runner: nil, isolation: nil)
    s.enqueueStartSession(crewId: "c", brief: "活二", runner: "codex", isolation: false)
    s.enqueueCreateChildCrew(
        crewId: "c", sessionId: "captain-parent", brief: "拆一块", title: "支付")
    let cmds = s.drainCommands()
    precondition(cmds.count == 3, "multiCount")
    precondition(cmds.filter { $0.kind == "start_session" }.count == 2, "multiStartSessionCount")
    precondition(cmds.filter { $0.kind == "create_child_crew" }.count == 1, "multiCreateChildCrewCount")
    precondition(cmds.first { $0.kind == "create_child_crew" }?.title == "支付", "multiTitle")
}

func checkEnqueueRejectsEmptyBrief() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.enqueueStartSession(crewId: "c", brief: "   \n ", runner: nil, isolation: nil)
    s.enqueueCreateChildCrew(
        crewId: "c", sessionId: "captain-parent", brief: "", title: nil)
    precondition(s.drainCommands().isEmpty, "rejectsEmptyBrief")
}

func checkCommandsAndRenamesDoNotInterfere() {
    let s = LocalCrewControlStore(directory: tempDir())
    s.requestRename(crewId: "c", name: "起个名")
    s.enqueueStartSession(crewId: "c", brief: "干活", runner: nil, isolation: nil)
    precondition(s.drainCommands().count == 1, "interfereDrainCommands1")
    precondition(s.pendingRename(crewId: "c") == "起个名", "interfereRenameUntouched")
    s.enqueueStartSession(crewId: "c", brief: "再干", runner: nil, isolation: nil)
    precondition(s.drainRenames().count == 1, "interfereDrainRenames")
    precondition(s.drainCommands().count == 1, "interfereDrainCommands2")
}

@main
struct LocalCrewControlStoreMain {
    static func main() {
        checkRequestThenPeek()
        checkRequestTrimsAndRejectsEmpty()
        checkDrainReturnsAndDeletes()
        checkLastWriteWins()
        checkCrewsIsolated()
        checkPersistsAcrossInstances()
        checkEnqueueStartSessionThenDrain()
        checkEnqueueMultipleCommandsAllDrained()
        checkEnqueueRejectsEmptyBrief()
        checkCommandsAndRenamesDoNotInterfere()

        print("controlstore checks OK")
    }
}
