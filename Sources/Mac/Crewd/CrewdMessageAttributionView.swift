import SwiftUI

struct CrewdMessageAttributionView:View {
    @ObservedObject var store:CrewdWorkStore
    let crew:String
    let message:String
    @State private var value:CrewdValue?
    @State private var selection=""
    @State private var targetKind="work"
    @State private var targetID=""
    @State private var failure:String?
    @State private var busy=false
    @State private var token=UUID()
    private var identity:String {crew+"/"+message}
    var body:some View {
        VStack(alignment:.leading,spacing:8) {
            Text("消息归属").font(.headline)
            if let value {
                Text(source(value))
                if let id=value["link"]["id"].string {
                    Text("当前关联：" + (value["candidates"].array.first(where:{$0["link"] == value["link"]})?["summary"].string ?? (value["resolvedCandidate"]["link"] == value["link"] ? value["resolvedCandidate"]["summary"].string:nil) ?? "对象摘要暂不可读取") + "（" + id + "）")
                } else {Text(value["source"] == .string("human") ? "已明确设为无归属。":"当前没有有效归属。")}
                Picker("纠正为",selection:$selection) {
                    Text("请选择关联对象").tag("")
                    ForEach(Array(value["candidates"].array.enumerated()),id:\.offset) { index,candidate in
                        Text((candidate["summary"].string ?? "") + "（" + (candidate["link"]["id"].string ?? "") + "）").tag(String(index))
                    }
                }
                Picker("样本外对象类型",selection:$targetKind) {Text("计划").tag("work");Text("需求").tag("requirement");Text("前序消息").tag("message")}
                TextField("对象编号",text:$targetID)
                Button("查找并核对对象") {resolve()}.disabled(busy || targetID.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
                if value["resolvedCandidate"] != .null {
                    Text("查找结果：" + (value["resolvedCandidate"]["summary"].string ?? ""))
                    Button("使用已核对对象纠正") {write("set",resolved:true)}.disabled(busy)
                }
                Text(value["candidatesTruncated"] == .bool(true) ? "候选为当前来源范围内的部分样本，并非全部对象。":"候选限当前来源范围；不会改变消息回复或工作负责人。").font(.caption).foregroundStyle(.secondary)
                HStack {
                    Button("应用纠正") {write("set")}.disabled(selection.isEmpty)
                    Button("明确无归属") {write("clear")}
                    Button("撤销人工覆盖") {write("reset")}.disabled(value["override"] == .null)
                }.disabled(busy)
                Text("撤销后只恢复仍然有效的已有模型结果，不会重新请求模型。").font(.caption).foregroundStyle(.secondary)
            }
            Button(busy ? "正在处理…":"刷新归属及候选") {read()}.disabled(busy)
            if let failure {Text(failure).foregroundStyle(.orange)}
        }.font(.callout)
        .task(id:identity) {read()}
        .onChange(of:identity) {_,_ in invalidate()}
        .onChange(of:store.readScope) {_,_ in invalidate()}
        .onDisappear {invalidate()}
    }
    private func source(_ v:CrewdValue)->String {["model":"当前采用模型归属","human":"当前采用人工纠正","none":"暂无归属记录","stale":"原归属依据已失效"][v["source"].string ?? ""] ?? "未知来源"}
    private func invalidate() {token=UUID();value=nil;failure=nil;selection="";busy=false}
    private func read() {
        let request=UUID(),original=identity;token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let result=try await store.messageAttributionWithSummary(crew:crew,message:message);guard token == request,identity == original else{return};value=result;selection=""}
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
    private func resolve() {
        let request=UUID(),original=identity;token=request;busy=true;failure=nil
        let target:CrewdValue = .object(["kind":.string(targetKind),"id":.string(targetID.trimmingCharacters(in:.whitespacesAndNewlines))])
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let result=try await store.messageAttribution(crew:crew,message:message,target:target);guard token == request,identity == original else{return};value=result;selection=""}
            catch {if token == request {failure=error.localizedDescription}}
        }
    }
    private func write(_ action:String,resolved:Bool=false) {
        guard let value else{return}
        let link:CrewdValue?
        if action == "set",resolved {link=value["resolvedCandidate"]["link"]}
        else if action == "set" {guard let index=Int(selection),value["candidates"].array.indices.contains(index) else{return};link=value["candidates"].array[index]["link"]} else {link=nil}
        let request=UUID(),original=identity;token=request;busy=true;failure=nil
        Task { @MainActor in
            defer {if token == request {busy=false}}
            do {let result=try await store.setMessageAttribution(crew:crew,message:message,revision:value["revision"].string ?? "",action:action,link:link);guard token == request,identity == original else{return};self.value=result;selection=""
                if action == "set",let enriched=try? await store.messageAttributionWithSummary(crew:crew,message:message),token == request,identity == original {self.value=enriched}
            }
            catch is CancellationError {return}
            catch {
                guard token == request,identity == original else{return}
                failure="操作未获确认："+error.localizedDescription+"；只刷新当前状态，不会重发。"
                if let result=try? await store.messageAttributionWithSummary(crew:crew,message:message),token == request,identity == original {self.value=result;selection=""}
            }
        }
    }
}
