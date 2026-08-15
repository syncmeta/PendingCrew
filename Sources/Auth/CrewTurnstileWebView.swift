// VENDORED from PendingBot apps/pendingbot/Sources/Features/Onboarding/TurnstileChallenge.swift
// 仅取 iOS TurnstileWebView (UIViewRepresentable) → CrewTurnstileWebView，#if os(iOS)
// 守护（macOS twin 在 Task 14 补）。WKWebView + Turnstile HTML bridge 逐字照搬。
// 再对齐 = 对照源文件重拷。
#if os(iOS)
import SwiftUI
@preconcurrency import WebKit

/// Embeddable Cloudflare Turnstile widget. Hosts a `WKWebView` running the
/// Turnstile JS SDK against `CrewHostedConfig.turnstileHost` (the hostname
/// registered for our site key). On a successful pass it reports a
/// short-lived (~5min) token to the caller, who forwards it to Supabase
/// Auth via `signInWithOTP(captchaToken:)`.
///
/// Why an in-app WebView instead of a native SDK: Cloudflare doesn't ship
/// an official iOS SDK and the WebView path is the recommended approach
/// for native apps. The Turnstile script enforces an origin check, so the
/// WebView's baseURL must match a hostname registered for the site key.
///
/// UX shape: this view is always laid out by the parent (so the SDK can
/// pre-load while the user is doing something else) but only takes up
/// real space when it actually needs to show an interactive challenge.
/// The parent observes `onInteractive` to know when to give it room.
/// The widget uses `appearance: 'interaction-only'` so silent passes
/// resolve without ever flashing the iframe. Bumping the `id(_:)` on the
/// SwiftUI wrapper recreates the WebView from scratch, which is the
/// path used to issue a fresh token on resend.
struct CrewTurnstileWebView: UIViewRepresentable {
    let siteKey: String
    let host: URL
    var onToken: (String) -> Void
    var onError: (String) -> Void
    var onInteractive: () -> Void

    func makeCoordinator() -> Coordinator {
        Coordinator(onToken: onToken, onError: onError, onInteractive: onInteractive)
    }

    func makeUIView(context: Context) -> WKWebView {
        let cfg = WKWebViewConfiguration()
        let userContent = WKUserContentController()
        userContent.add(context.coordinator, name: "turnstile")
        cfg.userContentController = userContent
        cfg.websiteDataStore = .nonPersistent()
        let view = WKWebView(frame: .zero, configuration: cfg)
        view.isOpaque = false
        view.backgroundColor = .clear
        view.scrollView.isScrollEnabled = false
        view.loadHTMLString(buildHTML(siteKey: siteKey), baseURL: host)
        return view
    }

    func updateUIView(_ view: WKWebView, context: Context) {}

    private func buildHTML(siteKey: String) -> String {
        // The Turnstile JS API script must be requested with the
        // `?onload=...` query so it calls our bootstrap once the SDK
        // finishes loading. `async defer` keeps the page responsive.
        return """
        <!doctype html>
        <html>
        <head>
          <meta charset="utf-8">
          <meta name="viewport" content="width=device-width, initial-scale=1">
          <script src="https://challenges.cloudflare.com/turnstile/v0/api.js?onload=onloadTurnstileCallback" async defer></script>
          <style>
            html, body { margin: 0; padding: 0; background: transparent; }
            body { display: flex; align-items: center; justify-content: center; min-height: 100vh; font-family: -apple-system, BlinkMacSystemFont, sans-serif; }
            #ts { display: flex; align-items: center; justify-content: center; }
          </style>
        </head>
        <body>
          <div id="ts"></div>
          <script>
            function send(type, payload) {
              try {
                window.webkit.messageHandlers.turnstile.postMessage(Object.assign({type: type}, payload || {}));
              } catch (_) {}
            }
            window.onloadTurnstileCallback = function() {
              try {
                turnstile.render('#ts', {
                  sitekey: '\(siteKey)',
                  appearance: 'interaction-only',
                  callback: function(token) { send('token', {token: token}); },
                  'error-callback': function(err) { send('error', {message: String(err)}); },
                  'expired-callback': function() { send('error', {message: 'expired'}); },
                  'timeout-callback': function() { send('error', {message: 'timeout'}); },
                  'before-interactive-callback': function() { send('interactive', {}); }
                });
              } catch (e) {
                send('error', {message: 'render: ' + (e && e.message || e)});
              }
            };
            setTimeout(function() {
              if (typeof turnstile === 'undefined') {
                send('error', {message: 'script failed to load'});
              }
            }, 10000);
          </script>
        </body>
        </html>
        """
    }

    @MainActor
    final class Coordinator: NSObject, WKScriptMessageHandler {
        let onToken: (String) -> Void
        let onError: (String) -> Void
        let onInteractive: () -> Void
        private var settled = false

        init(onToken: @escaping (String) -> Void,
             onError: @escaping (String) -> Void,
             onInteractive: @escaping () -> Void) {
            self.onToken = onToken
            self.onError = onError
            self.onInteractive = onInteractive
        }

        func userContentController(_ userContentController: WKUserContentController,
                                   didReceive message: WKScriptMessage) {
            guard let dict = message.body as? [String: Any] else { return }
            let type = dict["type"] as? String ?? ""
            switch type {
            case "token":
                guard !settled, let token = dict["token"] as? String else { return }
                settled = true
                onToken(token)
            case "error":
                guard !settled else { return }
                let msg = dict["message"] as? String ?? "unknown"
                onError(msg)
            case "interactive":
                onInteractive()
            default:
                break
            }
        }
    }
}
#endif
