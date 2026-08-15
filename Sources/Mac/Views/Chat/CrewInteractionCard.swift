import SwiftUI

/// ask_human 交互卡（payload.kind == "interaction"）。操作者就地答。
struct CrewInteractionCard: View {
    let entry: CrewWhiteboardEntry
    @Binding var reply: String
    let onAnswer: () -> Void

    private var reqId: String? { entry.payload?.permissionRequestId }
    private var pending: Bool { (entry.payload?.status ?? "pending") == "pending" }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Agent 在问你", systemImage: "person.fill.questionmark")
                .font(Theme.Fonts.caption.weight(.semibold))
                .foregroundStyle(Theme.Palette.amber)
            Text(entry.payload?.question ?? entry.displayText)
                .font(Theme.Fonts.callout)
                .foregroundStyle(Theme.Palette.ink)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if pending, reqId != nil {
                HStack(spacing: 8) {
                    TextField("回答…", text: $reply, axis: .vertical)
                        .textFieldStyle(.roundedBorder)
                        .lineLimit(1...4)
                        .onSubmit(onAnswer)
                    Button("回答", action: onAnswer)
                        .buttonStyle(.borderedProminent)
                        .tint(Theme.Palette.accent)
                        .disabled(reply.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                }
            } else if !pending {
                Text("已回答").font(Theme.Fonts.caption).foregroundStyle(Theme.Palette.inkMuted)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
            .fill(Theme.Palette.amberBg.opacity(0.5)))
        .overlay(RoundedRectangle(cornerRadius: Theme.Metrics.cardRadius, style: .continuous)
            .strokeBorder(Theme.Palette.amber.opacity(0.30), lineWidth: 0.6))
        .padding(.horizontal, 12)
        .padding(.vertical, 4)
    }
}
