import SwiftUI
import WebKit

/// A code view receives only the knowledge bridge of its captured revision.
/// Ephemeral storage and CSP are containment layers, not an OS resource quota.
struct CrewdKnowledgeCanvas: View {
    @ObservedObject var runtime: CrewdKnowledgeViewRuntime
    @State private var status = ""
    @State private var enableContext: CrewdKnowledgeViewContext?
    static var supported: Bool {
        #if os(iOS)
        if #available(iOS 18.4, *) { return true }; return false
        #else
        return true
        #endif
    }
    var body: some View {
        VStack(spacing: 0) {
            ViewThatFits(in: .horizontal) {
                HStack { codeControls; pendingReview; writeControl }.fixedSize(horizontal: true, vertical: false)
                VStack(alignment: .leading, spacing: 8) { codeControls; pendingReview; writeControl }
            }.frame(maxWidth: .infinity, alignment: .leading).padding(8)
            if !status.isEmpty { Text(status).font(.caption).padding(4) }
            if !Self.supported {
                Text("此系统仅提供源码预览；执行代码需要 iOS 18.4 或更新版本。").font(.caption).foregroundStyle(.secondary).padding(8)
                sourcePreview
            } else if runtime.failed { ContentUnavailableView("视图已停止", systemImage: "exclamationmark.triangle", description: Text(runtime.unsupportedResourcePolicy ? "当前 WebKit 的必要资源拒绝策略无法验证，代码未继续运行；原生数据与文档仍可用。":"未重新提交任何写入。返回原生视图查看状态。")) }
            else if runtime.executionEnabled { CrewdKnowledgeWebSurface(runtime: runtime) }
            else { sourcePreview }
        }
        .confirmationDialog("启用当前版本的作者代码与交互？", isPresented: Binding(get: { enableContext != nil }, set: { if !$0 { enableContext = nil } }), titleVisibility: .visible) {
            if let context = enableContext {
                Button("启用此版本代码") {
                    enableContext = nil
                    do { try runtime.setExecutionEnabledFromNativeHost(true, context: context); status = "" }
                    catch { status = "版本或连接已变化；未启用代码。" }
                }
            }
            Button("取消", role: .cancel) { enableContext = nil }
        } message: {
            Text("启用后，视图脚本可能在交互期间修改系统剪贴板。仅在信任此版本代码时启用；团队数据写入需另行授权。")
        }
        .onChange(of: runtime.context) { _,_ in enableContext = nil }
        .onDisappear { enableContext = nil; runtime.revoke() }
    }
    private var codeControls: some View {
        HStack {
                Text("代码视图").font(.caption).foregroundStyle(.secondary)
                Spacer()
                if runtime.executionEnabled {
                    Button("停止代码") {
                        guard let context = runtime.context else { return }
                        try? runtime.setExecutionEnabledFromNativeHost(false, context: context)
                    }
                } else {
                    Button("启用此版本代码…") { enableContext = runtime.context }
                        .disabled(!Self.supported || runtime.context == nil || runtime.failed)
                }
        }
    }
    @ViewBuilder private var pendingReview: some View {
                if runtime.pendingWrite?.context.origin == runtime.context?.origin {
                    Button("查询原写入结果") {
                        Task {
                            do { _ = try await runtime.reviewPendingFromNativeHost(); status = "已查询；未重新提交。" }
                            catch { status = "结果仍未知。" }
                        }
                    }
                }
    }
    private var writeControl: some View {
                Toggle("允许此版本写入知识数据", isOn: Binding(get: { runtime.writeEnabled }, set: { enabled in
                    do { try runtime.setWriteEnabledFromNativeHost(enabled); status = "" }
                    catch { status = "此视图当前不可获得写入能力。" }
                }))
                .toggleStyle(.switch).disabled(!Self.supported || runtime.context == nil || runtime.failed || !runtime.renderQualified || runtime.bundle?.declaresKnowledgeWrite != true)
    }
    private var sourcePreview: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                Text("源码预览 · 作者代码尚未运行").font(.headline)
                if let bundle = runtime.bundle {
                    Text("HTML").font(.caption); Text(bundle.html).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    Text("CSS").font(.caption); Text(bundle.css).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    Text("JavaScript").font(.caption); Text(bundle.javascript).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                }
            }.frame(maxWidth: .infinity, alignment: .leading).padding()
        }
    }
}

#if os(macOS)
private struct CrewdKnowledgeWebSurface: NSViewRepresentable {
    let runtime: CrewdKnowledgeViewRuntime
    func makeCoordinator() -> CrewdKnowledgeWebCoordinator { .init(runtime: runtime) }
    func makeNSView(context: Context) -> WKWebView { context.coordinator.make() }
    func updateNSView(_ view: WKWebView, context: Context) { context.coordinator.update() }
    static func dismantleNSView(_ view: WKWebView, coordinator: CrewdKnowledgeWebCoordinator) { coordinator.clear() }
}
#else
private struct CrewdKnowledgeWebSurface: UIViewRepresentable {
    let runtime: CrewdKnowledgeViewRuntime
    func makeCoordinator() -> CrewdKnowledgeWebCoordinator { .init(runtime: runtime) }
    func makeUIView(context: Context) -> WKWebView { context.coordinator.make() }
    func updateUIView(_ view: WKWebView, context: Context) { context.coordinator.update() }
    static func dismantleUIView(_ view: WKWebView, coordinator: CrewdKnowledgeWebCoordinator) { coordinator.clear() }
}
#endif

@MainActor private final class CrewdKnowledgeWebCoordinator: NSObject, WKScriptMessageHandlerWithReply, WKNavigationDelegate, WKUIDelegate, WKURLSchemeHandler {
    private let runtime: CrewdKnowledgeViewRuntime
    private weak var web: WKWebView?
    private var captured: CrewdKnowledgeViewContext?
    private var assets: [String: CrewdKnowledgeViewBundle.Asset] = [:]
    private var initialNavigation = false
    private var childNavigation = false
    private var tasks: [UUID: Task<Void, Never>] = [:]
    init(runtime: CrewdKnowledgeViewRuntime) { self.runtime = runtime }
    func make() -> WKWebView {
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.userContentController.addScriptMessageHandler(self, contentWorld: .world(name: "PendingCrewKnowledgeHost"), name: "knowledge")
        config.userContentController.addScriptMessageHandler(self, contentWorld: .world(name: "PendingCrewKnowledgeHost"), name: "knowledgeLifecycle")
        config.setURLSchemeHandler(self, forURLScheme: "pendingcrew-knowledge-resource")
        let view = WKWebView(frame: .zero, configuration: config)
        view.navigationDelegate = self; view.uiDelegate = self
        web = view
        update()
        return view
    }
    func clear() {
        captured = nil; assets.removeAll(); initialNavigation = false
        tasks.values.forEach { $0.cancel() }; tasks.removeAll()
        web?.stopLoading()
        web?.configuration.userContentController.removeScriptMessageHandler(forName: "knowledge", contentWorld: .world(name: "PendingCrewKnowledgeHost"))
        web?.configuration.userContentController.removeScriptMessageHandler(forName: "knowledgeLifecycle", contentWorld: .world(name: "PendingCrewKnowledgeHost"))
        web?.configuration.userContentController.removeAllUserScripts()
        web?.navigationDelegate = nil; web?.uiDelegate = nil
        // This clears the presentation. stopLoading is not claimed to terminate arbitrary JavaScript.
        web?.loadHTMLString("<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'\">", baseURL: nil)
    }
    func update() {
        runtime.synchronizeAuthority()
        guard let view = web else { return }
        guard let next = runtime.context, let bundle = runtime.bundle, !runtime.failed, runtime.executionEnabled else {
            if captured != nil {
                captured = nil; assets.removeAll(); tasks.values.forEach { $0.cancel() }; tasks.removeAll()
                view.stopLoading(); initialNavigation = true
                view.loadHTMLString("<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'\">", baseURL: nil)
            }
            return
        }
        guard captured != next else { return }
        tasks.values.forEach { $0.cancel() }; tasks.removeAll()
        captured = next; assets = bundle.assets; initialNavigation = true; childNavigation = false
        let nativeFile = view.uiDelegate === self && Self.fileDenialAvailable && responds(to: NSSelectorFromString("webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:"))
        let nativeMedia = view.uiDelegate === self && responds(to: NSSelectorFromString("webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:"))
        let nativeGeo = view.uiDelegate === self && Self.geolocationDenialAvailable && responds(to: NSSelectorFromString("webView:requestGeolocationPermissionForOrigin:initiatedByFrame:decisionHandler:"))
        let document = CrewdKnowledgeWebDocument(bundle: bundle, context: next, nativeFileDenial: nativeFile, nativeMediaDenial: nativeMedia, nativeGeolocationDenial: nativeGeo)
        let controller = view.configuration.userContentController
        controller.removeAllUserScripts()
        controller.addUserScript(WKUserScript(source: document.hostScript, injectionTime: .atDocumentEnd, forMainFrameOnly: true, in: .world(name: "PendingCrewKnowledgeHost")))
        view.loadHTMLString(document.hostHTML, baseURL: nil)
    }

    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage, replyHandler: @escaping (Any?, String?) -> Void) {
        runtime.synchronizeAuthority()
        guard let view = web, message.webView === view, message.frameInfo.isMainFrame,
              message.frameInfo.request.url?.absoluteString == "about:blank",
              let context = captured, runtime.context == context, runtime.executionEnabled,
              let text = message.body as? String else { replyHandler(nil, "Unavailable"); return }
        if message.name == "knowledgeLifecycle" {
            if text == "ready" { runtime.renderReady(); replyHandler("ready", nil); return }
            guard text == "frameChanged" || text == "policyUnsupported" else { replyHandler(nil, "Unavailable"); return }
            captured = nil; assets.removeAll(); runtime.renderFailed(resourcePolicyUnsupported: text == "policyUnsupported")
            replyHandler("stopped", nil); return
        }
        guard message.name == "knowledge", let request = try? CrewdKnowledgeViewRuntime.Request.decode(text) else { replyHandler(nil, "Unavailable"); return }
        let taskID = UUID()
        tasks[taskID] = Task { [weak self] in
            guard let self else { replyHandler(nil, "Unavailable"); return }
            defer { self.tasks.removeValue(forKey: taskID) }
            do {
                let result = try await self.runtime.handle(request, from: context)
                guard !Task.isCancelled, self.captured == context, self.runtime.context == context,
                      self.web === view else { replyHandler(nil, "Unavailable"); return }
                let data = try JSONEncoder().encode(result.value)
                guard data.count <= 1_048_576, let json = String(data: data, encoding: .utf8) else { replyHandler(nil, "Unavailable"); return }
                replyHandler(json, nil)
            } catch { replyHandler(nil, "Unavailable") }
        }
    }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard webView === web else { decisionHandler(.cancel); return }
        if initialNavigation, navigationAction.targetFrame?.isMainFrame == true,
           navigationAction.request.url?.absoluteString == "about:blank", navigationAction.navigationType == .other {
            initialNavigation = false; decisionHandler(.allow); return
        }
        // srcdoc is the one host-created child. Subsequent child loads are additionally
        // detected by the trusted isolated host script; no author popup/nested frame is admitted.
        if !childNavigation, navigationAction.sourceFrame.isMainFrame, navigationAction.targetFrame?.isMainFrame == false,
           navigationAction.request.url?.absoluteString == "about:srcdoc", navigationAction.navigationType == .other {
            childNavigation = true; decisionHandler(.allow); return
        }
        runtime.observedResourceDenial(.navigation)
        decisionHandler(.cancel)
        captured = nil; assets.removeAll(); runtime.renderFailed()
    }
    private static var fileDenialAvailable:Bool {
        #if os(iOS)
        if #available(iOS 18.4, *) {return true};return false
        #else
        return true
        #endif
    }
    private static var geolocationDenialAvailable:Bool {
        if #available(macOS 27.0,iOS 27.0,*) {return true};return false
    }
    @available(macOS 27.0,iOS 27.0,*)
    func webView(_ webView: WKWebView, requestGeolocationPermissionFor origin: WKSecurityOrigin, initiatedByFrame frame: WKFrameInfo, decisionHandler: @escaping (WKPermissionDecision)->Void) {
        if webView === web {runtime.observedResourceDenial(.geolocation)}
        decisionHandler(.deny)
    }
    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? { if webView === web { runtime.observedResourceDenial(.newWindow) }; return nil }
    func webView(_ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin, initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType, decisionHandler: @escaping (WKPermissionDecision) -> Void) { if webView === web { runtime.observedResourceDenial(.mediaCapture) }; decisionHandler(.deny) }
    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping () -> Void) { completionHandler() }
    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (Bool) -> Void) { completionHandler(false) }
    func webView(_ webView: WKWebView, runJavaScriptTextInputPanelWithPrompt prompt: String, defaultText: String?, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping (String?) -> Void) { completionHandler(nil) }
    @available(iOS 18.4, macOS 10.12, *)
    func webView(_ webView: WKWebView, runOpenPanelWith parameters: WKOpenPanelParameters, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping ([URL]?) -> Void) { if webView === web { runtime.observedResourceDenial(.filePanel) }; completionHandler(nil) }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        guard webView === web else { return }
        captured = nil; assets.removeAll(); tasks.values.forEach { $0.cancel() }; tasks.removeAll()
        runtime.renderFailed()
    }
    func webView(_ webView: WKWebView, start urlSchemeTask: WKURLSchemeTask) {
        guard webView === web, let context = captured, runtime.context == context, runtime.executionEnabled,
              let url = urlSchemeTask.request.url, url.scheme == "pendingcrew-knowledge-resource",
              url.host == context.instance.uuidString.lowercased(), url.query == nil, url.fragment == nil,
              url.user == nil, url.password == nil, url.port == nil,
              url.pathComponents.count == 2, let key = url.pathComponents.last,
              let asset = assets[key], asset.renderable else {
            urlSchemeTask.didFailWithError(URLError(.resourceUnavailable)); return
        }
        urlSchemeTask.didReceive(URLResponse(url: url, mimeType: asset.mime, expectedContentLength: asset.data.count, textEncodingName: nil))
        urlSchemeTask.didReceive(asset.data); urlSchemeTask.didFinish()
    }
    func webView(_ webView: WKWebView, stop urlSchemeTask: WKURLSchemeTask) { }
}
