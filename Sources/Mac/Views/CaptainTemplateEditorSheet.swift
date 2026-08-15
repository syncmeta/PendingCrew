#if os(macOS)
import SwiftUI

/// 新建本机 captain 模板的 sheet。
///
/// 字段:
/// - 名称 — 必填,后续创建 crew 时显示用
/// - 模型 — 必填,provider-qualified id(预置三个常用,可手填覆盖)
/// - system prompt — 可选
///
/// 保存后通过 `onCreated` 回调通知父 view(`CreateCrewSheet`)新模板已建,
/// 由父 view 决定要不要立即选中它。
///
/// **scope**:本 sheet 不做模型 ping/验证 —— 模板正确性等到真正发起 LLM
/// 调用时再暴露,provider 自己 401/422,UI 透传错误。
struct CaptainTemplateEditorSheet: View {
    @EnvironmentObject private var store: CaptainTemplateStore
    @Environment(\.dismiss) private var dismiss

    var onCreated: (CaptainTemplate) -> Void

    @State private var name: String = ""
    @State private var model: String = "anthropic/claude-opus-4-8"
    @State private var systemPrompt: String = ""

    /// 三个 provider 各放一个常用 model 作为预设。手填也允许(可以覆盖)。
    private let presetModels = [
        "anthropic/claude-opus-4-8",
        "anthropic/claude-sonnet-5",
        "openai/gpt-4o-mini",
        "openai/gpt-4o",
        "google/gemini-2.0-flash",
        "google/gemini-2.5-pro"
    ]

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            form
            Divider()
            footer
        }
        .frame(width: 480)
        .frame(minHeight: 400)
    }

    private var header: some View {
        HStack {
            Text("新建 Captain 模板")
                .font(.title3.weight(.semibold))
            Spacer()
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 14)
    }

    private var form: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("名称").font(.callout.weight(.medium))
                    TextField("例如:Code Reviewer", text: $name)
                        .textFieldStyle(.roundedBorder)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text("模型").font(.callout.weight(.medium))
                    Picker("", selection: $model) {
                        ForEach(presetModels, id: \.self) { m in
                            Text(m).tag(m)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    TextField("或手填 (provider/model)", text: $model)
                        .textFieldStyle(.roundedBorder)
                        .font(.system(.callout, design: .monospaced))
                    Text("格式:provider/model。provider 必须是 anthropic / openai / google。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                VStack(alignment: .leading, spacing: 6) {
                    Text("System prompt(可选)").font(.callout.weight(.medium))
                    TextEditor(text: $systemPrompt)
                        .font(.system(.callout, design: .monospaced))
                        .frame(minHeight: 80, maxHeight: 140)
                        .border(.tertiary, width: 0.5)
                    Text("captain 的初始 system prompt;留空则用 PendingCrew 默认机长 prompt(待接通 LLM provider)。")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            .padding(20)
        }
    }

    private var footer: some View {
        HStack(spacing: 8) {
            Spacer()
            Button("取消") { dismiss() }
                .keyboardShortcut(.cancelAction)
            Button("保存") { save() }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
                .disabled(!canSave)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 12)
    }

    private var canSave: Bool {
        let nameOk = !name.trimmingCharacters(in: .whitespaces).isEmpty
        let modelOk = model.contains("/")
            && !model.trimmingCharacters(in: .whitespaces).isEmpty
        return nameOk && modelOk
    }

    private func save() {
        let trimmedName = name.trimmingCharacters(in: .whitespaces)
        let trimmedModel = model.trimmingCharacters(in: .whitespaces)
        let trimmedPrompt = systemPrompt.trimmingCharacters(in: .whitespacesAndNewlines)
        let template = store.create(
            displayName: trimmedName,
            model: trimmedModel,
            systemPrompt: trimmedPrompt.isEmpty ? nil : trimmedPrompt
        )
        onCreated(template)
        dismiss()
    }
}
#endif
