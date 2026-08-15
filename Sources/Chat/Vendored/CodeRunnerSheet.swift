// VENDORED from PendingBot apps/pendingbot/Sources/Components/CodeRunnerSheet.swift @ 43c8ea2e
// 再对齐 = 对照源文件重拷。仅允许偏离打 `// PENDINGCREW SHIM:` 标注。
#if os(iOS)
import SwiftUI
import WebKit

/// Modal sheet that runs a snippet in a sandboxed WKWebView. Mirrors the
/// web client's iframe sandbox: a fresh JS realm, no network / cookies,
/// a 5s wall-clock timeout, console output piped back into a SwiftUI panel.
///
/// Supports JS / JavaScript (wraps source in a try/catch + console hook)
/// and HTML (renders source as the page body). The bridge is one-way:
/// JS posts to a `runner` message handler that we read from native.
struct CodeRunnerSheet: View {
    let content: String
    let language: String

    @Environment(\.dismiss) private var dismiss
    @State private var lines: [LogLine] = []
    @State private var done = false
    @State private var hasError = false
    @State private var runId = UUID().uuidString
    @State private var nonce: Int = 0  // bump to force the WebView to re-mount on 重跑

    private var isHTML: Bool { language.lowercased() == "html" }

    var body: some View {
        NavigationStack {
            ZStack {
                Theme.Palette.canvas.ignoresSafeArea()
                VStack(spacing: 0) {
                    sourceHeader
                    Divider().background(Theme.Palette.hairline)
                    output
                    if isHTML {
                        Divider().background(Theme.Palette.hairline)
                        renderedPreview
                    }
                }
            }
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("关闭") { dismiss() }
                        .foregroundStyle(Theme.Palette.inkMuted)
                }
                ToolbarItem(placement: .principal) {
                    Text(isHTML ? "运行 HTML" : "运行 JavaScript")
                        .font(Theme.Fonts.serif(size: 17, weight: .semibold))
                        .foregroundStyle(Theme.Palette.ink)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button {
                        rerun()
                    } label: {
                        Label("重跑", systemImage: "arrow.clockwise")
                    }
                    .foregroundStyle(Theme.Palette.accent)
                }
            }
            .navigationBarTitleDisplayMode(.inline)
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    private var sourceHeader: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            Text(content)
                .font(Theme.Fonts.monospaced(size: 12.5))
                .foregroundStyle(Theme.Palette.ink)
                .padding(.horizontal, 14)
                .padding(.vertical, 8)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxHeight: 120)
        .background(Theme.Palette.surfaceMuted.opacity(0.6))
    }

    private var output: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if lines.isEmpty && !done {
                    HStack {
                        ProgressView().tint(Theme.Palette.accent)
                        Text("运行中…")
                            .font(Theme.Fonts.footnote)
                            .foregroundStyle(Theme.Palette.inkMuted)
                    }
                } else if lines.isEmpty {
                    Text("(无输出)")
                        .font(Theme.Fonts.footnote)
                        .foregroundStyle(Theme.Palette.inkMuted)
                } else {
                    ForEach(lines) { line in
                        Text(line.text)
                            .font(Theme.Fonts.monospaced(size: 13))
                            .foregroundStyle(line.isError ? Theme.Palette.danger : Theme.Palette.ink)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .textSelection(.enabled)
                    }
                }
            }
            .padding(14)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        // Hidden runner — it doesn't render anything visible; we pipe its
        // postMessage output into `lines`. Mounting + re-mounting is what
        // executes the snippet.
        .background(
            CodeRunnerWebView(
                source: content,
                runId: runId,
                isHTML: isHTML,
                nonce: nonce,
                onLog: { text in
                    lines.append(LogLine(text: text, isError: false))
                },
                onError: { text in
                    lines.append(LogLine(text: text, isError: true))
                    hasError = true
                },
                onDone: {
                    done = true
                }
            )
            .frame(width: 0, height: 0)
            .opacity(0)
        )
    }

    @ViewBuilder
    private var renderedPreview: some View {
        // For HTML, also show a visible WebView so the user can see the
        // rendered result, not just console output.
        VStack(alignment: .leading, spacing: 0) {
            Text("渲染")
                .font(Theme.Fonts.rounded(size: 11, weight: .medium))
                .foregroundStyle(Theme.Palette.inkMuted)
                .padding(.horizontal, 14)
                .padding(.top, 8)
            HTMLPreviewWebView(html: content, nonce: nonce)
                .frame(maxWidth: .infinity)
                .frame(height: 220)
        }
    }

    private func rerun() {
        lines.removeAll()
        done = false
        hasError = false
        runId = UUID().uuidString
        nonce += 1
    }

    private struct LogLine: Identifiable {
        let id = UUID()
        let text: String
        let isError: Bool
    }
}

// ── WKWebView wrapper ───────────────────────────────────────────────────────

private struct CodeRunnerWebView: UIViewRepresentable {
    let source: String
    let runId: String
    let isHTML: Bool
    let nonce: Int
    var onLog: (String) -> Void
    var onError: (String) -> Void
    var onDone: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onLog: onLog, onError: onError, onDone: onDone)
    }

    func makeUIView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        let userContent = WKUserContentController()
        userContent.add(context.coordinator, name: "runner")
        cfg.userContentController = userContent
        // Empty website data store keeps cookies / localStorage out of the
        // sandbox. Each runner gets its own ephemeral realm.
        cfg.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: cfg)
        view.isOpaque = false
        view.backgroundColor = .clear
        load(into: view, coordinator: context.coordinator)
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        if context.coordinator.lastNonce != nonce {
            context.coordinator.lastNonce = nonce
            load(into: view, coordinator: context.coordinator)
        }
    }

    private func load(into view: WKWebView, coordinator: Coordinator) {
        coordinator.armTimeout(view: view)
        view.loadHTMLString(buildHTML(), baseURL: nil)
    }

    private func buildHTML() -> String {
        // Match the web client: console.* and uncaught errors get piped
        // back via a JS bridge. JSON.stringify with a function fallback
        // so we don't trip over [object Object] and friends.
        let escSource = source
            .replacingOccurrences(of: "</script>", with: "<\\/script>")
            .replacingOccurrences(of: "<!--", with: "<\\!--")

        let bootstrap = """
        (function(){
          function fmt(a){
            if (a instanceof Error) return a.stack || (a.name+': '+a.message);
            if (typeof a === 'string') return a;
            try { return JSON.stringify(a, function(k,v){
              if (typeof v === 'function') return '[Function '+(v.name||'')+']';
              if (typeof v === 'undefined') return '[undefined]';
              return v;
            }, 2); } catch(_) { return String(a); }
          }
          function send(type, text){
            try { window.webkit.messageHandlers.runner.postMessage({type:type, text:text}); } catch(_){}
          }
          ['log','info','warn','error','debug','dir'].forEach(function(k){
            var prev = console[k];
            console[k] = function(){
              var parts = [];
              for (var i=0;i<arguments.length;i++) parts.push(fmt(arguments[i]));
              send('log', parts.join(' '));
              if (prev) try { prev.apply(console, arguments); } catch(_){}
            };
          });
          window.addEventListener('error', function(ev){
            send('error', (ev.error && (ev.error.stack || ev.error.message)) || ev.message || 'unknown error');
          });
          window.addEventListener('unhandledrejection', function(ev){
            send('error', 'Unhandled rejection: ' + fmt(ev.reason));
          });
        })();
        """

        if isHTML {
            return """
            <!doctype html><html><head><meta charset="utf-8">
            <script>\(bootstrap)</script>
            </head><body>
            \(escSource)
            <script>window.webkit.messageHandlers.runner.postMessage({type:'done'});</script>
            </body></html>
            """
        } else {
            return """
            <!doctype html><html><head><meta charset="utf-8"></head><body><script>
            \(bootstrap)
            try {
            \(escSource)
            } catch (e) {
              window.webkit.messageHandlers.runner.postMessage({type:'error', text:(e && (e.stack||e.message))||String(e)});
            }
            window.webkit.messageHandlers.runner.postMessage({type:'done'});
            </script></body></html>
            """
        }
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler {
        let onLog: (String) -> Void
        let onError: (String) -> Void
        let onDone: () -> Void
        var lastNonce: Int = -1
        weak var view: WKWebView?
        var timeoutTask: Task<Void, Never>?

        init(onLog: @escaping (String) -> Void,
             onError: @escaping (String) -> Void,
             onDone: @escaping () -> Void) {
            self.onLog = onLog
            self.onError = onError
            self.onDone = onDone
        }

        func armTimeout(view: WKWebView) {
            self.view = view
            timeoutTask?.cancel()
            timeoutTask = Task { [weak self] in
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                guard let self, !Task.isCancelled else { return }
                self.onError("⏱ 超时（5s），已中断")
                self.view?.stopLoading()
                self.onDone()
            }
        }

        func userContentController(_ userContentController: WKUserContentController,
                                    didReceive message: WKScriptMessage) {
            guard let dict = message.body as? [String: Any] else { return }
            let type = dict["type"] as? String ?? ""
            let text = dict["text"] as? String ?? ""
            switch type {
            case "log":   onLog(text)
            case "error": onError(text)
            case "done":
                timeoutTask?.cancel()
                onDone()
            default: break
            }
        }
    }
}

// ── HTML preview WebView ────────────────────────────────────────────────────

private struct HTMLPreviewWebView: UIViewRepresentable {
    let html: String
    let nonce: Int

    func makeUIView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        cfg.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: cfg)
        view.isOpaque = false
        view.backgroundColor = .white
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {
        view.loadHTMLString(html, baseURL: nil)
    }
}
#endif
