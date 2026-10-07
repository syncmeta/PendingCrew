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

## quota.scheduled
「{name}」撞到额度上限，已挂额度重置唤醒（约 {minutes} 分钟后）。

## quota.fallback
「{name}」撞到额度上限，拿不到可信的重置时刻，已按退避挂唤醒（约 {minutes} 分钟后）。

## quota.wake
额度应该已经重置。你上次撞到额度上限被迫停下——醒来第一件事是核现状：get_quota 看额度、git log 看 main 到哪了、看自己的分支还在不在。积压里 @ 你的消息可能在你停下时已经被处理掉了，先核实再决定哪几条还要回。若仍受限，用 schedule_wakeup 自己顺延。

## delivery.failed.group
给「{name}」的唤醒连续 {attempts} 次没送进去（最后一次：{error}），已放弃。相关消息仍在群里，它下次醒来会看到；急的话请人看一下这个 session。

## loop.tripped.human
疑似唤醒环路：「{origin}」在 {minutes} 分钟内叫醒「{target}」{count} 次，期间「{target}」没有新产出（没调工具、没动工作项、回话只是回给叫它的人）。已只停这一条：之后「{origin}」不会再自动叫醒「{target}」，人的消息照常送达。确认没问题的话，在客户端对这条熔断点「恢复」。

## loop.tripped.owner
「{origin}」→「{target}」的自动唤醒疑似打转，已被熔断（只停这一条，详情在人类群）。需要它们继续协作的话，请人在客户端恢复。

## origin.system
系统

## captain.changed.new
你现在是这个机组的机长。机长专用的工具已对你开放；先看一眼大厅和人类群的未读，再决定下一步。

## captain.changed.old
你不再是这个机组的机长（新机长：{name}）。手上的事继续做完，派活与对人的汇报交给新机长。

## decision.first.worker
{name} 卡住了，在等人拍板：

## decision.first.captain
{name}（机长）卡住了，在等一个回复：

## decision.escalate
{name} 仍卡着，已等 {minutes} 分钟没人处理：

## decision.options.numbered
怎么答：这一屏的选项带编号，nudge_session 发对应数字即可。

## decision.options.unnumbered
怎么答：这一屏的选项没有编号，nudge_session 在这种框上会拒收数字。它认这几个裸按键：{keys}（每个键单独按一下，不额外补回车；esc 也不是「取消这个框」，它会让 claude 直接退出）；其余输入会被当成一条消息发进会话（打进输入框再提交）——提交那一下同样落在当前高亮的那一项上，未必是你想选的。高亮当下就停在你要的那一项上 → 发 `enter`；{move}

## decision.move.none
不在那一项上 → 交给人。

## decision.move.keys
不在 → 先发 {keys} 把它移到位，再发 `enter`。

## decision.tail.worker
机长可 inspect_session 看现场；按法照上面那一行（它是按当时那一屏算出来的）。拍不了板就 @人。

## decision.tail.captain
需要人打开这个 session 的终端选一下。

## decision.tail.escalate
需要人来定：打开这个 session 的终端直接选，或让机长 nudge_session 代按。

## decision.session
（session: {session}）

## hibernate.resume
你上一次是被「休眠」暂停的：人要重启电脑（或让 crewd 整体歇一下），crewd 先让各个 session 停在安全点再退出。你的对话原样接回来了；这期间群里的消息照常会注入。

## hibernate.doing
停下之前你在处理的唤醒：{doing}

## hibernate.doing.unknown
停下之前你在干活，但 crewd 没记下是哪一批唤醒——翻一下自己对话的最后几轮。

## hibernate.idle
停下时你是空闲的（上一轮已经做完）。醒来先看群里的未读，再决定要不要动手。

## hibernate.finished
你在休眠期限之内做完了那一轮才停下，没有被打断。先核对一下现状再继续。

## hibernate.interrupted
休眠期限到了你还没做完那一轮，crewd 按 Esc 打断了你。被打断时在跑的命令（构建、测试、安装、迁移……）没跑完，它的输出和结果都不可信：先核对现状（git status、做到哪一步了），把被打断的命令重新跑一遍，再往下做。
