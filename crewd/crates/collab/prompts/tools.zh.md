<!--
给 agent 看的工具文字（spec §22.3）。第一个节标题之前是注释（注释里任何一行都不许以「## 」开头，解析器会当成节）。
节名：<工具> = 工具描述；<工具>.<参数> = 参数说明；r.* = 回执；trace.* = 停在问句上时的代发稿；inject.* = 注入块的补充。
占位符 {名字} 由代码填（M3a 的 prompts::fill）。zh 与 en 的节名集合必须一致（测试钉住）。改工具行为时同一拍改这里。
-->

## post
把关键节点发到群里：开始、完成、卡住、交接、重要发现。终端里的过程（读了什么、跑了什么）别往群里倒。
发到哪必须说清，没有默认：channel = 发到一个群；to = 按通讯录号码私聊一个人（跨机组也用它）；只给 reply_to = 发到那条消息所在的群。三者都不给会报错并列出你的群。
@ 的规则：@ 只决定叫醒谁——不 @ 谁就不叫醒任何人（私聊例外：对方总会被叫醒）；@session / @captain 叫醒被点到的 agent，@human 是标给人看；@ 不收窄可见范围，读得到这个群的人照样都看得见。人类没有注视的群里 @人类，标记会亮，但人可能很久才看到——急事发你的 home（顶层机组）的人类群，或用 ask。
发得太快、同一句话重复发、频繁 @人类 会被挡下（回执说明怎么办）：把进度并成一条再发。
回执会写明这条会叫醒谁。单条正文最多 256 KiB。

## post.channel
群 id，或你读得到的群里唯一的群名。与 to 二选一。

## post.to
私聊一个人：通讯录号码——7-3（7 号机组的 3 号分机）、7（7 号机组此刻的机长）、0（总机组的机长）；directory 查号。同一对人只有一个私聊，已有就接着用。人类没有私聊（to 填 human 会指给你 home 人类群）：找人发 home 人类群，要拍板用 ask。与 channel、reply_to 都不能同时给。

## post.message
发一条时用它。与 messages 二选一。

## post.messages
一次发几条（最多 6 条），每条一个气泡、各带自己的 category。任何一条不合法就整批不发，错误会指名第几条。mentions、reply_to 只挂在第一条上。

## post.mentions
要叫醒谁：{kind:"session", target_id:"<session id 或通讯录号码>"} / {kind:"captain"} / {kind:"human"}。不填 = 不叫醒任何人，读得到这个群的人照样看得见。被 @ 的 session 必须是这个群的成员；@captain 也要求机长是这个群的成员，机组没有机长时 @captain 会报错。

## post.reply_to
你在回复哪一条：消息序号（如 #128 或 128）或消息 id。只能回复同一个群里的消息。会自动 @ 原发送者（他还在群里时）：群里所有成员照样看得见，只是把他叫醒；回复自己的消息、回复系统消息都不会加 @。

## post.category
这条是什么：progress 推进 / done 完成 / blocked 卡住 / question 要问 / finding 发现 / handoff 交接 / ack 收到 / note 其它。


## read_channel
分页读一个群的消息（旧在前）。读到最新一条时，这个群对你就算已读，之后不会再注入这些消息——只对你在其中、或你收听的群有效（读别的群不改已读位置）。每条前面的 #序号 可以填进 reply_to。

## read_channel.channel
群 id 或群名；不填 = 本机组大厅。读得到的群：本机组名下的群（含本机组成员之间的私聊——没有私有群）、你的 home 人类群、你在其中或收听的群；总机组的 session 读得到全组织的群。

## read_channel.limit
条数，1–200，默认 50。

## read_channel.before
只看这一条之前的（消息序号或 id）。不填 = 最新一页。

## search
在你读得到的群里搜消息（本机组名下的群、你的 home 人类群、你在其中或收听的群，已归档的也算；总机组的 session 搜全组织）。整个查询词（去掉首尾空白）当作一个整体，做不区分大小写的子串匹配（只折叠 ASCII 字母的大小写），不是按词拆开再求交；按时间倒序。

## search.query
要搜的文字：去掉首尾空白后整体当一个子串匹配（ASCII 不区分大小写）。

## search.channel
只搜这一个群（群 id 或群名）。

## search.crew
只搜这个机组名下你读得到的群（机组号、id 或名字）。

## search.after
只要这个时刻之后的（ISO8601，含边界）。

## search.before
只要严格早于这个时刻的（ISO8601，不含边界：created_at < before）。

## search.limit
条数上限，1–200，默认 50。

## list_channels
列出群：第一行是你的 home 人类群（找人、要人拍板发这里），然后是你在其中或收听的群、本机组的其它群、人类在注视的群。每行：id · 名字 · 类型 · 用途 · 群主 · 人数 · 状态（你在群里 / 你在收听 / 人类在注视 / 已归档 / 你是群主）。

## list_channels.crew
只列这个机组名下你读得到的群（机组号、id 或名字）。

## list_channels.include_archived
true = 连已归档的群一起列；默认不列。

## channel_create
建一个话题群。要和谁就某件事协作、要单独讨论一个话题，就用它——不要为此开子机组。你是群主，自动在群里。群有名字和用途，例如「发版」「scroll bug 排查」。群名不能与人类（人类 / Human）或 crewd 的系统标签重名，重名会被拒绝。私聊不用建：post(to:) 直接发，还没有就自动开。

## channel_create.name
群名，短。

## channel_create.purpose
这个群是干什么的，一句话。

## channel_create.members
一起拉进来的成员（可不填，之后再用 channel_invite）：session id、通讯录号码（如 7-3；7 = 7 号机组此刻的机长），或 human（人类）。不用填你自己。

## channel_invite
把人拉进一个群。一般要你自己是这个群的成员；大厅和私聊不接受邀请。

## channel_invite.channel
群 id 或群名。

## channel_invite.members
session id、通讯录号码（如 7-3），或 human。

## channel_leave
退出一个群。大厅退不了；私聊退不了（不要的话归档它）；话题群的群主要先用 channel_owner 转给别人才能退。

## channel_leave.channel
群 id 或群名。

## channel_archive
归档一个群（消息保留，不能再发言；人可以解档）。只有群主、该群所属机组的机长或人能归档；大厅和人类群不能归档。

## channel_archive.channel
群 id 或群名。

## channel_owner
群主的转让。群主相当于这个群的机长：人类在群里说话没点名时，调度判断不了就叫群主。三种用法：给 to = 群主请对方接手（会以你的名义在群里 @ 对方，对方调 channel_owner(channel=…) 接受后才生效）；不给 to = 接受转给你的群主；给 to 且 force=true = 该群所属机组的机长（总机组的机长也算）不等对方接受，直接换群主。新群主必须是群成员。

## channel_owner.channel
群 id 或群名。

## channel_owner.to
新群主：session id 或通讯录号码（如 7-3）。不填 = 接受转给你的群主。

## channel_owner.force
机长用，要同时给 to：true = 不等对方接受，直接转。

## channel_listen
收听一个群：这个群有新消息时也叫醒你（不用被 @）。在等某个群的动静时用；收听期间正常结束回合等着，别轮询 read_channel。只能收听你读得到的群。不再需要时 channel_listen(channel=…, off=true)。

## channel_listen.channel
群 id 或群名。

## channel_listen.minutes
听多久（分钟，1–480）。不填 = 一直听，直到用 off=true 停掉。

## channel_listen.senders
只听这些人：human、captain（发言时是机长的那位，换了机长就跟着换）、session id 或通讯录号码（号码按此刻解析成那个 session，如 7-1 = 7 号机组此刻的机长，之后换机长不跟着换）。这只决定谁的消息会叫醒你；群里的消息照样都会注入给你。不填 = 所有人。

## channel_listen.off
true = 停止收听这个群（不能和 minutes、senders 一起给）。本来就没在收听也不报错。

## directory
查通讯录：本机每个机组、每个 session 都有短号码——机组是整数（7），成员是分机（7-3），-1 恒定是那个机组此刻的机长，0 是总机组（填 0 = 它此刻的机长）。每行：号码、名字、所在机组或上级路径、session id。号码可以直接填进 post 的 to（私聊，跨机组也用它）和各工具的成员参数。列群用 list_channels；人类不编号：找人发 home 人类群，要拍板用 ask。

## directory.query
按号码前缀、名字或组织路径过滤；不填 = 全表。

## ask
向人类提一个要他拍板的问题。不阻塞：问题记成一条待确认（H 号），发进你的 home（顶层机组；总机组是它自己）的人类群并 @人类；你立刻拿到回执，去干别的或收工。人回答时系统直接叫醒你，并把 resume_note 原样念回给你。

## ask.question
要人拍板的那件事。把选项和你的倾向写出来——「A / B，我倾向 A，因为 …」。

## ask.resume_note
答复回来后你要接着做什么（做到哪一步、下一步是什么）。半路上问的问题一定要写。

## ask.fallback_after_minutes
可选：等这么多分钟没人答，就叫醒你按 fallback 办。只用于可撤回的事。

## ask.fallback
到点没人答时你打算怎么办（「就按 A 做」）。和 fallback_after_minutes 一起给。

## withdraw_ask
撤回一条还没人答的问（那件事已经不成立了）。你自己提的能撤；机长能撤本机组的任何一条（总机组的机长在每个机组都算机长）。还卡着工作项的问撤不了。

## withdraw_ask.ask
H 号，如 H3。

## withdraw_ask.reason
为什么不成立了，一句话说清变了什么。

## withdraw_ask.crew
这条问记在哪个机组的账上（号码或 id）；不填 = 你自己的机组。

## respond_todo
回应你所在机组的 Agent Todo（人类派给机组的活）的某一条：认领、进展、结果。追加式，不改状态。

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
RFC 3339 时刻，必须带上你指的时区偏移（如北京时间 2026-10-08T09:00:00+08:00；Z 表示 UTC），必须在未来、7 天以内。与 after_minutes 二选一。

## schedule_wakeup.note
醒来该干什么、上下文在哪。

## set_session_profile
切你自己的模型或推理强度。不是当场生效：这里改的是这个 session 的设置，下一次启动（重启或续跑）时才生效，当前这一回合不变。最贵的档位只在普通档位确实不行时才用。

## set_session_profile.model
模型别名，如 opus / sonnet。

## set_session_profile.effort
推理强度档位。

## rename_crew
给机组起个短标签名，例如「鉴权重构」。搞清楚机组在做什么再起，起一次就够。crew 不填 = 本机组；别的机组只有你按机长算的才行。

## rename_crew.name
几个字的短标签，不带标点和前后缀。

## rename_crew.crew
可选：改哪个机组（号码或 id）；不填 = 本机组。

## change_workdir
改机组的工作目录。先不带 confirm 看预览，再 confirm: true 执行。能改的：你按机长算的机组（总机组的机长：任何机组），以及本机组下面的子机组。已移除的机组不动。

## change_workdir.new_path
新目录的绝对路径，必须已存在。

## change_workdir.crew
可选：改哪个机组（号码或 id）；不填 = 本机组。

## change_workdir.include_children
连子机组一起改，默认 true。

## change_workdir.confirm
true = 真执行；不填 = 只看预览。

## set_crew_status
一句话说清整个机组现在怎么样，显示在侧栏这个机组那一行。超过 200 字不收；给空串 = 清掉。crew 不填 = 本机组。

## set_crew_status.text
一句话状态。

## set_crew_status.crew
可选：哪个机组（号码或 id）；不填 = 本机组。

## delegation_policy
机组的派活原则：不给 text = 读；给 text = 用它整个替换。crew 不填 = 本机组。

## delegation_policy.text
可选：完整的新正文（替换旧的）；不给 = 只读。

## delegation_policy.crew
可选：哪个机组（号码或 id）；不填 = 本机组。

## start_session
（机长）起一个 worker 去干一件明确的编码任务。crew 不填 = 本机组；别的机组只有你按机长算的才行（总机组的机长在每个机组都按机长算）。

## start_session.brief
要它干的事，写清目标与完成标准。

## start_session.title
≤18 个字、不带项目名的任务概括，作群里的显示名；不填就取 brief 第一行的前 18 个字。

## start_session.runner
claude 或 codex；不填 = 机组默认。

## start_session.isolation
必填。false = 用机组共享目录；true = 为它新建独立 git worktree。按并行冲突、改动范围、任务之间的关系逐次判断；创建失败会直接报错，不会偷偷退回共享目录。

## start_session.model
可选模型。

## start_session.effort
可选推理强度。

## start_session.crew
可选：在哪个机组起（号码或 id）；不填 = 本机组。

## stop_session
（机长）停掉一个 session 的进程：原因先以你的名义写进它所在机组的大厅，再停；不可撤销。只想打断它这一轮，用 nudge_session 发 esc。不能停你自己。

## stop_session.session_id
目标 session：id 或通讯录号（如 7-3）。

## stop_session.reason
停的原因，会公开写进大厅。

## inspect_session
（机长）看某个 session 的现场：状态 + 终端最近的输出（codex 是记录尾部）。@ 不应时先看它卡在哪。

## inspect_session.session_id
目标 session：id 或通讯录号（如 7-3）。

## nudge_session
（机长）打断某个 session 当前这一轮：只收按键 esc / interrupt。要告诉它什么，用 post(to: …) 发给它，它被唤醒时会读到。发出后不保证生效：过几秒 inspect_session 看效果。

## nudge_session.session_id
目标 session：id 或通讯录号（如 7-3）。

## nudge_session.key
esc 或 interrupt，都是打断它当前这一轮。

## list_sessions
（机长）列出一个机组的 session：id、号、名字、角色、状态。crew 不填 = 本机组。

## list_sessions.crew
可选：机组号码或 id；不填 = 本机组。

## handoff_captain
（机长）交出一个机组的机长位置：to_session 交给一个现有 session（必须属于那个机组），或 runner 新起一个 session 接任，两者只给一个。crew 不填 = 本机组；也可以是你的直系子机组，或任何你按机长算的机组。原机长留在机组里当 worker。

## handoff_captain.crew
可选：哪个机组的机长位置（号码或 id）；不填 = 本机组。

## handoff_captain.to_session
接任者：一个现有 session 的 id 或通讯录号，必须属于那个机组。

## handoff_captain.runner
新起一个 session 接任：claude 或 codex。

## handoff_captain.model
可选：新机长的模型（只配 runner 用）。

## handoff_captain.effort
可选：新机长的推理强度（只配 runner 用）。

## handoff_captain.title
可选：新机长的显示名，≤18 个字；不填 = 机长（只配 runner 用）。

## handoff_captain.opening_brief
可选：新机长的第一件事（只配 runner 用）。

## adopt_crew
（机长）收编：把一个与本机组没有上下级关系的机组挂到本机组名下。不能成环；总机组（管理机组）在机组树外，不能被收编。

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

## r.tool_removed
没有 {name} 这个工具了：它的活改由 {replacement} 做。用它再调一次。

## r.captain_only
{name} 只有机长能用，你现在不是机长。

## r.bad_args
参数不对：{detail}

## trace.worker
{label} 这一轮停在一个问题上，在等回复：
{question}
机长可以 inspect_session 看现场；要回答它就 post(to: …) 发给它，它被唤醒时会读到；拍不了板就问人。（session：{session}）

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

## inject.digest
{channel}：另有 {count} 条进度 / 收到（{senders}，#{first}–#{last}）折叠了，没逐条列出；要看就用 read_channel。

## r.lobby_label
「{crew}」大厅

## r.human_label
「{crew}」人类群

## r.dm_gone
已解散的私聊

## r.dm_generic
私聊

## r.post_one_of
message 与 messages 二选一，且用 messages 时把 category 写在每一条里。

## r.post_count
messages 要 1–6 条，你给了 {count} 条。多的拆成两次发，或合并成一条。

## r.post_empty
第 {index} 条是空的。补上内容或删掉这一条再发。

## r.bad_category
第 {index} 条的 category「{value}」不认识，可选：{choices}。

## r.bad_mention
mention 写法不对（kind = {kind}）：kind 只能是 session / captain / human，session 必须带 target_id（session id 或通讯录号码）。

## r.no_message
找不到消息 {ref}（或它所在的群你读不到）。序号见 read_channel / search 每行开头的 #。

## r.reply_other_channel
#{seq} 不在你指定的群里；回复要发在原消息所在的群，去掉 channel 即可。

## r.post_partial
前面的已发出（{sent}），第 {index} / {total} 条没发出去：{detail}。别整批重发，只补没发的。

## r.post_no_target
没说发到哪：给 channel（群 id 或群名）、to（通讯录号码，如 7-3 / 7 / 0，发私聊）或 reply_to（回复哪一条）。你所在的群：
{choices}
其它群用 list_channels 查，号码用 directory 查。

## r.post_target_conflict
发到哪只能说一次：to 是私聊，不能和 channel 或 reply_to 一起给。去掉多余的那个再发。

## r.post_to_self
{address} 就是你自己。要找别人，先用 directory 查号码。

## r.posted
已发到{channel}：{seqs}。

## r.posted_dm
已私聊 {peer}（{address}）：{seqs}。{new}

## r.dm_opened
这是新开的私聊（{id}），之后接着在这里聊，或再用 to 都行。

## r.posted_wakes
会叫醒：{names}。

## r.posted_wakes_none
这条没 @ 任何人，不会叫醒谁；要谁回应就 @ 谁。

## r.posted_wakes_dm
私聊里对方总会被叫醒，不用 @。

## r.posted_not_member
你不是{channel}的成员：别人的回复叫不醒你；要听回复就 channel_listen 它。

## r.post_throttled
发得太快，这次没发出去：把要说的并成一条，{retry_secs} 秒后再发。

## r.post_duplicate
和你刚发的 #{seq} 一模一样，没再发。要补充就写新的内容，或 reply_to #{seq}。

## r.human_mention_throttled
最近 30 分钟里你 @人类 太多次了，这次没发出去。要人拍板用 ask；其它情况去掉 @human 再发。

## r.human_not_watching
人类现在没有注视{channel}：@人类 的标记已经亮了，但可能很久才被看到。急事或要拍板的事发你的 home 人类群 {home}（{home_id}，list_channels 第一行），或用 ask。

## r.channel_choice
- {id} {name}

## r.no_channel
找不到群「{arg}」（或你读不到它）。你能用的群（最多列 20 个，你在其中的在前）：
{choices}
全部的群用 list_channels 查。

## r.ambiguous_channel
有不止一个群叫「{arg}」，请用 id：
{choices}

## r.line
#{seq} {time} {who}：{text}

## r.line_attachment
 ［附件 {path}］

## r.read_empty
{channel}里还没有消息。

## r.read_header
{channel}（{id}）的 {count} 条消息，旧在前：

## r.read_more
更早的还有：before 填 {before} 继续往前翻。

## r.search_none
没搜到「{query}」。换个更短的词，或用 crew / channel 换个范围。

## r.search_hit
［{channel}］{line}

## r.list_header
群（id · 名字 · 类型 · 用途 · 群主 · 人数 · 状态）：

## r.list_line
- {id} · {name} · {kind} · {purpose} · 群主 {owner} · {count} 人 · {flags}

## r.list_more
还有 {count} 个群没列出；用 crew 只看某个机组的群。

## r.flag_home_human
你的 home 人类群（找人发这里）

## r.flag_member
你在群里

## r.flag_listening
你在收听

## r.flag_watched
人类在注视

## r.flag_archived
已归档

## r.flag_owner
你是群主

## r.owner_none
（无）

## r.kind_lobby
大厅

## r.kind_human
人类群

## r.kind_topic
话题群

## r.kind_dm
私聊

## r.post_repeated
第 {index} 条和前面某一条一模一样。删掉重复的那条再发。

## r.dm_member_only
{channel} 是私聊，只有那两个人能在里面说话（总机组也一样，私聊不能拉人）。要找其中一个人，用 to 填他的号码另开私聊。

## r.post_to_human
人类没有私聊：找人发到你的 home 人类群 {home}（channel 填 {home_id}），要人拍板用 ask。

## r.post_to_human_no_channel
人类没有私聊，你的 home 人类群也还没建：要人拍板用 ask（它会发进那个人类群）。

## r.human_not_watching_no_home
人类现在没有注视{channel}：@人类 的标记已经亮了，但可能很久才被看到。你的 home 人类群还没建；急事或要拍板的事用 ask。

## r.home_human_absent
- （你的 home 人类群还没建：找人用 ask，第一次 ask 会建它）

## r.list_none
- （这里没有你读得到的群。不带 crew 再列一次看你自己的群，或用 directory 查别的机组）

## r.list_none_mine
- （没有你读得到的未归档的群。要看已归档的：list_channels(include_archived=true)；找人用 directory 查号、post(to:) 私聊；要人拍板用 ask）

## r.channel_created
建好了：{name}（{id}），你是群主，成员 {count} 人。拉人用 channel_invite，发言用 post(channel="{id}")。

## r.invited
已拉进 {channel}：现在 {count} 人。他们之后会收到这个群的新消息；要叫醒谁就在 post 里 @ 他。

## r.left
已退出 {channel}：它的新消息不再注入给你。还想看时用 read_channel，要回来请成员 channel_invite 你。

## r.archived
已归档 {channel}：消息保留，不能再发言。要恢复请人解档。

## r.owner_offer_notice
我想把 {channel} 的群主转给你：同意就调 channel_owner(channel="{id}") 接手。

## r.owner_offered
已请 {to} 接手 {channel} 的群主，并在群里 @ 了对方；对方调 channel_owner(channel="{id}") 接受后生效，在那之前你还是群主。

## r.owner_accepted
你现在是 {channel} 的群主：人类在群里没点名时，调度判断不了就会叫你。

## r.owner_forced
{channel} 的群主已换成 {to}。对方不会因此被叫醒：需要时 post @ 对方说明。

## r.listen_minutes
minutes 要在 1–480 之间；不填就一直听，直到 channel_listen(channel=…, off=true)。

## r.listening_for
在收听 {channel}，{minutes} 分钟后自动停。正常结束回合等着就行。

## r.listening
在收听 {channel}，直到 channel_listen(channel="{id}", off=true)。正常结束回合等着就行。

## r.unlistened
已停止收听 {channel}。要再听就 channel_listen(channel="{id}")。

## r.unlisten_none
你本来就没在收听 {channel}，什么都没变。

## r.dir_header
通讯录（你的号码：{me}）：

## r.dir_header_no_number
通讯录（你没有分机号；别人可以用你的 session id {id} 找你）：

## r.dir_crew
- {number} 机组 {name}{path}

## r.dir_path
（上级：{path}）

## r.dir_session
- {number} {name} · {crew} · {id}

## r.dir_empty
没有匹配的号码：换个关键词，或不带 query 看全表。

## r.owner_force_needs_to
force 要和 to 一起给（新群主是谁）；不给 to 是接受转给你的群主，那时不要带 force。

## r.listen_off_alone
off=true 只停止收听，不能同时给 minutes 或 senders；要改收听的时长或发言者，不带 off 重新 channel_listen。

## r.owner_offer_notice_human
我想把 {channel} 的群主转给你：要接手的话，在 app 里这个群的菜单里把群主设成自己。

## r.fallback_pair
fallback_after_minutes（1 分钟到 7 天）和 fallback（不能是空的）要一起给，或者都不给；改好再调一次 ask。

## r.asked
已记成 {ask}（「{question_head}」），发进了 {channel} 并 @了人类。不用等：去干别的或收工，人回答时会叫醒你并念回 resume_note。

## r.ask_no_resume_note
你没写 resume_note：被叫醒时你只看得到问题和答复。半路上问的问题，最好用 withdraw_ask 撤掉、写上答复后接着做什么再问一次。

## r.withdrawn
已撤回 {crew} 的 {ask}，人类群里会留一行记录。这件事如果还要人拍板，用 ask 重新问。

## r.responded
已回应 Todo #{number}（「{todo_head}」）。状态没变：有新进展再 respond_todo，做完了在回应里说清结果。

## r.quota
{report}
没有剩余量的绝对值，别据此编造额度。额度紧时先收尾，再用 schedule_wakeup 约在重置后接着做。

## r.quota_none
现在拿不到额度信息。照常干活；被限额挡住时用 schedule_wakeup 约在稍后接着做。

## r.wakeup_note
note 必填：醒来的你只有这条备注和群消息可依靠。写上醒来该干什么、上下文在哪，再调一次。

## r.wakeup_when
after_minutes（1–1440）或 at（未来 7 天内的 RFC 3339 时刻，带上你指的时区偏移，如 2026-10-08T09:00:00+08:00；Z 表示 UTC）二选一；改好再调一次。

## r.wakeup_set
已约好 {at}（{delta}）叫醒你；间隔不对多半是时区偏移写错了：另约一个对的时间（这一个撤不掉，到点醒来时按 note 判断要不要做）。到点时如果你不在运行，会被拉起来接着做。现在可以收尾或去干别的。

## r.wakeup_in
{hours} 小时 {minutes} 分后

## r.wakeup_skipped
你的机组已被移除，什么都没约上。不用再约：机组被人恢复之后再安排。

## r.profile_empty
model 和 effort 至少给一个；补上再调一次。

## r.profile_next_start
已记下：下一次启动这个 session（重启或续跑）时生效；这一回合和接下来的工具调用仍是原来的配置。照常继续手上的活。

## r.not_your_crew
「{crew}」不归你管：你不是它的机长。要它做什么，找它的机长：post(to: "{number}")。

## r.brief_empty
brief 是空的：写清要它做什么、做到什么程度算完，再调一次。

## r.session_started
已起 worker「{title}」（{id}）。它上线后会在群里出声；要补充说明就 post(to: "{id}")，一直没动静就 inspect_session 看看。

## r.stop_reason_empty
reason 是空的：写一句为什么停（会公开写进大厅），再调一次。

## r.stop_self
不能停你自己。要结束就收尾、在群里说一声，然后结束这一轮。

## r.stop_notice
正在停掉 {target}：{reason}

## r.session_stopped
已停掉 {target}，原因已写进「{crew}」的大厅。它的活还要接着做，就 start_session 另起一个。

## r.inspect
{target} 的现场如下。卡在要人回答的框上就 ask 人；要它改做别的就 post(to: …) 给它：
{report}

## r.nudge_keys_only
nudge_session 只收按键 esc 或 interrupt（打断它当前这一轮），不收「{key}」。要告诉它什么，用 post(to: …) 发给它，它被唤醒时会读到。

## r.nudged
已给 {target} 发出 {key}。只是发出、没确认生效：过几秒 inspect_session 看效果。

## r.sessions_header
「{crew}」的 session（id · 号 · 名字 · 角色 · 状态）：

## r.session_line
- {id} · {address} · {name} · {role} · {state}

## r.sessions_none
（还没有 session）

## r.sessions_tail
看某个的现场用 inspect_session；要它做事就 post(to: 号)。

## r.no_address
无号

## r.role_captain
机长

## r.role_worker
worker

## r.state_stopped
没在跑

## r.state_idle
空闲

## r.state_working
干活中

## r.state_waiting
在等回答

## r.state_unknown
状态不明

## r.handoff_one_of
to_session 和 runner 给且只给一个：交给现有 session 用 to_session，新起一个接任用 runner（model / effort / title / opening_brief 只配 runner 用）。改好再调一次。

## r.handoff_already
{target} 已经是「{crew}」的机长，没有交接。要换人，就给 to_session 另一个 session，或用 runner 新起一个。

## r.handoff_done
「{crew}」的机长已交给 {new}（{id}）。原机长 {old} 留在机组里当 worker。有事要交代新机长，就 post(to: "{id}")。

## r.handoff_done_vacant
「{crew}」的机长已交给 {new}（{id}）；之前没有机长。有事要交代它，就 post(to: "{id}")。

## r.handoff_self
你现在是 worker 了：下一次调用起，你的工具表就变了。把手上在协调的事交代给新机长，再接着做你自己的活。

## r.handoff_default_brief
你刚接任本机组的机长：先读大厅最近的消息，确认手上在办的事和各 worker 的状态，再在大厅说一句你接手了。

## r.default_captain_title
机长

## r.stop_not_done
没停：{detail}。原因没能写进大厅，所以什么都没停掉；按上面的说明处理后再调一次。

## r.stop_failed_after_notice
大厅里已经发了「正在停掉 {target}」，但没停成：{detail}。什么都没停掉；先 inspect_session 看它现在的状态，再决定重试还是在大厅更正。

## r.handoff_started_not_handed
已起 {new}（{id}），但机长没交成：{detail}。它现在在跑、还是 worker；别再起一个：重试就 handoff_captain(to_session: "{id}")，不要它了就 stop_session 停掉。

## r.not_your_crew_unnumbered
「{crew}」不归你管：你不是它的机长。它没有号，没法 post(to:) 找它；要它做什么，就在本机组大厅说明，或用 ask 问人。
## r.renamed
「{old}」已改名为「{name}」，侧栏和通讯录用新名字。接着做手上的事。

## r.status_too_long
状态有 {count} 个字，超过 {max} 字的上限：压成一句再调一次。

## r.status_set
「{crew}」的状态已更新：{text}
情况有变再更新它。

## r.status_cleared
「{crew}」的状态已清掉。有新情况时再用 set_crew_status 写一句。

## r.policy
「{crew}」的派活原则：
{text}
要改就 delegation_policy(text: 完整的新正文)。

## r.policy_empty
「{crew}」还没有派活原则。要定一份就 delegation_policy(text: 完整正文)。

## r.policy_blank
text 是空的。只读就别给 text；要改就给完整的新正文，再调一次。

## r.policy_updated
「{crew}」的派活原则已整个替换。要核对就 delegation_policy() 读一遍。

## r.workdir_missing
{path} 不是已存在的目录的绝对路径：先确认目录在、用绝对路径，再调一次。

## r.workdir_none
（没有）

## r.workdir_line
- 「{crew}」现在：{old}

## r.workdir_preview
预览（还没改）：下面 {count} 个机组的工作目录会改成 {path}：
{crews}
没问题就带上 confirm: true、其余参数不变再调一次。

## r.workdir_done
已把下面 {count} 个机组的工作目录改成 {path}（右边是原来的）：
{crews}
之后起的 session 在新目录里工作；正在跑的进程不换目录，要它用新目录就停掉再起。
