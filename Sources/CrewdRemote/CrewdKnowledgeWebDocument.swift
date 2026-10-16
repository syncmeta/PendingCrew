import Foundation

/// Only this host-generated document owns the WK main frame. Author source is
/// serialized into srcdoc data and never spliced into a top-level script or attribute.
struct CrewdKnowledgeWebDocument {
    static let deniedFeatures = "geolocation 'none'; camera 'none'; microphone 'none'; display-capture 'none'; clipboard-read 'none'; clipboard-write 'none'; fullscreen 'none'; payment 'none'; usb 'none'; serial 'none'; bluetooth 'none'"
    let hostHTML: String
    let hostScript: String
    init(bundle: CrewdKnowledgeViewBundle, context: CrewdKnowledgeViewContext, nativeFileDenial: Bool = false, nativeMediaDenial: Bool = false, nativeGeolocationDenial: Bool = false) {
        let nonce = UUID().uuidString.replacingOccurrences(of: "-", with: "")
        let basePolicy = "default-src 'none'; script-src 'nonce-\(nonce)'; style-src 'unsafe-inline'; img-src pendingcrew-knowledge-resource:; font-src pendingcrew-knowledge-resource:; connect-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; media-src 'none'"
        let childPolicy = basePolicy + "; frame-src 'none'; child-src 'none'"
        let authorSource = Self.literal(bundle.javascript)
        let childBootstrap = """
        (() => {
          const pending = new Map();
          let authorStarted = false;
          let releaseReady; const readiness = new Promise(resolve => { releaseReady = resolve; });
          const uuid = () => {
            const b = crypto.getRandomValues(new Uint8Array(16)); b[6] = (b[6] & 15) | 64; b[8] = (b[8] & 63) | 128;
            const h = Array.from(b, x => x.toString(16).padStart(2,'0')).join('');
            return h.slice(0,8)+'-'+h.slice(8,12)+'-'+h.slice(12,16)+'-'+h.slice(16,20)+'-'+h.slice(20);
          };
          window.addEventListener('message', event => {
            if (event.source !== parent || event.origin !== 'null') return;
            const d = event.data;
            if (d && d.channel === 'knowledge-ready-v1') {
              let opaqueParent = false; try { void parent.document; } catch (_) { opaqueParent = true; }
              const policy = document.permissionsPolicy || document.featurePolicy;
              const denied = name => {
                try { return !!policy && typeof policy.features === 'function' && policy.features().includes(name) && policy.allowsFeature(name) === false; } catch (_) { return false; }
              };
              parent.postMessage({channel:'knowledge-handshake-v1',opaqueParent,geolocationDenied:denied('geolocation'),cameraDenied:denied('camera'),microphoneDenied:denied('microphone')}, '*'); return;
            }
            if (d && d.channel === 'knowledge-accepted-v1') {
              if (authorStarted) return; authorStarted = true;
              releaseReady();
              // Author code is data until the trusted host has qualified this
              // exact child. No raw-function wrapper can escape this boundary.
              const script = document.createElement('script');
              script.nonce = '\(nonce)'; script.textContent = \(authorSource);
              document.body.appendChild(script); return;
            }
            if (!d || d.channel !== 'knowledge-response-v1' || typeof d.callID !== 'string') return;
            const p = pending.get(d.callID); if (!p) return; pending.delete(d.callID); clearTimeout(p.timer);
            if (typeof d.response === 'string') { try { p.resolve(JSON.parse(d.response)); } catch (_) { p.reject(new Error('Unavailable')); } }
            else p.reject(new Error('Unavailable'));
          });
          Object.defineProperty(window, 'knowledge', {value:Object.freeze({
            request: (operation, input) => {
              const callID = uuid();
              return new Promise((resolve,reject) => {
                if (pending.size >= 4) { reject(new Error('Busy')); return; }
                const timer = setTimeout(() => { pending.delete(callID); reject(new Error('Unknown; query original callID')); }, 65000);
                pending.set(callID,{resolve,reject,timer});
                readiness.then(() => { if (pending.has(callID)) parent.postMessage({channel:'knowledge-request-v1',payload:JSON.stringify({callID,operation,input})}, '*'); });
              }).then(response => ({callID,response}));
            },
            resource: key => 'pendingcrew-knowledge-resource://\(context.instance.uuidString.lowercased())/' + encodeURIComponent(key)
          }),writable:false,configurable:false});
        })();
        """
        let script = childBootstrap.replacingOccurrences(of: "</script", with: "<\\/script", options: .caseInsensitive)
        let css = bundle.css.replacingOccurrences(of: "</style", with: "<\\/style", options: .caseInsensitive)
        let child = "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"\(childPolicy)\"><meta name=\"referrer\" content=\"no-referrer\"><style>\(css)</style></head><body>\(bundle.html)<script nonce=\"\(nonce)\">\(script)</script></body></html>"
        hostHTML = "<!doctype html><html><head><meta charset=\"utf-8\"><meta http-equiv=\"Content-Security-Policy\" content=\"\(basePolicy); frame-src about:\"><meta name=\"referrer\" content=\"no-referrer\"><style>html,body{margin:0;width:100%;height:100%}iframe{display:block;border:0;width:100%;height:100%}</style></head><body><main id=\"knowledge-host\"></main></body></html>"
        let source = Self.literal(child)
        let features = Self.literal(Self.deniedFeatures)
        hostScript = """
        (() => {
          'use strict';
          const root = document.getElementById('knowledge-host');
          if (!root || document.querySelector('iframe')) return;
          const frame = document.createElement('iframe');
          frame.setAttribute('sandbox', 'allow-scripts');
          frame.setAttribute('allow', \(features));
          frame.setAttribute('referrerpolicy', 'no-referrer');
          let ready = false, qualified = false, loaded = false, stopped = false, inflight = 0;
          const stop = reason => {
            if (stopped) return; stopped = true; ready = false; qualified = false;
            frame.remove();
            window.webkit.messageHandlers.knowledgeLifecycle.postMessage(reason).catch(() => {});
          };
          // Attribute validity and opaque origin are necessary independent gates;
          // source/DI is not claimed to prove every installed WebKit policy enforcement.
          if (!frame.sandbox.supports('allow-scripts') || frame.sandbox.length !== 1 || !frame.sandbox.contains('allow-scripts') || frame.allow !== \(features)) { stop('policyUnsupported'); return; }
          frame.addEventListener('load', () => {
            if (loaded) { stop('frameChanged'); return; }
            loaded = true; ready = true;
            frame.contentWindow.postMessage({channel:'knowledge-ready-v1'}, '*');
          });
          window.addEventListener('message', async event => {
            if (stopped || !ready || event.source !== frame.contentWindow || event.origin !== 'null') return;
            const d = event.data;
            if (d && Object.getPrototypeOf(d) === Object.prototype && Object.keys(d).sort().join(',') === 'cameraDenied,channel,geolocationDenied,microphoneDenied,opaqueParent' && d.channel === 'knowledge-handshake-v1') {
              // Parent access must really be opaque. Missing policy introspection
              // is not denial: a supported public native deny callback is the
              // alternative. No permission or resource request is made here.
              if (d.opaqueParent !== true || !\(nativeFileDenial ? "true":"false") ||
                  !(d.geolocationDenied === true || \(nativeGeolocationDenial ? "true":"false")) ||
                  !((d.cameraDenied === true && d.microphoneDenied === true) || \(nativeMediaDenial ? "true":"false"))) { stop('policyUnsupported'); return; }
              if (!qualified) {
                try {
                  const response = await window.webkit.messageHandlers.knowledgeLifecycle.postMessage('ready');
                  if (!stopped && ready && response === 'ready') { qualified = true; frame.contentWindow.postMessage({channel:'knowledge-accepted-v1'}, '*'); }
                } catch (_) { stop('policyUnsupported'); }
              }
              return;
            }
            if (!qualified || !d || Object.getPrototypeOf(d) !== Object.prototype || Object.keys(d).sort().join(',') !== 'channel,payload' || d.channel !== 'knowledge-request-v1' || typeof d.payload !== 'string' || d.payload.length > 65536 || inflight >= 4) return;
            let id;
            try {
              const p = JSON.parse(d.payload);
              if (!p || Object.keys(p).sort().join(',') !== 'callID,input,operation' || !['read','write','lookup','navigate'].includes(p.operation) || typeof p.callID !== 'string' || !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(p.callID) || !p.input || Array.isArray(p.input) || typeof p.input !== 'object') return;
              id = p.callID; inflight++;
              const response = await window.webkit.messageHandlers.knowledge.postMessage(d.payload);
              if (!stopped && ready) frame.contentWindow.postMessage({channel:'knowledge-response-v1',callID:id,response}, '*');
            } catch (_) {
              if (!stopped && ready && id) frame.contentWindow.postMessage({channel:'knowledge-response-v1',callID:id,error:'Unavailable'}, '*');
            } finally { if (id) inflight--; }
          });
          frame.srcdoc = \(source);
          root.appendChild(frame);
        })();
        """
    }
    private static func literal(_ value: String) -> String {
        // JSON string encoding plus HTML/JS line-separator escaping. Never raw source interpolation.
        String(decoding: try! JSONEncoder().encode(value), as: UTF8.self)
            .replacingOccurrences(of: "<", with: "\\u003c")
            .replacingOccurrences(of: "\u{2028}", with: "\\u2028")
            .replacingOccurrences(of: "\u{2029}", with: "\\u2029")
    }
}
