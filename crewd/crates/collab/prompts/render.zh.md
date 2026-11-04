给人和 agent 看的群内公告与注入文字（crewd-collab 的 render / asks / restart 读这里）。
每节 `## 名字`；`{占位符}` 由代码填，允许哪些见 src/prompts.rs 的 REQUIRED。

## notice.restart
crewd 重新启动了。上次有 {count} 个 session 在跑，是否现在恢复它们？（先不恢复也行：在群里 @ 其中某一个，就只恢复它。）

## notice.restart.hibernate
crewd 从休眠里起来了。上次休眠时停下了 {count} 个 session，是否现在恢复它们？（先不恢复也行：在群里 @ 其中某一个，就只恢复它。被期限打断的那几个，恢复后会被提醒重跑被打断的命令。）

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
以下是你所在的群里别人发的新消息（不只是 @ 你的；每行标出 @ 了谁；进度、收到这类消息可能折叠成一行摘要）：

## inject.channel.lobby
【{crew} · 大厅】

## inject.channel.human
【{crew} · 人类群】

## inject.channel.topic
【群：{name}】

## inject.channel.dm
【私聊】

## line
#{seq} [{at}] {sender}：{text}

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

## wake.message.truncated
（这条太长，只带了开头。要看全文，用读群工具读 #{seq}。）

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

## wake.background_progress
背景进度更新，全文见 #{seq}。

## wake.background_ack
背景确认消息，全文见 #{seq}。

## notice.admin.handoff
行政事项请求你明确承接。request_id={request}；原范围工作：{targets}（正文最多50项，完整范围请用 admin_case_handoff_get）。原因：{reason}。通知送达不代表承接或工作完成；查看后使用 admin_case_handoff_accept 和当前 revision 明确承接。

## notice.admin.custody
用户提议你接管一项尚未解决的行政责任。case_id={case} crew_id={crew} request_id={request}；原工作范围（最多显示50项）：{targets}。原因：{reason}。用 admin_case_custody_get 查看完整事实及 pending.revision，再用 admin_case_custody_accept 明确接受。送达不等于承接，承接不等于业务完成；原工作owner不变。

## notice.work.coordination
请核实并协调 {work} 的进展，协调请求 ID 为 {decision}。请先读取当前工作和报告。若核实后决定换人，请在 work_item_assign 中附上 coordination_decision_id 和当前修订，以记录该请求的实际调整。模型建议不代表任务卡死或已经完成协调。

## notice.work.reassignment
{work} 的原负责人已核实失联，调度已将同一项工作重派给 {owner}。请查看当前负责人和进展；重派与通知不代表工作完成。

## notice.admin.title
行政事项

## notice.patrol.unreadable
巡检状态不可读取

## notice.patrol.retry
巡检读取失败；原进度保留，稍后重试

## notice.patrol.manual_interrupted
手动巡检已中断。原进度保留，可重新开始。

## notice.patrol.error_unreadable
巡检错误记录不可读取；原记录已保留

## notice.patrol.receipt_unreadable
机长简报回执不可读取

## notice.ask.still_wanted
待人类确认 {public} 已等待至少 72 小时。这件事还需要继续吗？请在待人类确认清单中处理原问题。

## notice.patrol.category.unowned
无人负责的工作

## notice.patrol.category.lost
负责人失联的工作

## notice.patrol.category.stop_escalated
停止检查已升级

## notice.patrol.category.long_waits
等待人类决定的工作

## notice.patrol.category.unclaimed
尚未认领的需求

## notice.patrol.category.needs_ruling
需要裁定的需求

## notice.patrol.category.idle_archive_candidates
可复查归档条件的群

## notice.patrol.header
本机组行政巡检简报（观察结束于 {time}）。以下为历史观察，处理前请核对当前事实。
