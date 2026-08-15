import Foundation
#if os(macOS)
import SwiftUI
import AppKit
#endif

/// 进程入口（spec local-first chunk 4：re-exec self）。
///
/// 同一个 app 二进制有两副身份：
/// - 带 `--mcp-serve` / `--mcp-hook` argv（claude 经 `--mcp-config` / `--settings`
///   的 hook 拉起）→ 当 **crew-comms helper** 跑（stdio MCP server / 注入未读白板），
///   不起 GUI。比 embed 独立 executable 更自包含（就一个二进制、`Bundle.main.
///   executablePath` 铁定可寻），也避开 macOS app bundle 嵌可执行文件的签名/拷贝坑。
/// - 否则 → 起正常 SwiftUI GUI（`PendingCrewApp.main()`）。
@main
struct PendingCrewEntry {
    static func main() {
        // 未捕获 NSException 留痕（2026-07-26 布局自激闪退：.ips 里连异常名都没有）。
        // helper 分支之前挂 —— helper 子进程崩了同样要留痕。
        UncaughtExceptionLog.install()
        // 抬 fd 软上限（2026-08-12 P0 的触发闸：launchd 给 GUI app 的默认软上限
        // 只有 256，批量读一次顶穿 → EMFILE → 被误判成文件损坏）。放在 helper 分支
        // **之前** —— 同一个二进制的 helper 子进程压的是同一批文件，一样要抬。
        FileDescriptorLimit.raiseSoftLimitToHardLimit()
        if McpHelperMain.runIfHelper(CommandLine.arguments) { return }
        #if os(macOS)
        if MainActor.assumeIsolated({ renderTranscriptSnapshotIfRequested(CommandLine.arguments) }) { return }
        #endif
        PendingCrewApp.main()
    }

    #if os(macOS)
    /// Dev 工具：`PendingCrew --render-snapshot <dir>` 用 `ImageRenderer` 把真
    /// `CodexTranscriptView` 喂一组代表性 item，headless 出 light/dark PNG 后退出。
    /// 本地新 build 够不到登录态 keychain、进不到真 codex 回合，这条让我们仍能审阅
    /// transcript 渲染（降噪样式）。无 argv 时返回 false，照常起 GUI。
    @MainActor
    static func renderTranscriptSnapshotIfRequested(_ argv: [String]) -> Bool {
        guard let i = argv.firstIndex(of: "--render-snapshot"), i + 1 < argv.count else { return false }
        let dir = argv[i + 1]
        _ = NSApplication.shared   // ImageRenderer 需要 AppKit 起来

        for (scheme, name) in [(ColorScheme.light, "codex-transcript-light"),
                               (ColorScheme.dark, "codex-transcript-dark")] {
            let t = CodexTranscript()
            t.apply(method: "item/completed", params: ["item": [
                "id": "u1", "type": "userMessage",
                "content": [["type": "text", "text": "帮我确认 turn/start 的白板注入有没有走对通道"]]]])
            t.apply(method: "item/completed", params: ["item": [
                "id": "r1", "type": "reasoning",
                "summary": ["用户想确认白板注入。先 grep additionalContext 看 turnStartParams 现在的形状，再对照 codex 0.137.0 schema 是不是 {key:{value,kind}}；若还是前置 text input 就是旧错法。"]]])
            t.apply(method: "item/completed", params: ["item": [
                "id": "c1", "type": "commandExecution",
                "command": "grep -n additionalContext CodexProtocol.swift",
                "exitCode": 0,
                "aggregatedOutput": "47:    static func turnStartParams(threadId: String, text: String, whiteboard: String?) -> [String: Any] {\n48:        var p: [String: Any] = [\"threadId\": threadId, \"input\": [...]]\n49:        if let wb = whiteboard, !wb.isEmpty {\n50:            p[\"additionalContext\"] = [\"crew_whiteboard\": [\"value\": wb, \"kind\": \"untrusted\"]]\n51:        }\n52:        return p\n53:    }\n— matched 1 file"]])
            t.apply(method: "item/completed", params: ["item": [
                "id": "t1", "type": "mcpToolCall",
                "server": "crew", "tool": "read_whiteboard", "status": "completed"]])
            t.apply(method: "item/completed", params: ["item": [
                "id": "f1", "type": "fileChange",
                "changes": [["path": "CodexProtocol.swift"]], "status": "completed"]])
            t.apply(method: "item/completed", params: ["item": [
                "id": "a1", "type": "agentMessage", "phase": "final_answer",
                "text": "对的，白板走的是 codex 原生 **`additionalContext`**（`kind: \"untrusted\"`）—— 不塞进 `input`：\n\n- **角色**：developer 侧外部上下文，不冒充用户输入\n- **去重**：`AdditionalContextStore::merge` 按 key，白板没变就零注入\n\n要不要我再起一轮真机核字段被 honor？"]])

            let view = CodexTranscriptRows(transcript: t, lazy: false)
                .frame(width: 600)
                .background(scheme == .dark ? Color(red: 0.12, green: 0.12, blue: 0.13) : Color.white)
                .environment(\.colorScheme, scheme)
            let renderer = ImageRenderer(content: view)
            renderer.scale = 2
            if let img = renderer.nsImage,
               let tiff = img.tiffRepresentation,
               let bm = NSBitmapImageRep(data: tiff),
               let png = bm.representation(using: .png, properties: [:]) {
                let url = URL(fileURLWithPath: dir).appendingPathComponent("\(name).png")
                try? png.write(to: url)
                print("SNAPSHOT: \(url.path) \(png.count)B")
            } else {
                print("SNAPSHOT FAILED: \(name)")
            }
        }
        return true
    }
    #endif
}
