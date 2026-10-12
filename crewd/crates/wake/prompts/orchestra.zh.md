<!--
crewd-wake 的群通知与唤醒附言（spec §22.4、§22.7）。每节一段；占位符见 src/text.rs 的 SECTIONS。
这里的话是写给 agent 或人看的，不写进代码。
-->

## restart.interrupted
crewd 重启打断了你上一轮的工作。先核对现状（git status、你在做的那件事做到哪了、群里有没有新消息），再决定怎么继续；别照着记忆里的半截步骤直接往下做。

## resume.refused.group
「{name}」的原对话接不回来了——claude 拒了这个会话号，原话：`{said}`。这一轮是新开的：群里的上下文它还在，但终端里聊过的细节要重讲。

## resume.refused.diag.found
顺带：这个会话号的日志在盘上是有的（{path}），所以不是「日志没了」，得另查（claude 版本、文件权限、文件本身坏了）。

## resume.refused.diag.missing
顺带：这个会话号的日志在 claude 的 projects 目录下一个都找不到。

## resume.refused.agent
【注意】原会话（{id}）没能续上——claude 自己的原话是「{said}」，所以这是新开的一轮。群里的消息会照常注入，别假装记得终端里聊过的细节。

## resume.failed.group
「{name}」起不来了：{detail}。它名下的事需要有人接手，或者请人看一下。

## quota.wake
额度应该已经重置。你上次撞到额度上限被迫停下——醒来第一件事是核现状：get_quota 看额度、git log 看 main 到哪了、看自己的分支还在不在。积压里 @ 你的消息可能在你停下时已经被处理掉了，先核实再决定哪几条还要回。若仍受限，用 schedule_wakeup 自己顺延。

## loop.tripped.human
疑似唤醒环路：「{origin}」在 {minutes} 分钟内叫醒「{target}」{count} 次，期间「{target}」没有新产出（没调工具、没动工作项、回话只是回给叫它的人）。已只停这一条：之后「{origin}」不会再自动叫醒「{target}」，人的消息照常送达。确认没问题的话，在客户端对这条熔断点「恢复」。

## loop.chain.tripped.human

已暂停 {origin} → {target} 的这条消息因果链：第 {hop} 跳超过当前工程上限 {limit}。链标识：{chain}。其他独立链仍可继续；只有人可以针对这条链和这一路径恢复。

## loop.chain.unresolved.human

已暂停 {origin} → {target} 的这条消息因果链：尚不能确认它所依赖的投递已成功。链标识：{chain}。其他独立链仍可继续；只有人可以针对这条链和这一路径恢复。

## origin.system
系统

## captain.changed.new
你现在是这个机组的机长。机长专用的工具已对你开放；先看一眼大厅和人类群的未读，再决定下一步。

## captain.changed.old
你不再是这个机组的机长（新机长：{name}）。手上的事继续做完，派活与对人的汇报交给新机长。

## decision.session
（session: {session}）

## hibernate.resume
你上一次是被「休眠」暂停的：人要重启电脑（或让 crewd 整体歇一下），crewd 先让各个 session 停在安全点再退出。你的对话原样接回来了；这期间群里的消息照常会注入。

## hibernate.doing
停下之前你在处理的唤醒：{doing}

## hibernate.doing.unknown
停下之前你在干活，但 crewd 没记下是哪一批唤醒——翻一下自己对话的最后几轮。

## hibernate.finished
你在休眠期限之内做完了那一轮才停下，没有被打断。先核对一下现状再继续。

## hibernate.interrupted
休眠期限到了你还没做完那一轮，crewd 按 Esc 打断了你。被打断时在跑的命令（构建、测试、安装、迁移……）没跑完，它的输出和结果都不可信：先核对现状（git status、做到哪一步了），把被打断的命令重新跑一遍，再往下做。

## decision.first
「{name}」正在等待人类决策：

## decision.howto
打开这个会话选择，或在待决策列表里选择。

## notice.recovered
「{name}」已恢复。

## notice.give_up.crew
本机组唤醒投递已放弃：{sessions}。最后错误：{error}。

## notice.quota.captain
机长「{name}」被 {executor} 额度挡住，已安排恢复唤醒。

## notice.binary_replaced
crewd 二进制已替换；请重启 crewd 后再启动新会话。

## notice.input_draft
人类输入草稿已挡住「{name}」的投递十分钟；请清除或提交草稿以恢复投递。

## session.input_draft_unknown
「{name}」的投递已等待十分钟：检测到人类输入，但无法判断输入框是草稿还是占位提示。请查看终端；投递仍暂停。

## brief.undelivered
「{name}」的开场任务未送达，请重新派发。

## notice.runner
{name} 的执行器通知：{detail}
