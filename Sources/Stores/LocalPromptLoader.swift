import Foundation

/// 加载 bundle 里的 crew prompt（PendingCrew 本地自带，开源核心；spec
/// 2026-06-05-pendingcrew-local-first-crew-design §4 prompts 全本地）。
///
/// 文件命名 `Resources/Prompts/<name>.<locale>.md`（扁平 locale-in-name，避免
/// group 打平后 zh/en 同名冲突）。读出后用 `PromptTemplate` 填 `{{var}}`。
/// 本地世界观（work session）+ captain persona（captain-session）都走这里，不依赖
/// edge / Langfuse。
struct LocalPromptLoader {
    let bundle: Bundle

    init(bundle: Bundle = .main) {
        self.bundle = bundle
    }

    enum LoaderError: Error, Equatable {
        case notFound(name: String, locale: String)
    }

    /// 读原始模板（不替换）。先试 `<locale>`，再回退 `zh`。
    func rawTemplate(name: String, locale: String) throws -> String {
        for loc in candidateLocales(locale) {
            if let url = resourceURL(name: name, locale: loc),
               let text = try? String(contentsOf: url, encoding: .utf8) {
                return text
            }
        }
        throw LoaderError.notFound(name: name, locale: locale)
    }

    /// 读模板 + 填 `{{var}}`。
    func render(name: String, locale: String, vars: [String: String]) throws -> String {
        PromptTemplate.render(try rawTemplate(name: name, locale: locale), vars: vars)
    }

    // MARK: - Internals

    private func candidateLocales(_ locale: String) -> [String] {
        locale == "zh" || locale.isEmpty ? ["zh"] : [locale, "zh"]
    }

    /// 多形 lookup —— 兼容 xcodegen 把 Resources 当 group 打平（扁平根）或
    /// 保留 `Prompts/` 子目录两种 bundling。
    private func resourceURL(name: String, locale: String) -> URL? {
        let flat = "\(name).\(locale)"
        return bundle.url(forResource: flat, withExtension: "md")
            ?? bundle.url(forResource: flat, withExtension: "md", subdirectory: "Prompts")
    }
}
