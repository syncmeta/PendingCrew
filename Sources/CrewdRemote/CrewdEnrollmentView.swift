import SwiftUI

/// One ordinary enrollment flow shared by Mac, iPhone and iPad. Preview is pure;
/// all credential/network work follows a named, explicit human action.
struct CrewdEnrollmentView:View {
    let initialText:String
    let completed:(UUID)->Void
    @Environment(\.dismiss) private var dismiss
    @State private var input=""
    @State private var label="我的设备"
    @State private var preview:CrewdEnrollmentLink?
    @State private var attempts:[CrewdEnrollmentSummary]=[]
    @State private var status:String?
    @StateObject private var operation=CrewdRemoteOperation()
    init(initialText:String="",completed:@escaping(UUID)->Void){self.initialText=initialText;self.completed=completed}
    var body:some View {
        NavigationStack {
            Form {
                Section("添加电脑") {
                    Text("粘贴电脑提供的一次性配对链接。先核对身份，再确认授权本设备。")
                    TextEditor(text:$input).crewdRemoteCodeInput(height:100).disabled(operation.busy)
                        .onChange(of:input){_,_ in operation.invalidate();preview=nil;status=nil}
                    Button("核对电脑身份"){inspect()}.disabled(operation.busy || input.isEmpty)
                }
                if let p=preview {
                    Section("确认电脑身份") {
                        Text(p.computerLabel).font(.headline)
                        Text("有效期至 \(p.expiresAt)").font(.caption).foregroundStyle(.secondary)
                        Text("\(p.endpoint.host):\(p.endpoint.port)")
                        Text(p.server.spkiSha256).font(.caption.monospaced()).textSelection(.enabled)
                        Text("请与电脑上的指纹核对。链接只授权这台设备，不会启动任务。")
                        TextField("本设备名称",text:$label).disabled(operation.busy)
                        Button("身份无误，开始配对"){run{scope in try await CrewdEnrollment.shared.confirm(p,deviceLabel:label,canRestore:scope)}}.buttonStyle(CrewdPrimaryButtonStyle()).disabled(operation.busy || label.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty)
                    }
                }
                Section("原配对操作") {
                    Text("只读取本设备已保存的原请求；不会自动重试或连接。结果不明时只查询原结果。")
                        .font(.caption).foregroundStyle(.secondary)
                    Button("读取已保存的配对操作"){load()}.disabled(operation.busy)
                    ForEach(attempts){a in
                        VStack(alignment:.leading,spacing:8) {
                            Text(a.name).font(.headline)
                            Text("本设备："+a.deviceLabel).font(.caption).foregroundStyle(.secondary)
                            Text("\(a.endpoint.host):\(a.endpoint.port)")
                            Text(a.pin).font(.caption.monospaced()).textSelection(.enabled)
                            Text(message(a.phase)).foregroundStyle(a.phase == .accepted ? Color.secondary : Color.primary)
                            if a.phase == .prepared {
                                Button("提交已确认的原配对请求"){run{scope in try await CrewdEnrollment.shared.begin(a.id,canRestore:scope).summary}}.buttonStyle(CrewdPrimaryButtonStyle())
                            } else if a.phase == .challengeReady {
                                Button("确认原挑战并完成配对"){run{scope in try await CrewdEnrollment.shared.confirmChallenge(a.id,canRestore:scope)}}.buttonStyle(CrewdPrimaryButtonStyle())
                            }
                            Button("查询原请求结果"){run{_ in try await CrewdEnrollment.shared.queryOriginal(a.id)}}
                            if a.phase == .accepted {
                                Button("连接这台电脑"){completed(a.id)}.buttonStyle(CrewdPrimaryButtonStyle())
                            }
                            DisclosureGroup("原请求编号"){Text(a.requestId).font(.caption.monospaced()).textSelection(.enabled)}
                        }.disabled(operation.busy)
                    }
                }
                if let status{Section{Text(status).foregroundStyle(.secondary)}}
            }
            .navigationTitle("添加电脑")
            .toolbar{ToolbarItem(placement:.cancellationAction){Button("关闭"){operation.invalidate();dismiss()}}}
        }
        .modifier(CrewdAdaptiveFormFrame(width:580,height:600,scrolls:false))
        .onAppear{if input.isEmpty && !initialText.isEmpty{input=initialText;inspect()}}
        .onDisappear{operation.invalidate()}
    }
    private func inspect(){do{preview=try CrewdEnrollment.preview(input.trimmingCharacters(in:.whitespacesAndNewlines));status=nil}catch{preview=nil;status="链接无效或电脑证书无法核对。请检查完整配对链接。"}}
    private func load(){guard !operation.busy else{return};operation.invalidate();operation.run({try await CrewdEnrollment.shared.summaries()},apply:{attempts=$0;status=nil},failed:{_ in status="无法读取已保存的配对操作。请解锁设备后再读取；不会创建替代请求。"})}
    private func run(_ action:@escaping @MainActor (@escaping CrewdEnrollment.RecoveryScope)async throws->CrewdEnrollmentSummary){
        guard !operation.busy else{return}
        operation.invalidate()
        let scope=operation.recoveryScope()
        operation.run({try await action(scope)},apply:{summary in
            attempts.removeAll{$0.id==summary.id};attempts.append(summary);preview=nil
            status=message(summary.phase)
        },failed:{error in
            status=(error as? CrewdEnrollmentTransportFailure)?.errorDescription ?? "操作未完成或结果尚未确认。请读取已保存的原配对操作核对；如原请求存在，只查询原结果，不要重新配对。"
            Task{if let values=try? await CrewdEnrollment.shared.summaries(),await scope(){attempts=values}}
        })
    }
    private func message(_ phase:CrewdEnrollmentAttempt.Phase)->String {
        switch phase {
        case .prepared:return "已保存本设备凭据，尚未提交。"
        case .beginUnknown:return "原邀请结果未确认，只查询原请求。"
        case .challengeReady:return "原电脑挑战已核对，等待你确认完成配对。"
        case .commitUnknown:return "原授权结果未确认，不重新发送授权。"
        case .accepted:return "本设备配对已确认。连接后才读取电脑工作区。"
        case .expired:return "原邀请已过期，原请求和本设备版本仍保留。"
        case .revoked:return "电脑已撤销原授权，旧确认不能重新激活。"
        case .notFound:return "尚未找到原请求结果。不会重发或生成替代身份。"
        }
    }
}
struct CrewdEnrollmentURLEntry:ViewModifier {
    let completed:(UUID)->Void
    @State private var incoming:Link?
    private struct Link:Identifiable{let id=UUID();let text:String}
    func body(content:Content)->some View {
        content.onOpenURL{url in
            let text=url.absoluteString
            guard text.hasPrefix("pendingcrew://enroll/") else{return}
            incoming=Link(text:text)
        }.sheet(item:$incoming){entry in CrewdEnrollmentView(initialText:entry.text){id in incoming=nil;completed(id)}}
    }
}
