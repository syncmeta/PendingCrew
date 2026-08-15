import Foundation

/// 每个 crew 记住自己没发出去的草稿（Todo #45 的配套）。
///
/// ## 为什么需要它
///
/// 切 crew 时群聊视图会**整个重建**（macOS 的 `.id(crewId)` / iPad 的 `.id(id)`）——
/// 不重建的话同一个视图实例被复用，在 A 群打一半的字会原样出现在 B 群的输入框里，
/// 可能误发。但重建等于 `@State` 全清，打一半的字点了下别的群回来就没了，同样是退步。
///
/// 两头都不接受，所以草稿不放在视图的 `@State` 里，改由这里按 crew 记着：视图重建时
/// 从这儿取回来。形状照抄 `CrewSessionRunner` 的 per-crew 右栏选中态（#481），别新造机制。
///
/// ## 只在内存里
///
/// app 重启草稿没了是可接受的 —— 不为它引入新的持久化文件。
///
/// 记的是「输入框里那条未发出的消息**整体**」：文本、已挂上的 @ 目标、正在回复谁。
/// 三者是一体的，只存文本会让恢复出来的 `@某人` 变成一串没有目标的字（看着 @ 了，
/// 发出去不 @ 任何人）。附件托盘不记 —— 附件是发送前临时挑的，切走清掉不算意外，
/// 而且它握着整份文件字节，攒在内存里不划算。
@MainActor
final class CrewComposerDraftStore {

    static let shared = CrewComposerDraftStore()

    struct Draft: Equatable {
        var text: String = ""
        /// 已挂上目标的 @（与 `text` 里的 `@token` 配对，发送时兑现）。
        var stagedMentions: [CrewStagedMention] = []
        /// 正在回复哪条（composer 上方的引用条 + 发送时的自动 @）。
        var replyTarget: CrewReplyTarget?

        /// 空草稿 = 什么都没留下，不占位。
        var isEmpty: Bool {
            text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                && stagedMentions.isEmpty && replyTarget == nil
        }
    }

    private var drafts: [String: Draft] = [:]

    init() {}

    /// 取某个 crew 记着的草稿；没有就是空草稿。
    func draft(for crewId: String) -> Draft { drafts[crewId] ?? Draft() }

    /// 存回去。空草稿直接删条目 —— 别让「打了又删光」的 crew 永久占着一格。
    func save(_ draft: Draft, for crewId: String) {
        if draft.isEmpty {
            drafts.removeValue(forKey: crewId)
        } else {
            drafts[crewId] = draft
        }
    }

    /// 发送成功后清掉（发出去了就不是草稿了）。
    func clear(_ crewId: String) { drafts.removeValue(forKey: crewId) }
}
