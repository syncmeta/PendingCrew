给人和 agent 看的群内公告与注入文字（crewd-collab 的 render / asks / approvals / restart 读这里）。
每节 `## 名字`；`{占位符}` 由代码填，允许哪些见 src/prompts.rs 的 REQUIRED。

## notice.approval
{session} 想用工具 {tool}：{summary}
这次已当场拒绝，它会先去干别的。在审批列表里同意 = 放行它重跑这一次（一次性）；拒绝 = 它会收到「不行」。

## notice.restart
crewd 重新启动了。上次有 {count} 个 session 在跑，是否现在恢复它们？（先不恢复也行：在群里 @ 其中某一个，就只恢复它。）

## notice.restart.item
- {name}
