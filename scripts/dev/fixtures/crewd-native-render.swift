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
            case "work.get": return wire("{\"id\":\"W42\",\"crewId\":\"engineering\",\"title\":\"贯通原始需求与客户端\",\"owner\":\"captain\",\"status\":\"active\",\"revision\":\"1\",\"sources\":[\"#42\"],\"pendingTransfer\":{\"to\":\"reviewer\",\"requestedBy\":\"captain\",\"requestSeq\":\"99\",\"note\":\"后端接口已经接通，请验收员接手完成独立核验。保留原始数据与故障证据。\",\"at\":\"2026-10-14T01:00:00.000Z\"}}")
            case "ask.list": return wire("[{\"ask\":{\"ask\":\"H43\",\"crewId\":\"engineering\",\"question\":\"希望保留哪种交接呈现？\",\"state\":\"open\",\"item\":\"W42\"},\"items\":[\"W42\"]}]")
            default: fatalError("unexpected call in offline renderer: \(method)")
            }
        }
        workspace.workStore.selectCrew("engineering");try await workspace.workStore.refresh()
        _ = try await workspace.workStore.detail(CrewdWorkID("W42",kind:.plan)!)
        let docID="10000000-0000-0000-0000-000000000001",folderID="20000000-0000-0000-0000-000000000001"
        let docBody="这里保留共享的开发说明。\n原始证据和下一步交接说明由机组成员共同维护。"
        let docSummary=CrewdValue.object(["id":.string(docID),"crewId":.string("engineering"),"title":.string("机组共同开发说明"),"revision":.string("1"),"bodyLength":.integer(Int64(docBody.utf8.count)),"createdBy":.object(["kind":.string("human")]),"updatedBy":.object(["kind":.string("session"),"sessionId":.string("captain")]),"createdAt":.string("2026-10-14T01:00:00Z"),"updatedAt":.string("2026-10-14T02:00:00Z"),"deletedAt":.null])
        workspace.resources=CrewdResources{method,params in
            switch method {
            case "document.list":return .object(["items":.array([docSummary]),"next":.null])
            case "document.get":guard case .object(var fields)=docSummary else{fatalError()};fields["body"] = .string(docBody);return .object(fields)
            case "viewFolder.list":
                let view=params["view"].string!,kind=view == "documents" ? "document":view == "messages" ? "channel":"work",id=view == "documents" ? docID:view == "messages" ? "group":"W42"
                return .object(["crewId":.string("engineering"),"view":.string(view),"treeRevision":.string("1"),"folders":.array([.object(["id":.string(folderID),"name":.string("开发资料"),"parentId":.null,"position":.integer(0)])]),"assignments":.array([.object(["object":.object(["kind":.string(kind),"id":.string(id)]),"folderId":.string(folderID)])])])
            default:fatalError("unexpected resource write in offscreen renderer: \(method)")
            }
        }
        workspace.resources.configureCapabilities(["documents","viewFolders"]);workspace.resources.selectCrew("engineering")
        try await workspace.resources.refreshDocuments();_ = try await workspace.resources.document(docID)
        for view in CrewdResourceView.allCases {_ = try await workspace.resources.refreshFolders(view)}
        var resourceDrafts=CrewdResourceDrafts();let docKey=CrewdResourceDrafts.Key(crew:"engineering",view:.documents,object:"",operation:"create",domain:.document)
        var docFields=CrewdResourceDrafts.Fields();docFields.title="准备新的交接说明";docFields.body=docBody;resourceDrafts.edit(docKey,docFields)
        let folderKey=CrewdResourceDrafts.Key(crew:"engineering",view:.documents,object:"",operation:"create",domain:.folder)
        var folderFields=CrewdResourceDrafts.Fields();folderFields.title="共享资料";folderFields.revision="1";resourceDrafts.edit(folderKey,folderFields)
        for (name,scheme,appearance) in [("light",ColorScheme.light,NSAppearance.Name.aqua),("dark",ColorScheme.dark,NSAppearance.Name.darkAqua)] {
            NSApplication.shared.appearance=NSAppearance(named:appearance)
            let screens:[(String,AnyView)] = [
                ("messages",AnyView(CrewdWorkspaceView(workspace:workspace,autoConnect:false))),
                ("documents",AnyView(CrewdDocumentsView(store:workspace.resources,drafts:.constant(resourceDrafts)))),
                ("document-editor",AnyView(CrewdDocumentEditor(store:workspace.resources,drafts:.constant(resourceDrafts),target:CrewdDocumentTarget(key:docKey)))),
                ("folder-editor",AnyView(CrewdFolderEditor(store:workspace.resources,drafts:.constant(resourceDrafts),view:.documents))),
                ("handoff",AnyView(CrewdHandoffView(store:workspace.workStore,id:CrewdWorkID("W42",kind:.plan)!,members:workspace.sessions))),
                ("human",AnyView(CrewdHumanActions(store:workspace.workStore,drafts:.constant(CrewdHumanDrafts()),target:CrewdHumanTarget(key:.init(scope:"engineering",crew:"engineering",ask:"H43"),crewTitle:"开发机组")))),
                ("todo",AnyView(CrewdWorkPanels(store:workspace.workStore,resources:workspace.resources,resourceDrafts:.constant(CrewdResourceDrafts()),drafts:.constant(CrewdRequirementDrafts()),planDrafts:.constant(CrewdPlanDrafts()),humanDrafts:.constant(CrewdHumanDrafts()),members:workspace.sessions,crews:workspace.crews))),
                ("progress",AnyView(CrewdWorkPanels(store:workspace.workStore,resources:workspace.resources,resourceDrafts:.constant(CrewdResourceDrafts()),drafts:.constant(CrewdRequirementDrafts()),planDrafts:.constant(CrewdPlanDrafts()),humanDrafts:.constant(CrewdHumanDrafts()),members:workspace.sessions,crews:workspace.crews,progressOnly:true)))
            ]
            let selected=ProcessInfo.processInfo.environment["PENDINGCREW_RENDER_SCREENS"].map{Set($0.split(separator:",").map(String.init))}
            if let selected {precondition(!selected.isEmpty && selected.isSubset(of:Set(screens.map{$0.0})),"explicit render screens must exist")}
            for (screen,view) in screens where selected == nil || selected!.contains(screen) {
            let controller=NSHostingController(rootView:view.preferredColorScheme(scheme).environment(\.colorScheme,scheme).frame(width:1320,height:820).background(CrewdChatStyle.canvas(scheme)))
            let windowStyle:NSWindow.StyleMask=ProcessInfo.processInfo.environment["PENDINGCREW_RENDER_BORDERLESS"] == "1" ? [.borderless]:[.titled]
            let window=NSWindow(contentRect:NSRect(x:0,y:0,width:1320,height:820),styleMask:windowStyle,backing:.buffered,defer:false)
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
            try png.write(to:output.appendingPathComponent("crew-"+screen+"-"+name+".png"))
        }
        }
        precondition(!workspace.client.connected)
        print("PASS: actual native view rendered offscreen; no connection, account, visible window or model")
    }
}
