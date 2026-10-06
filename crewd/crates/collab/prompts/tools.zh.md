<!--
给 agent 看的工具文字（spec §22.3）。第一个节标题之前是注释（注释里任何一行都不许以「## 」开头，解析器会当成节）。
节名：<工具> = 工具描述；<工具>.<参数> = 参数说明；r.* = 回执；trace.* = 停在问句上时的代发稿；inject.* = 注入块的补充。
占位符 {名字} 由代码填（M3a 的 prompts::fill）。zh 与 en 的节名集合必须一致（测试钉住）。改工具行为时同一拍改这里。
-->

## post
把关键节点发到群里：开始、完成、卡住、交接、重要发现。终端里的过程（读了什么、跑了什么）别往群里倒。
不填 channel：给了 reply_to 就发到那条所在的群，否则发本机组大厅。
@ 的规则：@session / @captain 收窄可见范围——只有被点到的 agent 在注入里看得见、并被叫醒；同时加上 @broadcast 就是「群里所有成员看得见、只叫醒被点到的」；@human 不收窄，只是标给人看。人类没有注视的群里 @人类，标记会亮，但人可能很久才看到——急事发根机组人类群，或用 ask。
单条正文最多 256 KiB。

## post.channel
群 id，或你所在的群里唯一的群名。不填见工具说明。

## post.message
发一条时用它。与 messages 二选一。

## post.messages
一次发几条（最多 6 条），每条一个气泡、各带自己的 category。任何一条不合法就整批不发，错误会指名第几条。

## post.mentions
定向 @：{kind:"session", target_id:"<session id>"} / {kind:"captain"} / {kind:"human"} / {kind:"broadcast"}。不填 = 发给群里所有人看。被 @ 的 session 必须是这个群的成员；@captain 也要求机长是这个群的成员，机组没有机长时 @captain 会报错。

## post.reply_to
你在回复哪一条：消息序号（如 #128 或 128）或消息 id。只能回复同一个群里的消息。会自动 @ 原发送者，形状是 [broadcast, 原发送者]：群里所有成员照样看得见，只是把他叫醒；回复自己的消息不会加 @；回复系统消息只加 broadcast。

## post.category
这条是什么：progress 推进 / done 完成 / blocked 卡住 / question 要问 / finding 发现 / handoff 交接 / ack 收到 / note 其它。


## read_channel
分页读一个群的消息（旧在前）。读到最新一条时，这个群对你就算已读，之后不会再注入这些消息——只对你在其中、或你收听的群有效（读别的群不改已读位置）。每条前面的 #序号 可以填进 reply_to。

## read_channel.channel
群 id 或群名；不填 = 本机组大厅。本机组的群都能读，不必是成员。

## read_channel.limit
条数，1–200，默认 50。

## read_channel.before
只看这一条之前的（消息序号或 id）。不填 = 最新一页。

## search
在你所在的群和你收听的群里搜消息。整个查询词（去掉首尾空白）当作一个整体，做不区分大小写的子串匹配（只折叠 ASCII 字母的大小写），不是按词拆开再求交；按时间倒序。

## search.query
要搜的文字：去掉首尾空白后整体当一个子串匹配（ASCII 不区分大小写）。

## search.channel
只搜这一个群（群 id 或群名）。

## search.after
只要这个时刻之后的（ISO8601，含边界）。

## search.before
只要严格早于这个时刻的（ISO8601，不含边界：created_at < before）。

## search.limit
条数上限，1–200，默认 50。

## list_channels
列出你所在的群（大厅、你加入的群、你收听的群）和本机组的其它群：id、名字、用途、群主、成员、人类是否在注视。

## human_watching
人类现在在哪儿：他标了「我在注视」的群，加上根机组的人类群。要人拍板、要人看见，发到这些群；不在其中的群里 @人类，他可能很久才看到。

## channel_create
建一个群（话题群或两人私聊）。要和谁就某件事协作、要单独讨论一个话题，就用它——不要为此开子机组。你是群主，自动在群里。群有名字和用途，例如「发版」「scroll bug 排查」。群名不能与人类（人类 / Human）或 crewd 的系统标签重名，重名会被拒绝。

## channel_create.name
群名，短。

## channel_create.purpose
这个群是干什么的，一句话。

## channel_create.members
要拉进来的成员：session id、通讯录号码（如 7-3），或 human（人类）。

## channel_create.kind
topic（话题群，默认）或 dm（恰好两个成员的私聊，其中至多一个是人类；同一对成员已有私聊就直接复用，不新建）。

## channel_invite
把人拉进一个群。你自己必须是这个群的成员；大厅和私聊（dm）不接受邀请。

## channel_invite.channel
群 id 或群名。

## channel_invite.members
session id、通讯录号码（如 7-3），或 human。

## channel_leave
退出一个群。大厅退不了；私聊（dm）退不了，不要的话归档它；话题群的群主要先 channel_transfer 转给别人才能退。

## channel_leave.channel
群 id 或群名。

## channel_archive
归档一个群（消息保留，不能再发言；可以由人解档）。只有群主或机长能归档；大厅和人类群不能归档。

## channel_archive.channel
群 id 或群名。

## channel_transfer
把群主转给别人。群主发起后要对方用 channel_accept 接受才生效；机长可以 force 直接转。群主相当于这个群的机长：人类在群里说话没点名时，调度判断不了就叫群主。

## channel_transfer.channel
群 id 或群名。

## channel_transfer.to
新群主：session id 或通讯录号码。必须是群成员。

## channel_transfer.force
机长专用：true = 不等对方接受，直接转。

## channel_accept
接受别人转给你的群主。

## channel_accept.channel
群 id 或群名。

## channel_listen
收听一个群：这个群有新消息时也叫醒你（不用被 @）。在等某个群的动静时用；收听期间正常结束回合等着，别轮询 read_channel。

## channel_listen.channel
群 id 或群名。

## channel_listen.minutes
听多久（分钟，1–480）。不填 = 一直听到 channel_unlisten。

## channel_listen.senders
只听这些人：human、captain，或 session id。这只决定谁的消息会叫醒你；群里看得见的消息照样都会注入给你。不填 = 所有人。

## channel_unlisten
停止收听一个群。

## channel_unlisten.channel
群 id 或群名。

## directory
查通讯录：本机每个机组、每个 session 都有短号码——机组是整数（7），成员是分机（7-3），-1 恒定是那个机组的机长。每条返回：号码、类别（机组或成员）、名字、机组 / 组织路径、session id；另外附上你所在的群。人类不编号，找人用 ask。

## directory.query
按号码前缀、名字或关键词过滤；不填 = 全表。

## contact
按号码联系别的机组或 session。to 填机组号（如 7）= 发到它的大厅并 @它的机长，消息带上你的来源；填分机号（如 7-3）= 与那个 session 私聊（没有就建）。你自己的大厅会留一行「已联系 …」。本机组的事直接 post。

## contact.to
目标号码：7（机组）或 7-3（session）。

## contact.message
要说的话：写清你是谁、要什么。

## ask
向人类提一个要他拍板的问题。不阻塞：问题记成一条待确认（H 号），发进根机组人类群并 @人类；你立刻拿到回执，去干别的或收工。人回答时系统直接叫醒你，并把 resume_note 原样念回给你。

## ask.question
要人拍板的那件事。把选项和你的倾向写出来——「A / B，我倾向 A，因为 …」。

## ask.resume_note
答复回来后你要接着做什么（做到哪一步、下一步是什么）。半路上问的问题一定要写。

## ask.fallback_after_minutes
可选：等这么多分钟没人答，就叫醒你按 fallback 办。只用于可撤回的事；到点之后若要动生产、花钱、隐私或凭据，照样会被审批拦住。

## ask.fallback
到点没人答时你打算怎么办（「就按 A 做」）。和 fallback_after_minutes 一起给。

## withdraw_ask
撤回一条还没人答的问（那件事已经不成立了）。你自己提的能撤；机长能撤本机组的任何一条。还卡着工作项的问撤不了。

## withdraw_ask.ask
H 号，如 H3。

## withdraw_ask.reason
为什么不成立了，一句话说清变了什么。

## respond_todo
回应 Agent 那本 Todo（人类派给你们的活）的某一条：认领、进展、结果。追加式，不改状态。

## respond_todo.number
条目编号 #N 里的 N。

## respond_todo.response
回应内容，一两句。

## get_quota
查本机 claude / codex 的订阅额度：各窗口已用百分比与重置时刻。开长活前看一眼。没有剩余 token 的绝对量，别编。

## schedule_wakeup
定时叫醒自己：到点把 note 送回给你。额度将尽时的标准动作：收尾到可交接 → 约在重置后几分钟 → 停手。醒来时只有 note 和群消息可依靠，note 要自足。

## schedule_wakeup.after_minutes
多少分钟后（1–1440）。与 at 二选一。

## schedule_wakeup.at
ISO8601 时刻，必须在未来。与 after_minutes 二选一。

## schedule_wakeup.note
醒来该干什么、上下文在哪。

## set_session_profile
切你自己的模型或推理强度。不是当场生效：这里改的是这个 session 的设置，下一次启动（重启或续跑）时才生效，当前这一回合不变。最贵的档位只在普通档位确实不行时才用。

## set_session_profile.model
模型别名，如 opus / sonnet。

## set_session_profile.effort
推理强度档位。

## rename_crew
（机长）给机组起个短标签名，例如「鉴权重构」。搞清楚机组在做什么再起，起一次就够。

## rename_crew.name
几个字的短标签，不带标点和前后缀。

## change_workdir
（机长）改机组的工作目录。先不带 confirm 看预览，再 confirm: true 执行。只能改自己和自己名下的子机组。

## change_workdir.new_path
新目录的绝对路径，必须已存在。

## change_workdir.crew
改哪个机组（名字、id 或号码）；不填 = 本机组。

## change_workdir.include_children
连子机组一起改，默认 true。

## change_workdir.confirm
true = 真执行；不填 = 只看预览。

## set_crew_status
（机长）一句话说清整个机组现在怎么样，显示在侧栏这个机组那一行。超过 200 字不收。

## set_crew_status.text
一句话状态。

## read_delegation_policy
（机长）读本机组的派活原则。

## update_delegation_policy
（机长）改本机组的派活原则：提交完整的新正文。

## update_delegation_policy.text
完整的新正文。

## start_session
（机长）在本机组起一个 worker 去干一件明确的编码任务。

## start_session.brief
要它干的事，写清目标与完成标准。

## start_session.title
≤18 字、不带项目名的任务概括，作群里的显示名。

## start_session.runner
claude 或 codex；不填 = 机组默认。

## start_session.isolation
必填。false = 用机组共享目录；true = 为它新建独立 git worktree。按并行冲突、改动范围、任务之间的关系逐次判断；创建失败会直接报错，不会偷偷退回共享目录。

## start_session.model
可选模型。

## start_session.effort
可选推理强度。

## stop_session
（机长）停掉本机组某个 session 的进程。先在大厅写明原因再停；不可撤销。只想打断一轮用 nudge_session 发 Esc。

## stop_session.session_id
目标 session id。

## stop_session.reason
停的原因，会公开写进大厅。

## inspect_session
（机长）看某个 session 的现场：状态 + 终端最近的输出（codex 是记录尾部）。@ 不应时先看它卡在哪。

## inspect_session.session_id
目标 session id。

## nudge_session
（机长）给某个 session 的终端发按键或文本替它解围：Enter / Esc 发对应按键，其它文本当一条输入提交。先 inspect_session 看清再按。

## nudge_session.session_id
目标 session id。

## nudge_session.input
Enter、Esc，或要发的文本。

## list_sessions
（机长）列出本机组的 session：id、号码、显示名、角色、状态。

## handoff_captain_to_session
（机长）把机长位置交给一个现有 session。target_crew_id 不填 = 本机组；填了只能是你的直系子机组。

## handoff_captain_to_session.session_id
接任者的 session id，必须属于目标机组。

## handoff_captain_to_session.target_crew_id
可选：直系子机组的 id。

## create_and_handoff_captain
（机长）新起一个 session 并把机长位置交给它。target_crew_id 不填 = 本机组；填了只能是你的直系子机组。

## create_and_handoff_captain.runner
claude 或 codex，必须明确选。

## create_and_handoff_captain.model
可选模型。

## create_and_handoff_captain.effort
可选推理强度。

## create_and_handoff_captain.title
可选显示名；不填 = 机长。

## create_and_handoff_captain.opening_brief
可选：新机长的第一件事。

## create_and_handoff_captain.target_crew_id
可选：直系子机组的 id。

## report_to_parent
（机长）向上级汇报：发到每个直系父机组的大厅并 @父机长。阶段性结果、要上级拍板或协调资源、被卡住时用。结论先行。

## report_to_parent.message
汇报内容。

## message_child_crew
（机长）给一个直系子机组下达消息：发到它的大厅并 @子机长。

## message_child_crew.crew
子机组的名字、id 或号码。

## message_child_crew.message
要传达的内容。

## adopt_crew
（机长）收编：把一个与本机组没有上下级关系的机组挂到本机组名下。不能成环；内建总机组不能被收编。

## adopt_crew.crew
目标机组的名字、id 或号码。

## release_crew
（机长）调整自己直系子机组的挂靠：不填 to = 摘出到顶层；填 to = 转挂到你的另一个直系子机组名下。

## release_crew.crew
直系子机组的名字、id 或号码。

## release_crew.to
可选：转挂目的地（你的另一个直系子机组）。

## create_parent_crew
（机长）在本机组头上新建一个父机组：本机组成为它的子机组，父机组继承工作目录并起父机长。只在组织结构真要多一层（把几个部门归到一个上级下）时用。

## create_parent_crew.title
可选：父机组的短标签名。

## adopt_parent
（机长）认父：把一个现有机组认作本机组的父机组。不能成环。

## adopt_parent.crew
目标机组的名字、id 或号码。

## create_child_crew
（机长）以本机组为父建子机组：继承工作目录，自动起子机长，brief 是子机长的第一件事。子机组只表达组织结构——一个长期存在、有自己机长的部门或团队。围绕某个话题或某件事协作，用 channel_create 建群，不要开子机组。

## create_child_crew.brief
子机组要干的事。

## create_child_crew.title
可选：短标签名。

## create_child_crew.runner
可选：子机长用 claude 还是 codex。

## r.unknown_tool
没有 {name} 这个工具。

## r.captain_only
{name} 只有机长能用，你现在不是机长。

## r.not_on_builtin
{name} 在总机组里不能用。

## r.bad_args
参数不对：{detail}

## trace.worker
{label} 这一轮停在一个问题上，在等回复：
{question}
机长可以 inspect_session 看现场、nudge_session 代答；拍不了板就问人。（session：{session}）

## trace.captain
{crew} 的 {label} 这一轮停在一个问题上，在等你回复：
{question}
需要你打开这个 session 的终端回它一句。（session：{session}）

## trace.crew
{title}（{number}号机组）

## r.failed
没成：{detail}

## r.no_handler
{name} 还没有实现。

## r.internal
内部错误 #{n}（详情见 crewd 日志）

## inject.omitted
{channel} 还有 {count} 条较新的消息这次没带上（字数上限），它们仍是未读：下次工具调用后会接着给你，也可以先用 read_channel 看。

## inject.omitted_at_least
{channel} 还有至少 {count} 条较新的消息这次没带上（字数上限；未读很多，实际可能更多），它们仍是未读：下次工具调用后会接着给你，也可以先用 read_channel 看。

## inject.truncated
{channel} 里 #{seq} 这条太长，只带了开头；要看全文：read_channel(channel="{id}", before={next}, limit=1)。

## r.lobby_label
「{crew}」大厅

## r.human_label
「{crew}」人类群

## r.dm_gone
已解散的私聊

## r.dm_generic
私聊

## r.approval_granted
人类已同意 {tool}，这一次放行（只此一次）。

## r.approval_filed
{tool} 需要人类同意，已请人审批，这次先拒绝。先去干不需要它的活；人同意后会叫你回来，那时重跑这一次调用就会放行。

## r.approval_pending
{tool} 还在等人类审批（已经请过了，不会再请一次）。先去干别的；人同意后会叫你回来。

## r.approval_unavailable
现在没法发起审批，已拒绝 {tool}。
