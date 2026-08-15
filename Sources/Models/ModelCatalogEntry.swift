import Foundation

/// 一行 `GET /v1/models` 返回。对应 edge `OpenRouterModelOut`
/// (apps/edge/src/routes/models.ts) —— 只挑 CreateSessionSheet 真正用到的
/// 字段(slug / display_name / provider / source),其余等到右栏 session 详情
/// 真要展示价格 / context 时再补。
///
/// 解码用 snake_case key — PendingCrewAPI.perform() 不开 keyDecodingStrategy
/// (会破坏 crews 端已经 camelCase 的返回),所以这里显式 CodingKeys。
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
