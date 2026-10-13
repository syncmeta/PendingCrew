import Foundation
import AppKit
import SwiftUI

func wire(_ text:String)->CrewdValue {try! JSONDecoder().decode(CrewdValue.self,from:Data(text.utf8))}
@main struct NativeRender {
    @MainActor static func main() async throws {
        guard CommandLine.arguments.count == 2 else {fatalError("explicit isolated output directory required")}
        let output=URL(fileURLWithPath:CommandLine.arguments[1],isDirectory:true)
        NSApplication.shared.setActivationPolicy(.prohibited)
        let workspace=CrewdWorkspace(dataDirectory:output.appendingPathComponent("unused-data"))
        workspace.crews=wire("[{\"id\":\"engineering\",\"title\":\"开发机组\",\"parentIds\":[],\"childIds\":[\"review\"],\"position\":0},{\"id\":\"review\",\"title\":\"验收机组\",\"parentIds\":[\"engineering\"],\"childIds\":[],\"position\":1}]").array
        workspace.sessions=wire("[{\"id\":\"captain\",\"crewId\":\"engineering\",\"displayName\":\"机长\",\"role\":\"captain\"},{\"id\":\"reviewer\",\"crewId\":\"engineering\",\"displayName\":\"验收员\",\"role\":\"worker\"}]").array
        workspace.channels=wire("[{\"id\":\"group\",\"crewId\":\"engineering\",\"name\":\"研发群\",\"kind\":\"topic\",\"members\":[{\"kind\":\"human\",\"id\":\"human\"},{\"kind\":\"session\",\"id\":\"captain\"},{\"kind\":\"session\",\"id\":\"reviewer\"}]},{\"id\":\"dm\",\"crewId\":\"engineering\",\"name\":\"机长\",\"kind\":\"dm\",\"members\":[{\"kind\":\"human\",\"id\":\"human\"},{\"kind\":\"session\",\"id\":\"captain\"}]}]").array
        workspace.crewID="engineering";workspace.sessionID="captain";workspace.channelID="group";workspace.status="离屏合成验收 · 未连接"
        workspace.messages=wire("[{\"id\":\"m1\",\"senderKind\":\"human\",\"senderName\":\"你\",\"text\":\"保留原来的通讯录和消息布局。先把需求与计划做成完整闭环。\"},{\"id\":\"m2\",\"senderKind\":\"session\",\"senderName\":\"机长\",\"senderSessionId\":\"captain\",\"text\":\"需求已保存为 #42，计划 W42 正在推进。验收员会独立检查。\"}]").array
        workspace.workStore=CrewdWorkStore {method,_ in
            switch method {
            case "work.list": return wire("{\"items\":[{\"id\":\"W42\",\"crewId\":\"engineering\",\"title\":\"贯通原始需求与客户端\",\"owner\":\"captain\",\"status\":\"active\",\"revision\":\"1\",\"sources\":[\"#42\"],\"parent\":null,\"blockedOn\":[],\"nextAfter\":null}],\"nextAfter\":null}")
            case "todo.list": return wire("[{\"id\":\"raw-42\",\"requirementId\":\"#42\",\"crewId\":\"engineering\",\"text\":\"保留原来的通讯录和消息布局\",\"status\":\"in_progress\"}]")
            case "ask.list": return .array([])
            default: fatalError("unexpected call in offline renderer: \(method)")
            }
        }
        workspace.workStore.selectCrew("engineering");try await workspace.workStore.refresh()
        for (name,scheme,appearance) in [("light",ColorScheme.light,NSAppearance.Name.aqua),("dark",ColorScheme.dark,NSAppearance.Name.darkAqua)] {
            NSApplication.shared.appearance=NSAppearance(named:appearance)
            let controller=NSHostingController(rootView:CrewdWorkspaceView(workspace:workspace,autoConnect:false).preferredColorScheme(scheme).environment(\.colorScheme,scheme).frame(width:1320,height:820))
            let window=NSWindow(contentRect:NSRect(x:0,y:0,width:1320,height:820),styleMask:[.titled],backing:.buffered,defer:false)
            window.appearance=NSAppearance(named:appearance)
            window.contentViewController=controller
            controller.view.frame=NSRect(x:0,y:0,width:1320,height:820)
            controller.view.layoutSubtreeIfNeeded()
            // Never order a window onscreen; yield for SwiftUI layout, without blocking its actor.
            try await Task.sleep(nanoseconds:250_000_000)
            controller.view.layoutSubtreeIfNeeded()
            guard let bitmap=controller.view.bitmapImageRepForCachingDisplay(in:controller.view.bounds) else {fatalError("native bitmap unavailable")}
            controller.view.cacheDisplay(in:controller.view.bounds,to:bitmap)
            guard let png=bitmap.representation(using:.png,properties:[:]) else {fatalError("native PNG unavailable")}
            try png.write(to:output.appendingPathComponent("crew-messages-"+name+".png"))
        }
        precondition(!workspace.client.connected)
        print("PASS: actual native view rendered offscreen; no connection, account, visible window or model")
    }
}
