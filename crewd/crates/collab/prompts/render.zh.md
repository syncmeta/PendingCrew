给人和 agent 看的群内公告与注入文字（crewd-collab 的 render / asks / restart 读这里）。
每节 `## 名字`；`{占位符}` 由代码填，允许哪些见 src/prompts.rs 的 REQUIRED。

## notice.restart
crewd 重新启动了。上次有 {count} 个 session 在跑，是否现在恢复它们？（先不恢复也行：在群里 @ 其中某一个，就只恢复它。）

## notice.restart.item
- {name}

## notice.ask.crew_removed
机组已被移除

## sender.human
人类

## sender.system
crewd

## sender.session
（{ext}号）{name}

## sender.session.unnumbered
（成员）{name}

## sender.captain
（机长）{name}

## inject.header
以下是你所在的群里别人发的全部新消息（不只是 @ 你的；每行标出 @ 了谁）：

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

## line.at
（@{targets}）

## line.at.you
你

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
到点后的动作照常经过你自己的权限确认（claude / codex 的权限框）；被拦下就先干别的。

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
