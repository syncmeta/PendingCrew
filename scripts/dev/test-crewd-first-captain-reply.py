#!/usr/bin/env python3
"""Compile the actual Swift first-reply projection and exercise its boundaries."""
import pathlib
import subprocess
import tempfile


root = pathlib.Path(__file__).resolve().parents[2]
workspace = (root / "Sources/Mac/Crewd/CrewdWorkspace.swift").read_text()
start = workspace.index("struct CrewdFirstCaptainReply: Equatable {")
end = workspace.index("\n@MainActor\nfinal class CrewdWorkspace", start)
checks = r"""
let channel = "crew-a/lobby"
let captain = "captain-a"
let original = "8915167d-bde2-4a97-aebb-725fe0b75298"
var progress = CrewdFirstCaptainReply(crewID: "crew-a", channelID: channel, messageID: original, firstSequence: 100)!
let human = CrewdFirstCaptainReply(crewID: "crew-a", channelID: "crew-a/human", messageID: original, firstSequence: 100)!
for source in [progress, human] {
    let target = "channel=\"\(source.channelID)\""
    let brief = source.startupBrief
    precondition(brief.contains("mcp__crew__read_channel（\(target), before=101, limit=1）"))
    precondition(brief.contains("#100"))
    precondition(brief.contains("mcp__crew__post，同时填写 \(target) 和 reply_to=\"\(original)\""))
    precondition(brief.contains("恢复已有会话时") && brief.contains("再次回复"))
}
precondition(CrewdFirstCaptainReply(crewID: "crew-a", channelID: "crew-b/human", messageID: original, firstSequence: 100) == nil)
precondition(CrewdFirstCaptainReply(crewID: "crew-a", channelID: channel, messageID: "invalid", firstSequence: 100) == nil)
precondition(CrewdFirstCaptainReply(crewID: "crew-a", channelID: channel, messageID: original, firstSequence: 0) == nil)
precondition(CrewdFirstCaptainReply(crewID: "crew-a", channelID: channel, messageID: original, firstSequence: Int.max) == nil)
precondition(progress.phase == .starting)
precondition(!progress.isReply(channel: channel, senderKind: "captain", sender: captain, sequence: 101))

for phase in [CrewdFirstCaptainReply.Phase.registered(captain), .launched(captain)] {
    progress.phase = phase
    precondition(!progress.isReply(channel: "crew-a/dm", senderKind: "captain", sender: captain, sequence: 101))
    precondition(!progress.isReply(channel: "crew-b/lobby", senderKind: "captain", sender: captain, sequence: 101))
    precondition(!progress.isReply(channel: channel, senderKind: "captain", sender: "captain-b", sequence: 101))
    precondition(!progress.isReply(channel: channel, senderKind: "worker", sender: captain, sequence: 101))
    precondition(!progress.isReply(channel: channel, senderKind: "system", sender: nil, sequence: 101))
    precondition(!progress.isReply(channel: channel, senderKind: "captain", sender: captain, sequence: 100))
    precondition(!progress.isReply(channel: channel, senderKind: "captain", sender: captain, sequence: nil))
    precondition(progress.isReply(channel: channel, senderKind: "captain", sender: captain, sequence: 101))
}
print("PASS original conversation and captain, strictly after the first message")
"""

with tempfile.TemporaryDirectory(prefix="pcw-first-captain-reply-") as directory:
    source = pathlib.Path(directory) / "main.swift"
    binary = pathlib.Path(directory) / "probe"
    source.write_text("import Foundation\n" + workspace[start:end] + checks)
    subprocess.run(
        ["xcrun", "swiftc", "-module-cache-path", str(source.parent / "cache"), str(source), "-o", str(binary)],
        check=True,
    )
    subprocess.run([str(binary)], check=True)
