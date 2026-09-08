import Foundation

/// 一次调用发**多条**群消息（人类 Todo #115 后半）。
///
/// 人类原话：「现在每次似乎都只能发一条整消息 我希望每一次能分开发几条 不然所有的
/// 比如一次汇报 把各种东西都揉在一条消息里 这就不好 **这是个重要的改变**」。
///
/// 一次汇报因此变成几个气泡 + 几本账各落一笔：
/// ```
/// [progress]    装 0.1.27 做完了，账本 47→11。
/// [human_todo]  侧栏那个黄色胶囊的像素没人看过，你瞄一眼。
/// [question]    迁移工作目录要不要也一起拆？
/// ```
///
/// ## 为什么**先全部校验、再逐条执行**
///
/// 分条之后失败有了「一半」这个新形态：第 3 条参数写错时，前两条**已经发出去了**。
/// 而这一单最坏的结果一直是「消息出去了、账没落上」——分条把它变成「**有些**出去了、
/// 有些没有」，更难看清。所以校验是**全有或全无**：任何一条不合法，整批拒、一条不发。
///
/// 真正执行时仍可能中途失败（IO），那时**回执必须逐条说清哪几条落了、哪几条没落** ——
/// 绝不许回一句「已发送」。
enum CrewMessageBatch {

    /// 一次最多几条。**不是性能限制，是产品判断**：人类抱怨的是「太多太乱」，
    /// 一次放出十几个气泡只是把一堵墙拆成一排墙。分不出 6 条以内的那次汇报，
    /// 多半本来就该分两次说。
    static let maxEntries = 6

    struct Entry {
        /// 在 `messages` 数组里的下标 —— 错误信息要能指名道姓「第几条」。
        let index: Int
        let text: String
        /// 这一条自己的参数（category / todo / todo_status / mentions …）。
        let args: [String: Any]
    }

    enum Decision {
        /// 老形态：单条，走原来那条路。
        case single
        case batch([Entry])
        case refuse(String)
    }

    static func parse(args: [String: Any]) -> Decision {
        let rawMessages = args["messages"]
        guard rawMessages != nil else { return .single }

        let single = (args["message"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !single.isEmpty {
            return .refuse("`message` 和 `messages` 只能给一个。"
                + "\n给了 `messages` 就把每一条都放进数组里（每条自己带 `text` 和 `category`）；"
                + "\n只发一条就用 `message`。**两个都给的话，没人知道你想发几条。**")
        }
        guard let list = rawMessages as? [[String: Any]] else {
            return .refuse("`messages` 要是一个数组，每项是一个对象（至少有 `text`）。"
                + "\n例：`[{\"text\":\"闸门全绿\",\"category\":\"progress\",\"plan\":3},"
                + "{\"text\":\"这条要你拍板\",\"category\":\"human_todo\"}]`")
        }
        guard !list.isEmpty else {
            return .refuse("`messages` 是空数组 —— 一条都没有，等于什么都没发。"
                + "\n真要发就放进去；只发一条用 `message`。")
        }
        guard list.count <= maxEntries else {
            return .refuse("一次最多 \(maxEntries) 条，你给了 \(list.count) 条。"
                + "\n**这不是性能限制**：人类抱怨的是「群里消息太多太乱」，"
                + "一次放出十几个气泡只是把一堵墙拆成一排墙。"
                + "\n出路：把最要紧的那几条这次发，其余的等有结果了再说。")
        }

        var entries: [Entry] = []
        for (i, item) in list.enumerated() {
            let text = (item["text"] as? String)?
                .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            guard !text.isEmpty else {
                return .refuse("`messages` 第 \(i + 1) 条没有 `text`（或只有空白）。"
                    + "\n**整批都没有发出去** —— 分条发送的校验是全有或全无，"
                    + "免得前几条已经出去了、后面那条才发现写错。")
            }
            entries.append(Entry(index: i, text: text, args: item))
        }
        return .batch(entries)
    }

    /// 逐条执行时用的**如实回执**。
    ///
    /// `sent` / `failed` 都按「第几条」列出来。**绝不许在有失败时只回一句「已发送」** ——
    /// 分条之后「一半成功」是常态形态，而它恰恰是最容易被读成「全成了」的那种。
    static func batchReceipt(sent: [Int], failed: [(index: Int, why: String)]) -> String {
        var lines: [String] = []
        if !sent.isEmpty {
            lines.append("已发出 \(sent.count) 条（第 " + sent.map { String($0 + 1) }
                .joined(separator: "、") + " 条）。")
        }
        guard !failed.isEmpty else { return lines.joined(separator: "\n") }
        lines.append("⚠️ **有 \(failed.count) 条没发出去**，逐条说明：")
        for f in failed {
            lines.append("  · 第 \(f.index + 1) 条：\(f.why)")
        }
        lines.append("没发出去的那几条**没有留在任何地方** —— 要它们的话得重发。")
        return lines.joined(separator: "\n")
    }
}
