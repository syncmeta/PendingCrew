import Foundation

/// **汇报该送到谁** —— 跟「组织树上存着哪条边」是两件事（人类 Todo #141 / #137）。
///
/// ## 为什么要分成两件
/// #137 的原话是「总机组是全局的一**层**，顶层机组就是它的子机组」。照字面去
/// **存边**会长出一类没有尺子会响的 bug：13 个顶层 crew 各存一条指向总机组的
/// 父边，然后某个 crew 被 `adopt` 走，它就同时挂着真父和总机组两条边 ——
/// 得有人记得去掉那条；`release` 回顶层时又得记得加回来。忘一次，组织树上多一
/// 根假枝，而且查不出来。**一层不该有边。**
///
/// 所以总机组永远不作为一条**存下来的边**存在（`refuseBuiltin` 那四道一个字没动），
/// 「顶层机组向上汇报落到总机组」这件事改成**派生**出来。派生天然自洽：一个 crew
/// 拿到真父的那一刻，它就自动不再往总机组汇报，**没有任何人需要记得去改一条边**。
///
/// ## 为什么它是一个有名字的纯函数，而不是投递路径上的一个 if
/// 写成 if 就成了「把组织关系藏进投递逻辑」—— 看不见、测不到、第二个调用方会
/// 再写一遍。这里是一个点，三个分支各有一条测试压着。
enum CrewReportingParent {

    /// - Parameters:
    ///   - stored: 这个 crew **存下来的**父边（`LocalCrewStore.parentIds`）。
    ///   - selfId: 这个 crew 自己的 id —— 用来拦「总机组汇报给自己」那条自环。
    ///   - builtinLayerExists: 内建那一层在名册里有没有。**没有就不派生** ——
    ///     往一个不存在的 crew 汇报，消息会落进一本没人看的群聊，而回执仍说送达了。
    /// - Returns: 这次汇报该送到的那些 crew。空 = 真的没有上级，照旧回「已是根」。
    static func resolve(
        stored: [String],
        selfId: String,
        builtinLayerExists: Bool
    ) -> [String] {
        // 有真父就只给真父 —— 别顺手再往总机组抄送一份。
        guard stored.isEmpty else { return stored }
        // 总机组自己没有上级。派生出自己就是一条自环。
        guard selfId != LocalCrew.chiefCrewId else { return [] }
        guard builtinLayerExists else { return [] }
        return [LocalCrew.chiefCrewId]
    }
}
