import Foundation

/// 一行可用模型目录条目。只挑 CreateSessionSheet 真正用到的字段
/// (slug / display_name / provider / source)。
///
/// ⚠️ **这个类型当前没有任何消费者**：唯一引用它的是
/// `PendingCrewBackend.listModels()`，而那个方法**在 #63 第二期之前就已经
/// 没有调用方了** —— 新建 session 页早已改读本机实探的 `ModelCatalogCenter`
/// （`AgentModelCatalog` 那套 models.json，形状不同）。
///
/// 它不在 #63 第二期的删除范围里（不是遥控/登录层的东西，是那一刀之前就落下的
/// 死代码），所以这一期只登记不动手。见 docs/tech-debt.md。
struct ModelCatalogEntry: Decodable, Equatable, Hashable, Identifiable {
    let slug: String
    let displayName: String
    let provider: String
    let source: String        // "openrouter" | "openai" | "anthropic" | "google-ai-studio"
    let modelProvider: String?

    var id: String { slug }

    enum CodingKeys: String, CodingKey {
        case slug
        case displayName = "display_name"
        case provider
        case source
        case modelProvider = "model_provider"
    }

    /// "native" 行(Anthropic / Google / OpenAI 自家直连) vs OpenRouter 长尾。
    /// 仅 UI 分组用。
    var isNative: Bool { source != "openrouter" }
}
