#!/usr/bin/env python3
"""Compile the actual Swift first-reply projection and exercise its boundaries."""
import pathlib
import subprocess
import tempfile


root = pathlib.Path(__file__).resolve().parents[2]
workspace = (root / "Sources/Mac/Crewd/CrewdWorkspace.swift").read_text()
start = workspace.index("struct CrewdFirstCaptainReply: Equatable {")
end = workspace.index("\n@MainActor\nfinal class CrewdWorkspace", start)
client = (root / "Sources/Mac/Crewd/CrewdClient.swift").read_text()
wire_start = client.index("indirect enum CrewdValue:")
wire_end = client.index("\nstruct CrewdClientError:", wire_start)
observer_start = workspace.index("    private func observeFirstCaptainReply(")
observer_end = workspace.index("\n    @discardableResult func startCaptainForCreatedCrew", observer_start)
observer = workspace[observer_start:observer_end]
event = next(line.strip() for line in workspace.splitlines()
             if 'name == "message.appended"' in line and "observeFirstCaptainReply(data)" in line)
list_start = workspace.index("            messages = rows.array", workspace.index('client.call("message.list"'))
list_end = workspace.index("            messages.forEach(observeFirstCaptainReply)", list_start)
timeline = workspace[list_start:list_end + len("            messages.forEach(observeFirstCaptainReply)")]
surface = f"""
final class FirstReplySurface {{
    var firstCaptainReplies: [String: CrewdFirstCaptainReply] = [:]
    var messages: [CrewdValue] = []

{observer}
    func receiveEvent(_ frame: CrewdValue) {{
        let name = frame["event"].string
        let data = frame["data"]
        {event}
    }}

    func receiveMessageList(_ rows: CrewdValue) {{
{timeline}
    }}
}}
"""
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

func message(channel: String = channel, senderKind: String = "captain",
             sender: String? = captain, sequence: Int64? = 101,
             reply: CrewdValue? = .string(original)) -> CrewdValue {
    var row: [String: CrewdValue] = ["channelId": .string(channel), "senderKind": .string(senderKind)]
    if let sender { row["senderSessionId"] = .string(sender) }
    if let sequence { row["seq"] = .integer(sequence) }
    if let reply { row["inReplyTo"] = reply }
    return .object(row)
}

func wire(_ value: CrewdValue) throws -> CrewdValue {
    try JSONDecoder().decode(CrewdValue.self, from: JSONEncoder().encode(value))
}

let ineligible = [
    message(reply: nil),
    message(reply: .null),
    message(reply: .integer(17)),
    message(reply: .string("6fc22183-606d-410c-966e-d55d70e66cf8")),
    message(channel: "crew-a/dm"),
    message(channel: "crew-b/lobby"),
    message(sender: "captain-b"),
    message(senderKind: "worker"),
    message(senderKind: "system", sender: nil),
    message(sequence: 100),
    message(sequence: nil),
]
let eligible = message()
for phase in [CrewdFirstCaptainReply.Phase.starting, .registered(captain), .launched(captain)] {
    progress.phase = phase

    let live = FirstReplySurface()
    live.firstCaptainReplies[channel] = progress
    live.receiveEvent(try wire(.object(["event": .string("unrelated"), "data": eligible])))
    precondition(live.firstCaptainReplies[channel] == progress)
    for row in ineligible {
        live.receiveEvent(try wire(.object(["event": .string("message.appended"), "data": row])))
        precondition(live.firstCaptainReplies[channel] == progress, "an unrelated live message cleared first-reply progress")
    }
    live.receiveEvent(try wire(.object(["event": .string("message.appended"), "data": eligible])))
    precondition(live.firstCaptainReplies[channel] == (phase == .starting ? progress : nil))

    let readback = FirstReplySurface()
    readback.firstCaptainReplies[channel] = progress
    readback.receiveMessageList(try wire(.array(ineligible)))
    precondition(readback.firstCaptainReplies[channel] == progress, "an unrelated list row cleared first-reply progress")
    readback.receiveMessageList(try wire(.array(ineligible + [eligible])))
    precondition(readback.firstCaptainReplies[channel] == (phase == .starting ? progress : nil))
}
print("PASS exact original reply through both live event and message-list observation")

"""

with tempfile.TemporaryDirectory(prefix="pcw-first-captain-reply-") as directory:
    source = pathlib.Path(directory) / "main.swift"
    binary = pathlib.Path(directory) / "probe"
    source.write_text("import Foundation\n" + client[wire_start:wire_end] + workspace[start:end] + surface + checks)
    subprocess.run(
        ["xcrun", "swiftc", "-module-cache-path", str(source.parent / "cache"), str(source), "-o", str(binary)],
        check=True,
    )
    subprocess.run([str(binary)], check=True)
