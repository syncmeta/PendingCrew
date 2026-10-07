给人和 agent 看的拒绝文字（crewd-collab 的规则拒绝读这里）：每节 `## 码`，码是 `模块.名字`，对应 src/refusal.rs 的 `Code`。
每条文字写「下一步怎么做」；`{占位符}` 由代码填，允许哪些见 `refusal_codes!`，两种语言必须一致。

## ask.id_not_found
机组 {crew} 里没有 {id}；先核对编号（问以 H 开头、工作项以 W 开头、条目以 # 开头）

## ask.bad_id
「{input}」不是合法的问号；写法是 H 加正整数，如 H3

## ask.system_cannot_withdraw
系统身份不能撤回问；请以 session 或人的身份撤回

## ask.archive_blocked
机组 {crew} 的 {ask} 还卡着活项；先答掉它或让负责人改道，再归档

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
大厅与人类群不能手动归档，它们随机组归档；要停用就归档整个机组

## channel.lobby_cannot_add
大厅成员 = 本机组全体，不能手动加；要让谁进大厅，就让他成为本机组的 session

## channel.dm_cannot_grow
私聊只能两个人；要拉更多人就新建一个 topic 群

## channel.archived
群 {channel} 已归档，不收消息；要继续就解档，或新开一个群；它所在的机组若已归档，请人先解档机组

## channel.add_members_member_only
只有群成员能拉人；请群里的成员来拉，或先让他们拉你进群

## channel.no_human_channel
{crew} 不是一级机组，没有自己的人类群；人类群在 {home}/human，请发到那里

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

## crew.archived
机组 {crew} 已归档，只读；请人先解档它（解档只有人在客户端能做）

## crew.org_root_fixed
总机组是组织的根，不能被收编、摘出、隐藏、删除或归档；要整理组织就动它下面的机组

## crew.title_empty
机组名不能为空；给机组起个名字

## crew.working_dir_empty
工作目录不能为空；填机组 session 要用的目录路径

## crew.move_to_first_level
要挪到一级：先用 release 摘掉 {crew} 的其它上级

## crew.cannot_parent_self
机组不能挂在自己下面；换一个别的上级

## crew.would_cycle
{parent} 是 {crew} 的下级，挂上去会成环；换一个上级，或先把 {parent} 从 {crew} 下摘出

## crew.not_direct_child
{crew} 不是 {from} 的直系子机组；核对上下级关系再摘

## crew.release_to_root_other_parents
要摘到总机组下：先解除 {crew} 的其它上级（release），只剩 {from} 时再摘

## crew.running_sessions
机组 {crew} 还有 {running} 个在跑的 session；先停掉它们

## crew.live_children
机组 {crew} 下面还有没归档的子机组 {children}；先把它们挪走（crew.release）或归档

## crew.wakes_inflight
机组 {crew} 还有 {count} 条唤醒正在投递；等投递结束再归档

## crew.parent_archived
机组 {crew} 的上级 {parents} 已归档，归档的机组下面不挂活的；请人先解档 {parents}，解档之后若不想让 {crew} 继续挂在它们下面，再用 release 把 {crew} 摘出

## crew.not_archived
机组 {crew} 还没归档；永久删除只收已归档的机组，先归档（crew.delete）

## crew.session_not_in_crew
{session} 不在机组 {crew} 里；机长只能从本机组的 session 里选

## crew.reorder_mismatch
crewIds 必须包含这一层所有没归档的机组，不重复、不含别的机组（归档的可带可不带）：现有 {current}

## crew.no_captain
机组 {crew} 现在没有机长；先给它指定机长，或改发给具体的 session

## directory.bad_address
地址「{address}」不对：写机组号或「机组号-分机号」，如 3 或 3-2

## directory.no_such_crew
通讯录里没有 {number} 号机组；核对机组号

## directory.crew_archived
{number} 号机组已归档，不收消息；要联系它请先请人解档

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
meta.locale 存的是 {value}，只认 zh / en；改成 zh 或 en

## settings.bad_max_live
meta.maxLiveSessions 存的是 {value}，不是正整数；改成正整数，或清掉它

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
