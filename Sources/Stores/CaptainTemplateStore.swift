import Foundation

/// 本机 captain 模板池(spec v2 §5.2 "本机 captain 池")。
///
/// **scope**:仅 BYOK 模式用。登录态走 PendingBot 真 bot 库
/// (`GET /v1/me/bots`,后续 task 接通);未登录时没有真 bot 概念,只能在
/// 本机存一组"captain 模板",新建 crew 时选一个生成 captain bot 实例。
///
/// **设计取舍**(跟 LocalCrewStore 对齐,见 LocalCrewStore.swift 顶部注释):
/// - 不引入 SQLite/GRDB,JSON 文件够用(模板数 << 100)
/// - 文件位置:`~/Library/Application Support/PendingCrew/captain-templates.json`
/// - id 用 UUID v4,创建/删除粒度
/// - 每次 mutation atomic write 防半截
///
/// **shape**:`CaptainTemplate` 是"创建 captain 时的预填快照",不是"已实例化
/// 的 captain bot" —— 后者是 LocalCrewStore 里 crew 的 captainBotId / captainName。
/// 模板字段最小化:
/// - `id` —— 内部 key
/// - `displayName` —— 用户填的好记的名字 ("Code Reviewer", "API Designer")
/// - `model` —— provider qualified ("anthropic/claude-opus-4-7" /
///   "openai/gpt-4o-mini" / "google/gemini-2.0-flash");后续真接 LLM 调用
///   时按这个字符串路由到对应 provider key
/// - `systemPrompt` —— 可选,创建 crew 时把它当 captain 的初始 system
///   prompt(本 task scope 内只存,真用要等 LLM provider 接通)
/// - `createdAt` —— ISO8601,排序用
///
/// **MainActor**:跟 LocalCrewStore 一致,所有 I/O 走 UI 线程(量小不卡 frame)。
@MainActor
final class CaptainTemplateStore: ObservableObject {
    static let shared = CaptainTemplateStore()

    @Published private(set) var templates: [CaptainTemplate] = []

    private let fileURL: URL

    /// 测试时传 tmp 目录,生产传 nil 用默认 Application Support。
    init(baseDirectory: URL? = nil) {
        let base: URL
        if let baseDirectory {
            base = baseDirectory
        } else {
            let support = FileManager.default
                .urls(for: .applicationSupportDirectory, in: .userDomainMask)
                .first ?? URL(fileURLWithPath: NSTemporaryDirectory())
            base = support.appendingPathComponent("PendingCrew", isDirectory: true)
        }
        do {
            try FileManager.default.createDirectory(
                at: base,
                withIntermediateDirectories: true
            )
        } catch {
            NSLog("[CaptainTemplateStore] mkdir failed: \(error)")
        }
        self.fileURL = base.appendingPathComponent("captain-templates.json")
        loadFromDisk()
    }

    // MARK: - Public API

    /// 顺序按 createdAt DESC,跟 LocalCrewStore.listCrews() 一致。
    func list() -> [CaptainTemplate] {
        templates.sorted { $0.createdAt > $1.createdAt }
    }

    /// 新建模板。返回完整对象(含自生成 id + createdAt)。
    @discardableResult
    func create(displayName: String, model: String, systemPrompt: String?) -> CaptainTemplate {
        let now = ISO8601DateFormatter().string(from: Date())
        let template = CaptainTemplate(
            id: "tpl-" + UUID().uuidString.lowercased(),
            displayName: displayName,
            model: model,
            systemPrompt: systemPrompt,
            createdAt: now
        )
        templates.append(template)
        persistToDisk()
        return template
    }

    /// 删除模板。
    func delete(_ id: String) {
        templates.removeAll { $0.id == id }
        persistToDisk()
    }

    /// 全清(切回 WelcomeView / 调试)。
    func clearAll() {
        templates.removeAll()
        persistToDisk()
    }

    // MARK: - Persistence

    private func loadFromDisk() {
        guard FileManager.default.fileExists(atPath: fileURL.path) else {
            return
        }
        do {
            let data = try Data(contentsOf: fileURL)
            let payload = try JSONDecoder().decode(CaptainTemplateFile.self, from: data)
            templates = payload.templates
        } catch {
            // 跟 LocalCrewStore 同款 corrupt 处理:备份原文件再当空 store 启动。
            let backup = fileURL.appendingPathExtension(
                "corrupt-\(Int(Date().timeIntervalSince1970))"
            )
            try? FileManager.default.moveItem(at: fileURL, to: backup)
            NSLog("[CaptainTemplateStore] decode failed (\(error)), backed up to \(backup.lastPathComponent)")
        }
    }

    private func persistToDisk() {
        let payload = CaptainTemplateFile(version: 1, templates: templates)
        do {
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
            let data = try encoder.encode(payload)
            try data.write(to: fileURL, options: [.atomic])
        } catch {
            NSLog("[CaptainTemplateStore] persist failed: \(error)")
        }
    }
}

// MARK: - Model

/// 本机 captain 模板的快照表示。Equatable + Identifiable 给 SwiftUI
/// ForEach / Picker 用,Codable 给 JSON 文件用。
struct CaptainTemplate: Codable, Equatable, Identifiable, Hashable {
    let id: String
    let displayName: String
    /// provider-qualified model id。预期值:
    /// - `anthropic/claude-opus-4-7`
    /// - `openai/gpt-4o-mini`
    /// - `google/gemini-2.0-flash`
    /// 后续真接 LLM provider 时按 `/` 拆 provider + model。
    let model: String
    let systemPrompt: String?
    let createdAt: String
}

private struct CaptainTemplateFile: Codable {
    let version: Int
    let templates: [CaptainTemplate]
}
