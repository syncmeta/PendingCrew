给人和 agent 看的群内公告与注入文字（crewd-collab 的 render / asks / approvals / restart 读这里）。
每节 `## 名字`；`{占位符}` 由代码填，允许哪些见 src/prompts.rs 的 REQUIRED。

## notice.approval
{session} 想用工具 {tool}：{summary}
这次已当场拒绝，它会先去干别的。在审批列表里同意 = 放行它重跑这一次（一次性）；拒绝 = 它会收到「不行」。

## notice.restart
crewd 重新启动了。上次有 {count} 个 session 在跑，是否现在恢复它们？（先不恢复也行：在群里 @ 其中某一个，就只恢复它。）

## notice.restart.item
- {name}

## sender.human
人类

## sender.system
crewd

## sender.session
{name}（{ext}号）

## sender.session.unnumbered
{name}（成员）

## sender.captain
{name}（机长）

## inject.header
以下是你所在的群里的新消息（只含与你相关、你看得见的）：

## inject.channel.lobby
【{crew} · 大厅】

## inject.channel.human
【{crew} · 人类群】

## inject.channel.topic
【群：{name}】

## inject.channel.dm
【私聊】

## line
[{at}] {sender}：{text}

## line.directed
（发给 {targets} 的，不是派给你的）

## line.directed.unknown
（不是发给你的）

## line.reply
（回复 {reply}）

## wake.header
你被叫醒了，原因如下。

## wake.message
群里有需要你看的消息：

## wake.timer
你之前约的提醒到了。

## wake.ask_answer
人回答了你的问题 {ask}。
问题：{question}
答复：{answer}

## wake.ask_defaulted
你的问题 {ask} 到时没人回复，按默认方案办：{answer}
问题：{question}
到点后的动作照常过审批闸：被闸拒了就先干别的，人同意后会叫你回来重跑。

## wake.approval_allowed
人同意了你用 {tool}（{summary}）。现在重跑那一次调用会放行——只放行一次。

## wake.approval_denied
人没有同意你用 {tool}（{summary}）。换个不需要它的办法，或在群里说明为什么需要。

## wake.resume_note
你当时留的接续说明：{note}

## wake.restart
crewd 重启打断了你上一轮的工作。先核对当前状态（工作区改动、群里的新消息），再继续。

## wake.system
系统通知。

## wake.checkpoint
检查点到了。

## wake.stop_check
停下之前，请确认名下工作项都有下一步。

## wake.note
附言：{note}
