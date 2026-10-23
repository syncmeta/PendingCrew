import SwiftUI
#if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
import Inject
#endif
#if os(macOS)
import AppKit
#endif

enum CrewdType {
    #if os(macOS)
    static let title=Font.system(size:17,weight:.semibold)
    static let body=Font.system(size:13)
    static let label=Font.system(size:12)
    static let caption=Font.system(size:11)
    static let target:CGFloat=28
    #else
    static let title=Font.title3.weight(.semibold)
    static let body=Font.body
    static let label=Font.subheadline
    static let caption=Font.caption
    static let target:CGFloat=44
    #endif
}

/// Presentation only. Inputs, authority, receipts and mutations stay with callers.
struct CrewdPanelHeader:View {
    let title:String
    var detail:String?=nil
    var symbol:String?=nil
    var close:(()->Void)?=nil
    var body:some View {
        HStack(alignment:.top,spacing:12){
            if let symbol{Image(systemName:symbol).font(.system(size:18,weight:.medium)).foregroundStyle(Theme.Palette.accent).frame(width:30,height:32)}
            VStack(alignment:.leading,spacing:5){
                Text(title).font(CrewdType.title).foregroundStyle(.primary)
                if let detail{Text(detail).font(CrewdType.label).foregroundStyle(.secondary).fixedSize(horizontal:false,vertical:true)}
            }
            Spacer(minLength:8)
            if let close{Button(action:close){Image(systemName:"xmark").font(.system(size:11,weight:.semibold))}.buttonStyle(CrewdToolButtonStyle()).accessibilityLabel("关闭")}
        }.padding(.bottom,8)
    }
}
struct CrewdFormSection<Content:View>:View {
    let title:String
    var detail:String?=nil
    @ViewBuilder let content:()->Content
    var body:some View {
        VStack(alignment:.leading,spacing:12){
            VStack(alignment:.leading,spacing:4){
                Text(title).font(CrewdType.label.weight(.semibold)).foregroundStyle(.primary)
                if let detail{Text(detail).font(CrewdType.caption).foregroundStyle(.secondary).fixedSize(horizontal:false,vertical:true)}
            }
            content().frame(maxWidth:.infinity,alignment:.leading)
        }.padding(16).frame(maxWidth:.infinity,alignment:.leading)
            .background(Theme.Palette.surface,in:RoundedRectangle(cornerRadius:12))
            .overlay(RoundedRectangle(cornerRadius:12).strokeBorder(Theme.Palette.hairline.opacity(0.8),lineWidth:0.7))
    }
}
struct CrewdFormField<Content:View>:View {
    let title:String
    var detail:String?=nil
    @ViewBuilder let content:()->Content
    var body:some View {
        VStack(alignment:.leading,spacing:6){
            Text(title).font(CrewdType.label.weight(.medium)).foregroundStyle(.primary)
            content().font(CrewdType.body).textFieldStyle(.roundedBorder)
            if let detail{Text(detail).font(CrewdType.caption).foregroundStyle(.secondary).fixedSize(horizontal:false,vertical:true)}
        }.frame(maxWidth:.infinity,alignment:.leading)
    }
}
struct CrewdToolButtonStyle:ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration:Configuration)->some View {
        configuration.label.foregroundStyle(.secondary)
            .frame(minWidth:CrewdType.target,minHeight:CrewdType.target)
            .background(configuration.isPressed ? Theme.Palette.surfaceMuted:Color.clear,in:RoundedRectangle(cornerRadius:6))
            .contentShape(RoundedRectangle(cornerRadius:6)).opacity(enabled ? 1:0.4)
    }
}
struct CrewdPrimaryButtonStyle:ButtonStyle {
    @Environment(\.isEnabled) private var enabled
    func makeBody(configuration:Configuration)->some View {
        configuration.label.font(CrewdType.label.weight(.semibold))
            .foregroundStyle(enabled ? Theme.Palette.onAccent:Theme.Palette.inkMuted)
            .padding(.horizontal,15).padding(.vertical,9).frame(minHeight:CrewdType.target)
            .background(enabled ? (configuration.isPressed ? Theme.Palette.accentHover:Theme.Palette.accent):Theme.Palette.surfaceMuted,in:RoundedRectangle(cornerRadius:7))
            .opacity(configuration.isPressed ? 0.88:1)
    }
}
struct CrewdInlineNotice:View {
    let title:String
    var detail:String?=nil
    var symbol="info.circle"
    var warning=false
    var body:some View {
        HStack(alignment:.top,spacing:9){
            Image(systemName:symbol).font(CrewdType.body).foregroundStyle(warning ? Theme.Palette.amber:Theme.Palette.accent)
            VStack(alignment:.leading,spacing:4){
                Text(title).font(CrewdType.label.weight(.medium))
                if let detail{Text(detail).font(CrewdType.caption).foregroundStyle(.secondary).textSelection(.enabled)}
            }.frame(maxWidth:.infinity,alignment:.leading)
        }.padding(12).background(warning ? Theme.Palette.amberBg.opacity(0.5):Theme.Palette.surfaceMuted.opacity(0.5),in:RoundedRectangle(cornerRadius:8))
    }
}
struct CrewdDetailValue:View {
    let title:String
    let value:String
    var body:some View {
        HStack(alignment:.firstTextBaseline,spacing:12){Text(title).foregroundStyle(.secondary);Spacer(minLength:12);Text(value).multilineTextAlignment(.trailing).textSelection(.enabled)}.font(CrewdType.label)
    }
}

/// Counts only one already complete, current channel snapshot. No read, cache,
/// crew aggregation, unread-message substitution or owner/asker inference.
enum CrewdTodoAttention {
    static func pending(_ row:CrewdValue,attributes:CrewdValue?=nil,human:Bool)->Bool {
        guard row["object"]["kind"].string == (human ? "humanTask":"requirement") else{return false}
        let source=attributes ?? row["todo"]
        let state=source["manualState"].string ?? row["status"].string ?? ""
        return human ? state == "open":(!state.isEmpty && !["done","closed"].contains(state))
    }
}
struct CrewdAttentionAvatar<Content:View>:View {
    @ObservedObject var group:CrewdChannelWorkStore
    let crew:String
    let channel:String
    @ViewBuilder let content:()->Content
    private func count(_ human:Bool)->Int? {
        guard group.loaded,!group.loading,group.failure==nil,group.crew==crew,group.channel==channel else{return nil}
        return group.items.filter{CrewdTodoAttention.pending($0,attributes:group.todoAttributes[$0["object"]["id"].string ?? ""],human:human)}.count
    }
    var body:some View {
        content()
            .overlay(alignment:.topTrailing){if let n=count(false),n>0{badge(n,human:false).offset(x:6,y:-4)}}
            .overlay(alignment:.bottomTrailing){if let n=count(true),n>0{badge(n,human:true).offset(x:6,y:4)}}
            .padding(.trailing,3)
    }
    private func badge(_ count:Int,human:Bool)->some View {
        Text(count>99 ? "99+":String(count)).font(.system(size:9,weight:.semibold)).monospacedDigit()
            .foregroundStyle(human ? Color(red:0.37,green:0.28,blue:0):Color.white)
            .padding(.horizontal,4).frame(minWidth:16,minHeight:16)
            .background(human ? Color(red:0.91,green:0.73,blue:0.22):Color(red:0.91,green:0.33,blue:0.31),in:Capsule())
            .overlay(Capsule().strokeBorder(Theme.Palette.canvas,lineWidth:1.4))
            .accessibilityLabel("当前群"+(human ? "人工待办":"AI待办")+" "+String(count)+" 项未完成")
            .help("当前群已完整读取的"+(human ? "人工待办":"AI待办"))
    }
}

/// Provider attribution without fabricated quota rings or legacy-account data.
/// Token/context usage belongs to each actual session, not this subscription row.
struct CrewdProviderStatusFooter:View {
    var body:some View {
        VStack(alignment:.leading,spacing:9){
            row("ClaudeLogomark","Claude Code",Theme.Palette.amber)
            row("OpenAILogomark","Codex",Theme.Palette.openAIMark)
        }.padding(.horizontal,14).padding(.vertical,12)
            .frame(maxWidth:.infinity,alignment:.leading)
    }
    private func row(_ asset:String,_ name:String,_ color:Color)->some View {
        HStack(spacing:8){
            Image(asset).renderingMode(.template).resizable().scaledToFit().frame(width:15,height:15).foregroundStyle(color)
            Text(name).font(CrewdType.caption.weight(.medium))
            Spacer(minLength:4)
            Text("额度未读取").font(CrewdType.caption).foregroundStyle(.secondary)
        }.help("尚无这台电脑的订阅额度信息；不会将会话 Token 用量当作订阅额度。")
    }
}

/// Navigation intent only. The owning native shell chooses inline or sheet.
private struct CrewdShowInspectorKey:EnvironmentKey {static let defaultValue:(()->Void)?=nil}
extension EnvironmentValues {
    var crewdShowInspector:(()->Void)? {
        get{self[CrewdShowInspectorKey.self]}
        set{self[CrewdShowInspectorKey.self]=newValue}
    }
}

struct CrewdPairingStepHeader:View {
    let awaitingReceipt:Bool
    let hasPreview:Bool
    var body:some View {
        CrewdPanelHeader(title:awaitingReceipt ? "等待电脑确认":hasPreview ? "核对这台电脑":"连接你的电脑",
            detail:awaitingReceipt ? "把回应交给电脑，批准后导入正式确认码。":hasPreview ? "确认名称、地址和身份指纹，再授权这台设备。":"导入电脑生成的邀请，先查看身份，再由你确认。",
            symbol:awaitingReceipt ? "hourglass":"desktopcomputer")
    }
}

/// One material at the window root; panes remain transparent rather than
/// stacking glass sheets behind the conversation or individual sidebar rows.
struct CrewdWindowMaterial:ViewModifier {
    @Environment(\.colorScheme) private var scheme
    @ViewBuilder func body(content:Content)->some View {
        #if os(macOS)
        // One native semantic material on every supported Mac version. The
        // view's appearance is projected from the same SwiftUI scheme as text,
        // not inferred separately by a window-container background host.
        content.background(CrewdWindowVisualEffect(scheme:scheme).ignoresSafeArea())
        #else
        if #available(iOS 18.0,*) {
            content.containerBackground(for:.navigationSplitView){
                Rectangle().fill(.regularMaterial).environment(\.colorScheme,scheme)
            }
        }else {content.background(.regularMaterial)}
        #endif
    }
}
#if os(macOS)
private struct CrewdWindowVisualEffect:NSViewRepresentable {
    let scheme:ColorScheme
    func makeNSView(context:Context)->NSVisualEffectView {
        let view=CrewdBehindWindowMaterialView()
        configure(view)
        return view
    }
    func updateNSView(_ view:NSVisualEffectView,context:Context){configure(view)}
    private func configure(_ view:NSVisualEffectView){
        if view.material != .windowBackground { view.material = .windowBackground }
        if view.blendingMode != .behindWindow { view.blendingMode = .behindWindow }
        if view.state != .followsWindowActiveState { view.state = .followsWindowActiveState }
        let name:NSAppearance.Name = scheme == .dark ? .darkAqua:.aqua
        if view.appearance?.name != name { view.appearance=NSAppearance(named:name) }
    }
}
private final class CrewdBehindWindowMaterialView:NSVisualEffectView {
    override func viewDidMoveToWindow(){
        super.viewDidMoveToWindow()
        if let window {
            if window.isOpaque { window.isOpaque=false }
            if window.backgroundColor != .clear { window.backgroundColor = .clear }
        }
    }
}
#endif
/// Native SwiftUI buttons retain the original five-domain binding. Only this
/// navigation capsule uses Liquid Glass; long content and messages do not.
struct CrewdGlassWorkspaceTabs:View {
    #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
    @ObserveInjection private var injection
    #endif
    @Binding var selection:String
    #if os(macOS)
    @ScaledMetric(relativeTo:.caption) private var tabHeight:CGFloat=28
    #else
    @ScaledMetric(relativeTo:.subheadline) private var tabHeight:CGFloat=44
    #endif
    private let items=[("消息","消息"),("人待办","真人Todo"),("AI待办","Todo"),("文档/数据","文档"),("计划/进展","进展")]
    var body:some View {
        ScrollViewReader {proxy in
            ScrollView(.horizontal,showsIndicators:false){capsule.padding(1)}
                .onChange(of:selection){_,value in proxy.scrollTo(value,anchor:.center)}
                .onAppear{proxy.scrollTo(selection,anchor:.center)}
        }.frame(height:max(CrewdType.target,tabHeight)+10)
            .accessibilityElement(children:.contain).accessibilityLabel("工作区内容")
        #if DEBUG && PENDINGCREW_HOTRELOAD && os(macOS)
        .enableInjection()
        #endif
    }
    @ViewBuilder private var capsule:some View {
        if #available(macOS 26.0,iOS 26.0,*) {
            buttons.glassEffect(.regular,in:Capsule())
        }else {buttons.background(.regularMaterial,in:Capsule())}
    }
    private var buttons:some View {
        HStack(spacing:2){
            ForEach(items,id:\.1){item in
                Button {selection=item.1}label:{
                    Text(item.0).font(CrewdType.label.weight(selection == item.1 ? .semibold:.regular))
                        .foregroundStyle(.primary)
                        .lineLimit(1).padding(.horizontal,13).frame(minHeight:max(CrewdType.target,tabHeight))
                        .background(selection == item.1 ? Theme.Palette.accent.opacity(0.13):Color.clear,in:Capsule())
                        .contentShape(Capsule())
                }.buttonStyle(.plain).id(item.1)
                    .accessibilityAddTraits(selection == item.1 ? .isSelected:[])
            }
        }.padding(4).fixedSize(horizontal:true,vertical:false)
    }
}
/// Green means the current hello/subscription/authority-qualified connection,
/// never an entry in the paired-computer catalogue or an existing socket file.
struct CrewdHostConnectionLabel:View {
    @ObservedObject var client:CrewdClient
    let name:String
    var connecting=false
    var hasUnfinishedOperation=false
    private var state:String {client.readyForWork ? "已连接":client.connected ? "正在验证连接":connecting ? "正在连接":"未连接"}
    var body:some View {
        HStack(spacing:7){
            Circle().fill(client.readyForWork ? Color.green:client.connected || connecting ? Color.orange:Color.secondary.opacity(0.5)).frame(width:7,height:7)
            Text(name).lineLimit(1)
            if !client.readyForWork {Text(state).font(CrewdType.caption).foregroundStyle(.secondary).lineLimit(1)}
            if hasUnfinishedOperation {Image(systemName:"exclamationmark.triangle").foregroundStyle(.orange)}
        }.accessibilityElement(children:.ignore)
            .accessibilityLabel(name + "，" + state + (hasUnfinishedOperation ? "，当前操作未完成，可核对原请求":""))
    }
}

/// A single action for an empty workspace; no decorative canvas or message.
struct CrewdGlassAddButton:View {
    var title = "新建机组"
    let action:()->Void
    var body:some View {
        Button(action:action){Image(systemName:"plus").font(.system(size:22,weight:.medium)).frame(width:52,height:52).contentShape(Circle())}
            .buttonStyle(.plain).modifier(CrewdAddGlass()).accessibilityLabel(title).help(title)
    }
}
private struct CrewdAddGlass:ViewModifier {
    @ViewBuilder func body(content:Content)->some View {
        if #available(macOS 26.0,iOS 26.0,*){content.glassEffect(.regular.interactive(),in:Circle())}
        else{content.background(.regularMaterial,in:Circle())}
    }
}
