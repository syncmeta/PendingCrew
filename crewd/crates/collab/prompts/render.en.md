Group notices and injected text shown to people and agents (read by crewd-collab render / asks / restart).
One `## name` per section; `{placeholders}` are filled by code; see REQUIRED in src/prompts.rs.

## notice.restart
crewd restarted. {count} sessions were running before; resume them now? (Or not yet: @-mention one of them in a channel to resume just that one.)

## notice.restart.hibernate
crewd is back from hibernation. {count} session(s) were stopped by the hibernate — resume them now? (You can also leave them for now: @-mention one of them in a channel to resume only that one. Those cut off by the deadline will be told to re-run the interrupted command.)

## notice.restart.item
- {name}

## notice.ask.crew_removed
The crew was removed

## sender.human
Human

## sender.system
crewd

## sender.session
(ext {ext}) {name}

## sender.session.unnumbered
(member) {name}

## sender.captain
(captain) {name}

## inject.header
New messages from others in your channels (not only those that @-mention you; each line marks who was @-mentioned; progress and ack messages may be folded into one summary line):

## inject.channel.lobby
[{crew} · lobby]

## inject.channel.human
[{crew} · human channel]

## inject.channel.topic
[channel: {name}]

## inject.channel.dm
[direct message]

## line
#{seq} [{at}] {sender}: {text}

## line.at
(@{targets})

## line.at.you
you

## line.reply
(reply to {reply})

## wake.header
You were woken up for the following.

## wake.message
Messages that need your attention:

## wake.message.truncated
(This message is too long; only its beginning is shown. Read #{seq} in full with the read tool.)

## wake.timer
A reminder you scheduled is due.

## wake.ask_answer
The human answered your question {ask}.
Question: {question}
Answer: {answer}

## wake.ask_defaulted
Nobody answered your question {ask} in time; going with the default: {answer}
Question: {question}
Actions after the deadline still go through your own permission prompts (claude / codex); if one is blocked, move on to other work.

## wake.resume_note
Your note on how to continue: {note}

## wake.restart
A crewd restart interrupted your last turn. Check the current state (working tree, new channel messages) before continuing.

## wake.system
System notice.

## wake.checkpoint
A checkpoint is due.

## wake.stop_check
Before stopping, make sure every work item you own has a next step.

## wake.note
Note: {note}

## wake.background_progress
Background progress update; full text at #{seq}.

## wake.background_ack
Background acknowledgement; full text at #{seq}.

## notice.admin.handoff
行政事项请求你明确承接。request_id={request}；原范围工作：{targets}（正文最多50项，完整范围请用 admin_case_handoff_get）。原因：{reason}。通知送达不代表承接或工作完成；查看后使用 admin_case_handoff_accept 和当前 revision 明确承接。

## notice.admin.custody
用户提议你接管一项尚未解决的行政责任。case_id={case} crew_id={crew} request_id={request}；原工作范围（最多显示50项）：{targets}。原因：{reason}。用 admin_case_custody_get 查看完整事实及 pending.revision，再用 admin_case_custody_accept 明确接受。送达不等于承接，承接不等于业务完成；原工作owner不变。

## notice.work.coordination
请核实并协调 {work} 的进展；模型建议需要协调，不代表任务卡死或已经完成协调。请读取当前工作和报告，再决定下一步。

## notice.work.reassignment
{work} 的原负责人已核实失联，调度已将同一项工作重派给 {owner}。请查看当前负责人和进展；重派与通知不代表工作完成。

## notice.admin.title
行政事项

## notice.patrol.unreadable
巡检状态不可读取

## notice.patrol.retry
巡检读取失败；原进度保留，稍后重试

## notice.patrol.manual_interrupted
The manual patrol was interrupted. Its progress is retained, and a new patrol can be started.

## notice.patrol.error_unreadable
巡检错误记录不可读取；原记录已保留

## notice.patrol.receipt_unreadable
机长简报回执不可读取

## notice.ask.still_wanted
{public} has been waiting for a Human decision for at least 72 hours. Is this work still wanted? Please review the original question in your inbox.

## notice.patrol.category.unowned
Unowned work

## notice.patrol.category.lost
Work with a lost owner

## notice.patrol.category.stop_escalated
Escalated stop checks

## notice.patrol.category.long_waits
Work awaiting a Human decision

## notice.patrol.category.unclaimed
Unclaimed requirements

## notice.patrol.category.needs_ruling
Requirements awaiting a ruling

## notice.patrol.category.idle_archive_candidates
Channels to review for archival

## notice.patrol.header
Administrative patrol brief for this crew (observed through {time}). Check current facts before taking action.
