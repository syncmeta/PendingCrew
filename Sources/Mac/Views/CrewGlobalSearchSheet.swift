import SwiftUI

/// Reject malformed collections and ambiguous identities before exposing results.
enum CrewSearchWire {
    static func rows(_ value:CrewdValue,id:(CrewdValue)->String?) throws->[CrewdValue]{
        guard case .array(let rows)=value else{throw CrewdPublicWire.invalid()}
        var seen=Set<String>()
        for row in rows{guard let key=id(row),!key.isEmpty,seen.insert(key).inserted else{throw CrewdPublicWire.invalid()}}
        return rows
    }
}

enum CrewSearchScope:String,CaseIterable,Identifiable {
    case current = "当前群"
    case all = "全局"
    var id:String{rawValue}
}

/// Results live beneath the one system search field. Global means this verified
/// computer, never an aggregation of paired computers or an unbounded archive.
struct CrewGlobalSearchResults:View {
    @EnvironmentObject private var crewStore:CrewStore
    let query:String
    let selected:()->Void
    var body:some View {
        if let workspace=crewStore.crewdWorkspace {
            CrewCoreSearchResults(workspace:workspace,client:workspace.client,query:query,selected:selected)
        }else if crewStore.usesCrewdAuthority {
            ContentUnavailableView("当前电脑尚未连接",systemImage:"magnifyingglass",description:Text("搜索不会自动连接电脑。"))
        }else {
            CrewLegacySearchResults(query:query,selected:selected)
        }
    }
}
private struct CrewCoreSearchResults:View {
    @EnvironmentObject private var crewStore:CrewStore
    @ObservedObject var workspace:CrewdWorkspace
    @ObservedObject var client:CrewdClient
    let query:String
    let selected:()->Void
    @State private var documents:[CrewMessageSearchDocument]=[]
    @State private var loading=false
    @State private var failure:String?
    @State private var epoch:UUID?
    @State private var navigationGeneration=UUID()
    @State private var visible=false
    private var results:[CrewMessageSearchResult]{CrewMessageSearch.search(documents,query:query.trimmingCharacters(in:.whitespacesAndNewlines),limit:200,order:.newestFirst)}
    var body:some View {
        VStack(spacing:0){
            if loading{ProgressView("正在读取当前电脑的可访问消息…").frame(maxWidth:.infinity,maxHeight:.infinity)}
            else if let failure{ContentUnavailableView("搜索尚未完成",systemImage:"magnifyingglass",description:Text(failure))}
            else if query.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty{ContentUnavailableView("搜索当前电脑",systemImage:"magnifyingglass",description:Text("输入消息正文、发送者、附件名或时间。"))}
            else if results.isEmpty{ContentUnavailableView.search(text:query)}
            else{List(results){result in
                CrewSearchResultRow(result:result){Task{await open(result)}}
            }.listStyle(.plain)}
            Text("范围：当前已验证电脑的可访问机组与会话；每机组最多读取 1000 条，显示最新 200 条匹配。")
                .font(.caption).foregroundStyle(.secondary).padding(12)
        }
        .onAppear{visible=true}
        .onDisappear{visible=false;navigationGeneration=UUID()}
        .onChange(of:query){_,_ in navigationGeneration=UUID()}
        .task(id:client.connectionEpoch){await load()}
        .onChange(of:workspace.presentationScope){_,_ in documents=[];failure="原搜索来源已变化，请重新搜索。"}
        .onChange(of:client.readyForWork){_,ready in if !ready{documents=[];failure="原连接已变化，请重新搜索。"}}
    }
    private func load()async {
        documents=[];failure=nil;loading=true
        let captured=client.connectionEpoch,origin=workspace.presentationScope,generation=workspace.observationGeneration
        epoch=captured
        func valid()->Bool{!Task.isCancelled && client.readyForWork && client.connectionEpoch==captured && workspace.presentationScope==origin && workspace.acceptsObservation(generation) && crewStore.crewdWorkspace === workspace}
        do{
            guard valid() else{throw CancellationError()}
            let crews=try await client.call("crew.list",.null);guard valid() else{throw CancellationError()}
            var next:[CrewMessageSearchDocument]=[]
            for crew in try CrewSearchWire.rows(crews,id:{$0["id"].string}) {
                guard let id=crew["id"].string else{throw CrewdPublicWire.invalid()}
                let channels=try await client.call("channel.list",.object(["crewId":.string(id)]));guard valid() else{throw CancellationError()}
                var remaining=1000
                let channelRows=try CrewSearchWire.rows(channels,id:{$0["channel"]["id"].string})
                for row in channelRows where remaining>0 {
                    let channel=row["channel"]
                    guard let channelID=channel["id"].string,channel["crewId"].string==id else{throw CrewdPublicWire.invalid()}
                    let messages=try await client.call("message.list",.object(["channelId":.string(channelID),"limit":.integer(Int64(remaining))]));guard valid() else{throw CancellationError()}
                    let messageRows=try CrewSearchWire.rows(messages,id:{$0["id"].string})
                    for message in messageRows.prefix(remaining) {
                        guard let messageID=message["id"].string,let created=message["createdAt"].string,message["channelId"].string==channelID,let text=message["text"].string else{throw CrewdPublicWire.invalid()}
                        next.append(.init(crewId:id,crewTitle:crew["title"].string ?? id,messageId:messageID,text:text,senderName:message["senderName"].string ?? message["senderKind"].string ?? "",senderId:message["senderSessionId"].string,createdAt:created,attachmentMetadata:message["attachments"].array.flatMap{[$0["filename"].string,$0["mime"].string].compactMap{$0}},channelId:channelID))
                    }
                    remaining-=min(remaining,messageRows.count)
                }
            }
            guard valid() else{throw CancellationError()};documents=next;loading=false
        }catch{if epoch==captured{documents=[];loading=false;failure=error is CancellationError ? "原搜索连接已变化，请重新搜索。":error.localizedDescription}}
    }
    private func open(_ result:CrewMessageSearchResult)async {
        guard epoch==client.connectionEpoch,crewStore.crewdWorkspace === workspace,let channel=result.document.channelId else{return}
        let captured=client.connectionEpoch,request=navigationGeneration
        func current()->Bool{visible && navigationGeneration==request && captured==client.connectionEpoch && crewStore.crewdWorkspace === workspace}
        do{try await workspace.openSearchMessage(crew:result.document.crewId,channel:channel,message:result.document.messageId,stillCurrent:current)
            guard current() else{return}
            crewStore.openChatSearchResult(result,query:query);workspace.originalSection="消息";selected()
        }catch{if current(){failure=error.localizedDescription}}
    }
}
private struct CrewLegacySearchResults:View {
    @EnvironmentObject private var crewStore:CrewStore
    let query:String
    let selected:()->Void
    @State private var documents:[CrewMessageSearchDocument]=[]
    var body:some View {
        List(CrewMessageSearch.search(documents,query:query,limit:200,order:.newestFirst)){result in
            CrewSearchResultRow(result:result){crewStore.openChatSearchResult(result,query:query);selected()}
        }.task{
            let crews=crewStore.crews.map{($0.id,$0.title)}
            documents=await Task.detached{crews.flatMap{crew in LocalWhiteboardStore.shared.list(crewId:crew.0).map{CrewMessageSearchAdapters.local($0,crewId:crew.0,crewTitle:crew.1)}}}.value
        }
    }
}
private struct CrewSearchResultRow:View {
    let result:CrewMessageSearchResult
    let action:()->Void
    var body:some View {
        Button(action:action){VStack(alignment:.leading,spacing:5){
            HStack{Text(result.document.crewTitle).font(.headline);Text(result.document.senderName).foregroundStyle(.secondary);Spacer();Text(result.document.createdAt).font(.caption).foregroundStyle(.secondary)}
            Text(result.document.text.isEmpty ? "（仅附件）":result.document.text).lineLimit(3)
            if !result.document.attachmentMetadata.isEmpty{Text(result.document.attachmentMetadata.joined(separator:" · ")).font(.caption).foregroundStyle(.secondary)}
        }.frame(maxWidth:.infinity,alignment:.leading).padding(.vertical,5).contentShape(Rectangle())}.buttonStyle(.plain)
    }
}
