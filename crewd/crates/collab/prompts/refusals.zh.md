给人和 agent 看的拒绝文字（crewd-collab 的规则拒绝读这里）：每节 `## 码`，码是 `模块.名字`，对应 src/refusal.rs 的 `Code`。
每条文字写「下一步怎么做」；`{占位符}` 由代码填，允许哪些见 `refusal_codes!`，两种语言必须一致。

## ask.id_not_found
机组 {crew} 里没有 {id}；先核对编号（问以 H 开头、工作项以 W 开头、条目以 # 开头）

## ask.bad_id
「{input}」不是合法的问号；写法是 H 加正整数，如 H3

## ask.system_cannot_withdraw
系统身份不能撤回问；请以 session 或人的身份撤回

## ask.remove_blocked
机组 {crew} 的 {ask} 还卡着活项；先答掉它或让负责人改道，再移除机组

## ask.withdraw_forbidden
{ask} 只能由提问者本人、问所在机组的机长、总机组的机长或人撤回；需要撤的话请他们来撤，或在群里说明情况

## ask.reason_empty
撤回理由不能为空；用一句话写清变了什么

## ask.not_open
{ask} 已经答过、按默认办了或撤回了，不能再撤；这条不用再处理

## ask.in_use
{ask} 还卡着一个活项，撤不了；先让负责人改道（把那一项报回 open / active），再撤

## ask.question_empty
问题不能为空；写清要人拍板的那件事，最好带上选项和你的倾向

## channel.not_found
没有群 {channel}；先列出现有的群，核对 id

## channel.system_cannot_act
系统不能以成员身份做这件事；请以 session 或人的身份操作

## channel.dm_one_human
私聊至多一方是人类；要找人说话，请发到机组的人类群

## channel.create_kind_fixed
只能建 topic 或 dm；大厅与人类群由 crewd 自己建，不用也不能手动建

## channel.name_empty
群名不能为空；给群起个名字

## channel.name_is_sender_label
群名 {value} 与发言者标签（人类 / crewd）相同，会被当成人类或 crewd 在说话；换一个群名

## channel.dm_needs_two
私聊恰好两个人（至多一方是人类）；要更多人就建一个 topic 群

## channel.dm_wrong_crew
私聊归属发起者所在的机组 {owner}，不是 {crew}；用 {owner} 这个机组 id 来建

## channel.manage_forbidden
只有群主、机长或人能改这个群；请他们来改，或请群主转让群主

## channel.fixed_name
大厅与人类群不能改名，它们的名字跟着机组走；要改就改机组名

## channel.fixed_no_archive
大厅与人类群不能手动归档，它们随机组移除而冻结；要停用就移除整个机组（crew.remove，可恢复）

## channel.lobby_cannot_add
大厅成员 = 本机组全体，不能手动加；要让谁进大厅，就让他成为本机组的 session

## channel.dm_cannot_grow
私聊只能两个人；要拉更多人就新建一个 topic 群

## channel.archived
群 {channel} 已归档，不收消息；要继续就解档，或新开一个群；它所在的机组若已被移除，请人先恢复机组（crew.restore）

## channel.add_members_member_only
只有群成员能拉人；请群里的成员来拉，或先让他们拉你进群

## channel.no_human_channel
{crew} 不是顶层机组，没有自己的人类群；人类群在 {home}/human，请发到那里

## channel.human_channel_other_home
{session} 的人类群是 {home}/human，进不了群 {channel}；跨机组说话走私聊

## channel.lobby_cannot_remove
大厅成员 = 本机组全体，不能手动移出；要让谁离开大厅，就让他离开本机组

## channel.dm_cannot_leave
私聊不能退出；要结束就归档

## channel.leave_forbidden
只能自己退群，或由群主 / 机长 / 人移出；请他们来移出

## channel.owner_must_transfer
群主要先把群主转让给别的成员再退群

## channel.not_explicit_member
不在这个群的成员名单里；大厅与机长的人类群里的隐式成员不能被移出，要让他离开就让他离开本机组，或换机长

## channel.owner_follows_captain
大厅与人类群的群主跟着机组机长走；要换群主就换机长

## channel.transfer_owner_only
只有群主能发起转让；请群主来发起，或请机长 / 人强制换群主

## channel.already_owner
你已经是群主，不用转让给自己；换一个接收人

## channel.new_owner_must_be_member
新群主必须是群成员；先把他拉进群

## channel.transfer_target_left
你已经不在这个群里，没法接受群主；请群主另选一个人

## channel.transfer_not_for_you
这次转让的对象不是你；只有被指定的人能接受

## channel.no_pending_transfer
没有待接受的群主转让；先由群主发起转让

## channel.transfer_withdraw_forbidden
只有群主、被转让的人、机长或人能撤回这次转让；请他们来撤回

## channel.force_owner_forbidden
只有机长或人能强制换群主；请他们来操作，或请群主发起转让

## crew.not_found
没有机组 {crew}；先列出机组，核对 id

## crew.removed
机组 {crew} 已被移除（隐藏并冻结，没有删除），只读；请人先恢复它（crew.restore，只有人在客户端能做）

## crew.management_fixed
总机组是管理机组，不能隐藏、移除或永久删除；保持原样即可，要整理侧栏就隐藏或移除别的机组

## crew.management_not_in_tree
总机组不在机组树里：它不能有上级或下级，别的机组也不能挂到它下面；换一个机组当上级，或者不给 parentId、建成顶层机组

## crew.title_empty
机组名不能为空；给机组起个名字

## crew.working_dir_empty
工作目录不能为空；填机组 session 要用的目录路径

## crew.cannot_parent_self
机组不能挂在自己下面；换一个别的上级

## crew.would_cycle
{parent} 是 {crew} 的下级，挂上去会成环；换一个上级，或先把 {parent} 从 {crew} 下摘出

## crew.not_direct_child
{crew} 不是 {from} 的直系子机组；核对上下级关系再摘

## crew.running_sessions
机组 {crew} 还有 {running} 个在跑的 session；先停掉它们

## crew.live_children
机组 {crew} 下面还有没移除的子机组 {children}；先把它们挪走（crew.release）或移除

## crew.wakes_inflight
机组 {crew} 还有 {count} 条唤醒正在投递；等投递结束再移除

## crew.parent_removed
机组 {crew} 的上级 {parents} 已被移除，被移除的机组下面不挂活的；请人先恢复 {parents}，恢复之后若不想让 {crew} 继续挂在它们下面，再用 release 把 {crew} 摘出

## crew.not_removed
机组 {crew} 还没移除；永久删除只收已移除的机组，先移除（crew.remove）。永久删除会真的删掉这个机组的一切，删前的快照只留 30 天

## crew.session_not_in_crew
{session} 不在机组 {crew} 里；机长只能从本机组的 session 里选

## crew.reorder_mismatch
crewIds 必须包含这一层所有没移除的机组，不重复、不含别的机组（已移除的可带可不带）：现有 {current}

## crew.no_captain
机组 {crew} 现在没有机长；先给它指定机长，或改发给具体的 session

## directory.bad_address
地址「{address}」不对：写机组号或「机组号-分机号」，如 3 或 3-2

## directory.no_such_crew
通讯录里没有 {number} 号机组；核对机组号

## directory.crew_removed
{number} 号机组已被移除，不收消息；要联系它请先请人恢复它

## directory.no_such_extension
{number} 号机组没有分机 {ext}；核对分机号

## message.not_found
没有消息 {message}；核对消息 id

## message.seq_not_found
没有 seq={seq} 的消息；核对序号

## message.bad_time
时间「{value}」不是 RFC 3339（{why}）；写成 2026-01-02T03:04:05Z 这样的格式

## message.search_empty
搜索词不能为空；写要找的字

## message.not_a_member
{who} 不是群 {channel} 的成员；成员有：{members}

## message.captain_not_a_member
机组机长 {captain} 不是群 {channel} 的成员，@ 它叫不醒；成员有：{members}

## message.empty
消息不能为空；写上要说的话

## message.too_long
消息正文最长 {max} 字节，这条有 {actual} 字节；拆成几条，或把长内容写进文件再引用

## message.reserved_category
分类 {category} 留给系统公告，不能由发言带上；去掉分类再发

## message.attachments_unsupported
附件上传尚未实现；把内容贴进正文，或写成文件再给路径

## message.sender_not_member
{session} 不是群 {channel} 的成员；请群里的成员先把你拉进群，或改发到你所在的群

## message.mention_needs_target
@session 要带 targetId；写明要叫醒的是哪个 session

## message.reply_other_channel
只能回复同一个群里的消息；要回别的群的消息，请到那个群里回

## message.id_conflict
新消息 id 冲突；重发一次

## restart.not_found
没有重启询问 {id}；核对编号

## restart.already_resumed
已经答过「恢复」，不用再答

## restart.superseded
重启询问 {id} 已被更新的一条取代；去答最新的那条

## session.not_found
没有 session {session}；核对 session id

## session.bad_executor
执行器 {executor} 不认识；可选 claude、codex，或 acp:<id>

## session.title_too_long
session 标题最长 {max} 个字（现在 {actual} 个）；写短一点，细节放进 brief

## session.title_or_brief_required
session 要有标题（不超过 {max} 个字）或 brief；两者都没有就没法给它起名，补上一个

## session.title_is_sender_label
标题 {value} 与发言者标签（人类 / crewd）相同，会被当成人类或 crewd 在说话；换一个标题

## session.display_name_is_sender_label
显示名 {value} 与发言者标签（人类 / crewd）相同，会被当成人类或 crewd 在说话；换一个显示名

## session.captain_exists
机组 {crew} 已有机长；要换机长请用 set_captain

## settings.bad_locale
语言 {value} 不认识（库里存的或客户端给的），只认 zh / en；改成 zh 或 en

## settings.bad_max_live
meta.maxLiveSessions 存的是 {value}，不是正整数；改成正整数，或清掉它

## settings.bad_hibernate_deadline
meta.hibernate.deadlineSecs 存的是 {value}，不是非负整数（秒）；改成非负整数，或清掉它回到默认值

## settings.max_live_min
上限必须 ≥ 1；不设上限请传 null

## todo.response_empty
回应不能为空；写上回应内容

## todo.not_found
本机组没有 Agent Todo #{number}；核对编号

## wake.source_empty
唤醒的 source 不能为空；写明这次唤醒的来源

## wake.not_found
没有唤醒 {wake}；核对唤醒 id

## wake.bad_transition
唤醒 {wake} 是 {from}，不能改成 {to}；先看它现在的状态

## wake.latch_bad_scope
熔断台账的 id 不能为空，也不能含箭头字符；收到：{scope}。换一个只含普通字符的 id

## wake.latch_row_unreadable
wake_latches 里有认不出的行：scope={scope} id={id}；这是库里的脏数据，需要人处理

## listen.until_in_past
收听截止时间必须晚于现在；给一个将来的时间，或不设截止

## listen.not_readable
你读不到群 {channel}，不能收听它：收听不扩大读范围。只能收听读得到的群（自己机组的群、自己在的群、自己 home 的人类群）；要听这个群，请它所在机组的机长把你拉进群

## session.already_running
session {session} 已经在跑或正在起；不用再起，要它干活就给它发消息

## session.not_running
session {session} 没在跑；先恢复它，或给它发消息让它被拉起

## session.at_cap
已达设置里的并发 session 上限 {max}；等有 session 停下，或在设置里调高 / 清掉上限

## session.executor_unavailable
执行器 {executor} 在这台 crewd 上用不了（没接上）；改用 claude，或先接上它

## session.no_workdir
session {session} 没有可用的工作目录（它的 worktree 与机组目录都没有或已被删）；给机组设好工作目录再试


## orchestra.not_running
这台 crewd 没有启用会话管理（没配 agent 执行器）；用 crewd serve --claude <路径> 重新启动后再试

## orchestra.binary_replaced

crewd 的可执行文件已被替换。请重启 crewd 后再启动或恢复会话。

## orchestra.shutting_down
crewd 正在停机或休眠，不再起、恢复或叫醒 session；等它重新起来后再试

## orchestra.nudge_empty
nudge 的输入是空的；不会替它按一个裸回车（那会选中当前高亮的那一项），写明要按的键或要发的文字


## loop.edge_needs_from
scope=edge 要给 from（叫醒它的来源：session id 或 system）；要解这个 session 的整体熔断，用 scope=session

## loop.chain_needs_identity
scope=chain 要同时给非空的 chain_id、from 和 target，指定要恢复的因果链及其投递路径

## loop.unexpected_chain_id
chain_id 只用于 scope=chain；恢复 edge 或 session 时不要给 chain_id

## system.api_incompatible
客户端的 API 版本 {client} 太旧：这台 crewd 支持 {min} 到 {current}；更新客户端后再连

## crew.no_workdir
机组 {crew} 没有工作目录，没法为 session 新建 worktree；先给机组设好工作目录，或用 isolation=false

## decision.none
session {session} 屏上没有待决策的框，没有要按的键；要它干活就给它发消息

## decision.changed
session {session} 屏上的框变了（已被答掉、换成了别的框，或高亮被挪了），没有再往下按；看一眼它现在的框再决定

## decision.needs_terminal
session {session} 屏上的框要靠方向键选，但看不出高亮停在哪一项；打开它的终端直接选

## decision.out_of_range
session {session} 屏上的框只有 {options} 项，没有你选的那一项；按屏上的选项重新选

## session.input_draft
session {session} 的输入框里有一段还没发出的草稿：{text}；这次什么都没敲进去，等人把它发出去或清掉再试

## session.input_draft_unknown
session {session} 的终端里有人敲过字，此刻分不清它的输入框里是没发出的草稿还是空框的提示；这次什么都没敲进去，等人把输入框发出去或清空再试

## session.nothing_to_interrupt
session {session} 没在干活、屏上也没有待决策的框，没有可打断的；要它干活就给它发消息

## orchestra.keys_human_only
请交给人类在客户端回答；agent 不能替人按决策键。

## orchestra.nudge_busy
文字 nudge 只允许发给空闲会话。

## session.nudge_failed
nudge 未送达（{outcome}）：{detail}

## session.worktree.dir_missing
目录不存在: {path}. {detail}

## session.worktree.not_repo
不是 git 仓库: {path}. {detail}

## session.worktree.detached_head
当前是 detached HEAD: {path}. {detail}

## session.worktree.no_toplevel
查不到仓库根: {path}. {detail}

## session.worktree.mkdir
无法创建 worktree 目录: {path}. {detail}

## session.worktree.git_missing
无法运行 git: {path}. {detail}

## session.worktree.timeout
git 超时，已中止: {path}. {detail}

## session.worktree.pipes_held
git 已退出但输出管道一直被占用；到期限已中止: {path}. {detail}

## session.worktree.add_failed
git worktree add 失败: {path}. {detail}

## session.worktree.wait_failed
无法等待 git 结束: {path}. {detail}

## session.worktree.task_failed
新建 worktree 任务失败: {path}. {detail}

## work.bad_id
全局计划编号格式不正确：{input}。

## work.not_found
计划 {id} 不存在。

## work.bad_limit
工作列表每页须为 1 到 200 项。

## work.bad_revision
修订号须使用 work.get 返回的当前修订字符串。

## work.revision_conflict
计划 {id} 已变化，本次未写入。请核对回复中的当前事实，再决定是否更新；需要详情时读取计划。

## work.actor_forbidden
此调用者不能执行公开工作命令。

## work.link_other_crew
跨机组链接 {id} 需要尚未接线的全局关系层。

## work.bad_time
须提供 RFC3339 时间：{value}。

## work.bad_update
进展更新须有备注；凭据仅用于完成，完成不能安排下次检查。

## work.history_cursor
历史游标应为正整数的规范字符串，请使用上一页返回的 nextBefore。

## crew.work_referenced

机组 {crew} 不能永久删除：其他机组仍有 {total} 个对象引用它的工作、session、文档或群分类。引用：{ids}。请先处理这些关系；已移除机组继续只读。


## todo.human_only

只有人可以交代或裁定关闭原始需求。

## todo.text_empty

原始需求正文不能为空。

## todo.request_invalid

requestId 必须是 UUID。

## todo.request_conflict

请求 {request} 已对应另一份需求内容。

## todo.id_invalid

请使用已发放的全局原始需求编号，例如 #1。

## todo.status_invalid

expectedStatus 必须是原始需求状态。

## todo.status_conflict

原始需求 {id} 已从 {expected} 变成 {actual}；刷新后再裁定。

## resource.invalid
{field}无效，请修正该字段后重试。

## resource.not_found
对象{id}不可用，请刷新当前视图。

## resource.forbidden
当前连接没有所需的实际机组或对象访问权限。

## resource.request_conflict
请求{request}已对应不同的原始内容。

## document.revision_conflict
文档{id}已经变化，请核对当前元数据后编辑。

## document.deleted
文档{id}已删除，保留的历史只能读取。

## view_folder.revision_conflict
文件夹分类已变化，请核对当前修订。

## view_folder.limit
当前范围上限为{limit}，请减少这组内容。

## view_folder.cycle
此移动会形成循环或超过文件夹深度上限。

## view_folder.object_scope
对象不属于此视图或当前不可见。

## channel.work_revision_conflict
群工作引用已经变化。请刷新当前群的工作视图再重试。

## channel.work_cursor_expired
群工作引用已经变化。请刷新当前群的工作视图再重试。

## work.owner_required
请先指派负责人，再将工作推进为进行中或标为阻塞。
