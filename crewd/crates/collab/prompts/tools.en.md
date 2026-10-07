<!--
Agent-facing tool text (spec §22.3). Everything before the first section heading is a comment (no line in it may start with "## ").
Section names: <tool> = tool description; <tool>.<param> = parameter text; r.* = receipts; trace.* = relay posts; inject.* = injection extras.
Placeholders {name} are filled by code (M3a's prompts::fill). zh and en must have exactly the same section names (tested).
-->

## post
Post key moments to a channel: started, done, blocked, handing off, important findings. Keep terminal detail (what you read or ran) out of channels.
Say where it goes; there is no default: channel = post in a channel; to = a dm with one session by its directory number (also across crews); reply_to alone = the channel of that message. With none of the three you get an error listing your channels.
Mentions only decide who is woken: mention no one and no one is woken (except in a dm: the other side is always woken); @session / @captain wake the named agents, @human flags the message for the human; a mention never narrows visibility — everyone who can read the channel still sees the message. @human in a channel the human is not watching lights the badge but may be seen much later — for anything urgent post in your home (top-level crew) human channel or use ask.
Posting too fast, repeating the same text, or @human too often is held back (the receipt says what to do): merge your updates into one message.
The receipt says whom the post wakes. A message body is at most 256 KiB.

## post.channel
Channel id, or a channel name that is unique among the channels you can read. Not together with to.

## post.to
A dm with one session by its directory number: 7-3 (crew 7, extension 3), 7 (crew 7's current captain), 0 (the management crew's captain); look numbers up with directory. A pair has one dm, reused when it exists. The human has no dm (to: "human" points you to your home human channel): reach the human there, and use ask for a decision. Not together with channel or reply_to.

## post.message
Use for a single message. Mutually exclusive with messages.

## post.messages
Several messages at once (at most 6), one bubble each with its own category. If any item is invalid nothing is posted and the error names the item. mentions and reply_to apply to the first message only.

## post.mentions
Who to wake: {kind:"session", target_id:"<session id or directory number>"} / {kind:"captain"} / {kind:"human"}. Omit to wake no one; everyone who can read the channel still sees it. A mentioned session must be a member of the channel; @captain likewise requires the captain to be a member, and @captain in a crew without a captain is an error.

## post.reply_to
The message you are replying to: its number (#128 or 128) or its id. Same channel only. The original sender is mentioned automatically (while they are still in the channel): all channel members still see it, the sender is woken; replying to your own message or to a system message adds no mention.

## post.category
What this is: progress / done / blocked / question / finding / handoff / ack / note.


## read_channel
Read a channel page by page (oldest first). Reading up to the newest message marks the channel read for you, so those messages are not injected again — only for channels you are in or listen to (reading other channels does not move your read position). The #number before each message can go into reply_to.

## read_channel.channel
Channel id or name; omitted = your crew's lobby. Readable: your crew's channels (dms between its members included — there are no private channels), your home human channel, channels you are in or listen to; management crew sessions read the whole organisation.

## read_channel.limit
Number of messages, 1–200, default 50.

## read_channel.before
Only messages before this one (number or id). Omitted = newest page.

## search
Search messages in the channels you can read (your crew's channels, your home human channel, channels you are in or listen to, archived ones included; management crew sessions search the whole organisation). The whole trimmed query is matched as ONE case-insensitive substring (ASCII case folding only), not split into terms; newest first.

## search.query
The text to find: trimmed, then matched as one substring (ASCII case-insensitive).

## search.channel
Search only this channel (id or name).

## search.crew
Search only that crew's channels you can read (crew number, id or name).

## search.after
Only at or after this time (ISO8601).

## search.before
Only strictly before this time (ISO8601, exclusive: created_at < before).

## search.limit
Maximum results, 1–200, default 50.

## list_channels
List channels: first your home human channel (post there to reach the human or get a decision), then channels you are in or listen to, the other channels of your crew, and channels the human is watching. Each row: id · name · type · purpose · owner · members · state (member / listening / human watching / archived / you own it).

## list_channels.crew
List only that crew's channels you can read (crew number, id or name).

## list_channels.include_archived
true = include archived channels; they are left out by default.

## channel_create
Create a topic channel. Use it whenever you need to collaborate with someone on something or discuss a topic separately — do not create a child crew for that. You become the owner and a member. Give it a name and a purpose, e.g. "release", "scroll bug". A name equal to the human labels (人类 / Human) or to crewd's own system labels is rejected. A dm needs no creating: post(to:) opens one when there is none yet.

## channel_create.name
Short channel name.

## channel_create.purpose
One sentence: what the channel is for.

## channel_create.members
Members to add right away (optional; channel_invite later works too): session ids, directory numbers (like 7-3; 7 = crew 7's captain right now), or human. Leave yourself out.

## channel_invite
Add members to a channel. Usually you must be a member yourself; the lobby and a dm cannot take invites.

## channel_invite.channel
Channel id or name.

## channel_invite.members
Session ids, directory numbers (like 7-3), or human.

## channel_leave
Leave a channel. You cannot leave the lobby; a dm cannot be left (archive it instead); the owner of a topic channel must hand ownership on with channel_owner first.

## channel_leave.channel
Channel id or name.

## channel_archive
Archive a channel (messages kept, no new posts; the human can unarchive). Only the owner, the captain of the channel's crew or the human can archive; the lobby and the human channel cannot be archived.

## channel_archive.channel
Channel id or name.

## channel_owner
Channel ownership. The owner acts as the channel's captain: when the human posts without naming anyone and the dispatcher cannot tell who should answer, the owner is woken. Three uses: with to = the owner offers it (you mention them in the channel under your own name; it takes effect once they call channel_owner(channel=…)); without to = accept an ownership offered to you; with to and force=true = the captain of the channel's crew (the management captain counts) moves it at once, without acceptance. The new owner must be a member.

## channel_owner.channel
Channel id or name.

## channel_owner.to
New owner: session id or directory number (like 7-3). Omitted = accept the ownership offered to you.

## channel_owner.force
For captains, together with to: true = move ownership without waiting for acceptance.

## channel_listen
Listen to a channel: new messages there wake you even without a mention. Use it while waiting for activity; end your turn normally while listening instead of polling read_channel. Only channels you can read can be listened to. Stop with channel_listen(channel=…, off=true).

## channel_listen.channel
Channel id or name.

## channel_listen.minutes
How long (minutes, 1–480). Omitted = until you stop it with off=true.

## channel_listen.senders
Only these senders: human, captain (whoever is captain when the message is posted; follows captain changes), session ids or directory numbers (a number is resolved to its session now, e.g. 7-1 = crew 7's captain right now, and does not follow later changes). This only filters whose messages wake you; everything in the channel is still injected. Omitted = everyone.

## channel_listen.off
true = stop listening to this channel (not together with minutes or senders). Not listening already is not an error.

## directory
Look up the directory: every crew and session on this machine has a short number — crews are integers (7), members are extensions (7-3), -1 is always that crew's captain right now, and 0 is the management crew (0 = its captain right now). Each row: number, name, crew or parent path, session id. Numbers go straight into post's to (a dm, across crews too) and into member arguments of the tools. Channels are listed by list_channels; the human has no number: post in your home human channel, or ask for a decision.

## directory.query
Filter by number prefix, name or org path; omitted = everything.

## ask
Ask the human for a decision. Non-blocking: the question becomes a pending item (an H number), is posted to your home (top-level crew; the management crew is its own home) human channel mentioning the human, and you get a receipt right away — go do something else or end your turn. When the human answers you are woken and resume_note is read back to you verbatim.

## ask.question
The decision you need. State the options and your preference — "A / B, I lean A because …".

## ask.resume_note
What you will do once answered (where you are, what is next). Always write it for questions asked mid-task.

## ask.fallback_after_minutes
Optional: if nobody answers within this many minutes, you are woken to act on fallback. Only for reversible things.

## ask.fallback
What you will do if nobody answers in time ("go with A"). Give together with fallback_after_minutes.

## withdraw_ask
Withdraw an unanswered question that no longer applies. You can withdraw your own; the captain can withdraw any in the crew (the management crew's captain counts as the captain of every crew). A question still blocking a work item cannot be withdrawn.

## withdraw_ask.ask
The H number, like H3.

## withdraw_ask.reason
Why it no longer applies: what changed.

## withdraw_ask.crew
Whose book holds the question (crew number or id); omitted = your own crew.

## respond_todo
Respond to an item in your crew's Agent Todo list (work the human assigned to the crew): claim, progress, result. Append-only; does not change status.

## respond_todo.number
The N in #N.

## respond_todo.response
One or two sentences.

## get_quota
Subscription usage for claude / codex on this machine: percent used per window and reset times. Check before long work. There is no absolute remaining token count; do not invent one.

## schedule_wakeup
Wake yourself later: at the time, note is delivered back to you. When quota runs low: wrap up to a hand-off state, schedule a few minutes after the reset, stop. When you wake you only have the note and channel messages, so make the note self-contained.

## schedule_wakeup.after_minutes
Minutes from now (1–1440). Either this or at.

## schedule_wakeup.at
An RFC 3339 time carrying the offset you mean (like 2026-10-08T09:00:00+08:00; Z = UTC), in the future and at most 7 days ahead. Either this or after_minutes.

## schedule_wakeup.note
What to do on waking and where the context is.

## set_session_profile
Switch your own model or reasoning effort. Not immediate: this changes the session's setting, which takes effect the next time the session starts (restart or resume); the current turn is unchanged. Use the most expensive tier only when normal tiers clearly fail.

## set_session_profile.model
Model alias such as opus / sonnet.

## set_session_profile.effort
Reasoning effort level.

## rename_crew
(Captain) Give the crew a short label like "auth refactor". Do it once you know what the crew is doing.

## rename_crew.name
A few-word label, no punctuation.

## change_workdir
(Captain) Change the crew's working directory. Call without confirm for a preview, then with confirm: true. Only your crew and your child crews.

## change_workdir.new_path
Absolute path of an existing directory.

## change_workdir.crew
Which crew (name, id or number); omitted = your crew.

## change_workdir.include_children
Also change child crews; default true.

## change_workdir.confirm
true = apply; omitted = preview only.

## set_crew_status
(Captain) One sentence on how the whole crew is doing, shown on the crew's sidebar row. Over 200 characters is refused.

## set_crew_status.text
One-sentence status.

## read_delegation_policy
(Captain) Read the crew's delegation policy.

## update_delegation_policy
(Captain) Replace the crew's delegation policy with the full new text.

## update_delegation_policy.text
The full new text.

## start_session
(Captain) Start a worker for a clear coding task. crew omitted = this crew; another crew only if you act as its captain (the management crew's captain counts as the captain of every crew).

## start_session.brief
The task, with goal and done criteria.

## start_session.title
At most 18 characters, no project name; the worker's display name. Omitted = the first 18 characters of the brief's first line.

## start_session.runner
claude or codex; omitted = crew default.

## start_session.isolation
Required. false = use the crew's shared directory; true = create a separate git worktree for it. Decide per call from parallel conflicts, change scope and how the tasks relate; if creation fails you get an error, never a silent fallback to the shared directory.

## start_session.model
Optional model.

## start_session.effort
Optional reasoning effort.

## start_session.crew
Optional: the crew to start it in (number or id); omitted = this crew.

## stop_session
(Captain) Stop a session's process: the reason is posted in your name to its crew's lobby first, then it is stopped; irreversible. To interrupt one turn only, nudge_session with esc. You cannot stop yourself.

## stop_session.session_id
Target session: id or directory number (like 7-3).

## stop_session.reason
Why; posted publicly to the lobby.

## inspect_session
(Captain) See a session's state and recent terminal output (codex: transcript tail). Use it when a mention gets no answer.

## inspect_session.session_id
Target session: id or directory number (like 7-3).

## nudge_session
(Captain) Interrupt a session's current turn: only the keys esc / interrupt. To tell it something, post(to: …) to it; it reads that when woken. Sending is not confirmation: inspect_session a few seconds later to see the effect.

## nudge_session.session_id
Target session: id or directory number (like 7-3).

## nudge_session.key
esc or interrupt; both interrupt its current turn.

## list_sessions
(Captain) List a crew's sessions: id, number, name, role, state. crew omitted = this crew.

## list_sessions.crew
Optional: crew number or id; omitted = this crew.

## handoff_captain
(Captain) Hand over a crew's captain role: to_session hands it to an existing session (which must belong to that crew), or runner starts a new session to take over; give exactly one. crew omitted = this crew; it may also be a direct child crew of yours, or any crew you act as captain of. The old captain stays in the crew as a worker.

## handoff_captain.crew
Optional: whose captain role (crew number or id); omitted = this crew.

## handoff_captain.to_session
The successor: an existing session's id or directory number; it must belong to that crew.

## handoff_captain.runner
Start a new session to take over: claude or codex.

## handoff_captain.model
Optional: the new captain's model (with runner only).

## handoff_captain.effort
Optional: the new captain's reasoning effort (with runner only).

## handoff_captain.title
Optional: the new captain's display name, at most 18 characters; omitted = Captain (with runner only).

## handoff_captain.opening_brief
Optional: the new captain's first task (with runner only).

## adopt_crew
(Captain) Adopt a crew that has no parent/child relation with yours, making it your child. No cycles; the management crew is outside the crew tree and cannot be adopted.

## adopt_crew.crew
Target crew name, id or number.

## release_crew
(Captain) Re-home a direct child crew: without to it becomes top-level; with to it moves under another of your direct children.

## release_crew.crew
Direct child crew name, id or number.

## release_crew.to
Optional destination: another of your direct child crews.

## create_parent_crew
(Captain) Create a parent crew above yours: your crew becomes its child, it inherits the working directory and gets its own captain. Only when the organisation really needs another level (grouping departments under one parent).

## create_parent_crew.title
Optional short label.

## adopt_parent
(Captain) Make an existing crew your crew's parent. No cycles.

## adopt_parent.crew
Target crew name, id or number.

## create_child_crew
(Captain) Create a child crew under yours: it inherits the working directory, gets its own captain, and brief is that captain's first task. Child crews only express organisational structure — a lasting department or team with its own captain. To collaborate on a topic or a task, create a channel with channel_create instead.

## create_child_crew.brief
What the child crew should do.

## create_child_crew.title
Optional short label.

## create_child_crew.runner
Optional: claude or codex for the child captain.

## r.unknown_tool
There is no tool named {name}.

## r.captain_only
{name} is for the captain only, and you are not the captain now.

## r.bad_args
Invalid arguments: {detail}

## trace.worker
{label} stopped this turn on a question and is waiting for an answer:
{question}
Captain: inspect_session to see the state; to answer it, post(to: …) to it, and it reads that when woken; if you cannot decide, ask the human. (session: {session})

## trace.captain
{label} of {crew} stopped this turn on a question and is waiting for your answer:
{question}
Please open this session's terminal and reply. (session: {session})

## trace.crew
{title} (crew {number})

## r.failed
Did not go through: {detail}

## r.no_handler
{name} is not implemented yet.

## r.internal
Internal error #{n} (details are in the crewd log)

## inject.omitted
{count} newer messages in {channel} were left out this time (length cap); they are still unread: they will follow after your next tool call, or use read_channel to see them now.

## inject.omitted_at_least
At least {count} newer messages in {channel} were left out this time (length cap; the unread run is long, there may be more); they are still unread: they will follow after your next tool call, or use read_channel to see them now.

## inject.truncated
Message #{seq} in {channel} is too long; only its beginning is shown. For the full text: read_channel(channel="{id}", before={next}, limit=1).

## inject.digest
{channel}: {count} more progress / ack messages ({senders}, #{first}–#{last}) are folded, not listed one by one; read_channel shows them.

## r.lobby_label
lobby of "{crew}"

## r.human_label
human channel of "{crew}"

## r.dm_gone
dissolved direct message

## r.dm_generic
direct message

## r.post_one_of
Give either message or messages, not both; with messages put category inside each item.

## r.post_count
messages needs 1–6 items, you gave {count}. Split them over two posts, or merge them into one.

## r.post_empty
Item {index} is empty. Fill it in or drop it, then post again.

## r.bad_category
Item {index} has an unknown category "{value}"; choose from: {choices}.

## r.bad_mention
Invalid mention (kind = {kind}): kind must be session / captain / human, and session needs target_id (a session id or directory number).

## r.no_message
Message {ref} not found (or its channel is not readable for you). Numbers are the # at the start of each read_channel / search line.

## r.reply_other_channel
#{seq} is not in the channel you named; replies go to the original message's channel — just drop channel.

## r.post_partial
The first ones were posted ({sent}); item {index} of {total} was not: {detail}. Do not resend the batch, only the missing ones.

## r.post_no_target
Say where this goes: channel (a channel id or name), to (a directory number such as 7-3 / 7 / 0, for a dm) or reply_to (the message you answer). Channels you are in:
{choices}
Find other channels with list_channels and numbers with directory.

## r.post_target_conflict
Name one destination only: to is a dm and cannot go with channel or reply_to. Drop the extra one and post again.

## r.post_to_self
{address} is you. To reach someone else, look their number up with directory first.

## r.posted
Posted to {channel}: {seqs}.

## r.posted_dm
Sent to {peer} ({address}) in a dm: {seqs}. {new}

## r.dm_opened
This dm is new ({id}); keep talking there, or use to again.

## r.posted_wakes
Wakes: {names}.

## r.posted_wakes_none
This mentions no one, so it wakes no one; @ whoever needs to respond.

## r.posted_wakes_dm
In a dm the other side is always woken; no @ needed.

## r.posted_not_member
You are not a member of {channel}: replies will not wake you; channel_listen it to hear them.

## r.post_throttled
Posting too fast, so this was not posted: merge your update into one message and post it again in {retry_secs} s.

## r.post_duplicate
Same text as your #{seq}, so it was not posted again. Write what is new, or reply_to #{seq}.

## r.human_mention_throttled
Too many @human from you in the last 30 minutes, so this was not posted. For a decision use ask; otherwise post without @human.

## r.human_not_watching
The human is not watching {channel}: the @human badge is lit but may be seen much later. For urgent things or decisions post in your home human channel {home} ({home_id}, the first line of list_channels), or use ask.

## r.channel_choice
- {id} {name}

## r.no_channel
No channel "{arg}" (or you cannot read it). Channels you can use (at most 20, yours first):
{choices}
See all of them with list_channels.

## r.ambiguous_channel
More than one channel is called "{arg}"; use the id:
{choices}

## r.line
#{seq} {time} {who}: {text}

## r.line_attachment
 [attachment {path}]

## r.read_empty
{channel} has no messages yet.

## r.read_header
{count} messages in {channel} ({id}), oldest first:

## r.read_more
There are older ones: pass before = {before} to page back.

## r.search_none
Nothing found for "{query}". Try a shorter phrase, or another scope with crew / channel.

## r.search_hit
[{channel}] {line}

## r.list_header
Channels (id · name · type · purpose · owner · members · state):

## r.list_line
- {id} · {name} · {kind} · {purpose} · owner {owner} · {count} members · {flags}

## r.list_more
{count} more channels are not listed; narrow it down with crew.

## r.flag_home_human
your home human channel (post here to reach the human)

## r.flag_member
you are a member

## r.flag_listening
you are listening

## r.flag_watched
human is watching

## r.flag_archived
archived

## r.flag_owner
you own it

## r.owner_none
(none)

## r.kind_lobby
lobby

## r.kind_human
human channel

## r.kind_topic
topic

## r.kind_dm
dm

## r.post_repeated
Item {index} repeats an earlier item. Drop the repeat and post again.

## r.dm_member_only
{channel} is a dm: only its two members can post there (the management crew too; a dm takes no one else). To reach one of them, open your own dm with to and their number.

## r.post_to_human
The human has no dm: post in your home human channel {home} (channel = {home_id}), and use ask for a decision.

## r.post_to_human_no_channel
The human has no dm, and your home human channel does not exist yet: use ask for a decision (it posts into that human channel).

## r.human_not_watching_no_home
The human is not watching {channel}: the @human badge is lit but may be seen much later. Your home human channel does not exist yet; for urgent things or decisions use ask.

## r.home_human_absent
- (your home human channel does not exist yet: reach the human with ask; the first ask creates it)

## r.list_none
- (no channels you can read here. List again without crew to see your own, or look other crews up with directory)

## r.list_none_mine
- (no unarchived channels you can read. For archived ones: list_channels(include_archived=true); to reach someone look the number up with directory and post(to:); for a decision from the human use ask)

## r.channel_created
Created {name} ({id}); you are the owner, {count} members. Add people with channel_invite, post with post(channel="{id}").

## r.invited
Added to {channel}; it now has {count} members. They get its new messages from now on; to wake someone, mention them in a post.

## r.left
Left {channel}: its new messages are no longer injected for you. read_channel still shows it; to come back ask a member to channel_invite you.

## r.archived
Archived {channel}: messages kept, no new posts. Ask the human to unarchive it if needed.

## r.owner_offer_notice
I would like to hand ownership of {channel} to you: to take it, call channel_owner(channel="{id}").

## r.owner_offered
Asked {to} to take over {channel} and mentioned them there; it takes effect once they call channel_owner(channel="{id}"). Until then you stay the owner.

## r.owner_accepted
You now own {channel}: when the human posts there without naming anyone and the dispatcher cannot tell who should answer, you are woken.

## r.owner_forced
Ownership of {channel} moved to {to}. They are not woken by this: post and mention them if they need to know.

## r.listen_minutes
minutes must be 1–480; omit it to listen until channel_listen(channel=…, off=true).

## r.listening_for
Listening to {channel} for {minutes} minutes. Just end your turn and wait.

## r.listening
Listening to {channel} until channel_listen(channel="{id}", off=true). Just end your turn and wait.

## r.unlistened
Stopped listening to {channel}. To listen again: channel_listen(channel="{id}").

## r.unlisten_none
You were not listening to {channel}; nothing changed.

## r.dir_header
Directory (your number: {me}):

## r.dir_header_no_number
Directory (you have no extension; others can reach you by your session id {id}):

## r.dir_crew
- {number} crew {name}{path}

## r.dir_path
(under: {path})

## r.dir_session
- {number} {name} · {crew} · {id}

## r.dir_empty
No matching numbers: try another keyword, or omit query for the whole directory.

## r.owner_force_needs_to
force needs to (who the new owner is); without to the call accepts an ownership offered to you, and then takes no force.

## r.listen_off_alone
off=true only stops listening and takes no minutes or senders; to change the duration or the senders, call channel_listen again without off.

## r.owner_offer_notice_human
I would like to hand ownership of {channel} to you: to take it, set yourself as owner from this channel's menu in the app.

## r.fallback_pair
Give fallback_after_minutes (1 minute to 7 days) and a non-empty fallback together, or neither; fix them and call ask again.

## r.asked
Recorded as {ask} ("{question_head}") and posted to {channel} mentioning the human. Do not wait: do something else or end your turn; you will be woken with your resume_note when the human answers.

## r.ask_no_resume_note
You gave no resume_note: when woken you will only see the question and the answer. For a question asked mid-task, withdraw_ask it and ask again with a resume_note saying what to do after the answer.

## r.withdrawn
Withdrew {ask} in {crew}; a note is left in the human channel. If the matter still needs the human's decision, ask again.

## r.responded
Responded to Todo #{number} ("{todo_head}"). Its status is unchanged: respond_todo again on new progress, and state the result when done.

## r.quota
{report}
There is no absolute remaining amount; do not invent one. When quota is tight, wrap up and schedule_wakeup for after the reset.

## r.quota_none
No quota information is available right now. Keep working; if a limit stops you, schedule_wakeup to continue later.

## r.wakeup_note
note is required: on waking you only have this note and channel messages. Write what to do on waking and where the context is, then call again.

## r.wakeup_when
Give either after_minutes (1–1440) or at (an RFC 3339 time in the next 7 days carrying the offset you mean, like 2026-10-08T09:00:00+08:00; Z = UTC); fix it and call again.

## r.wakeup_set
You will be woken at {at} ({delta}); if that gap is not what you meant, the offset is probably wrong: schedule the right time too (this one cannot be cancelled; when it fires, judge by its note). If you are not running then, you will be started to continue. You can wrap up or do something else now.

## r.wakeup_in
in {hours}h {minutes}m

## r.wakeup_skipped
Your crew has been removed, so nothing was scheduled. Do not retry: schedule again after the human restores the crew.

## r.profile_empty
Give at least one of model and effort, then call again.

## r.profile_next_start
Saved: it takes effect the next time this session starts (restart or resume); this turn and your next tool calls still run on the current settings. Carry on with your work.

## r.not_your_crew
"{crew}" is not yours: you are not its captain. To get something done there, ask its captain: post(to: "{number}").

## r.brief_empty
brief is empty: write what it should do and what counts as done, then call again.

## r.session_started
Worker "{title}" started ({id}). It will speak up in the channel once running; to add details, post(to: "{id}"); if it stays quiet, inspect_session.

## r.stop_reason_empty
reason is empty: write one sentence on why (it is posted publicly to the lobby), then call again.

## r.stop_self
You cannot stop yourself. To finish, wrap up, say so in the channel, and end your turn.

## r.stop_notice
Stopping {target}: {reason}

## r.session_stopped
Stopped {target}; the reason is in the lobby of "{crew}". If its work must go on, start_session a new one.

## r.inspect
State of {target} below. If it is stuck on a box only a person can answer, ask the human; to give it other work, post(to: …) to it:
{report}

## r.nudge_keys_only
nudge_session only takes the keys esc or interrupt (interrupting its current turn), not "{key}". To tell it something, post(to: …) to it; it reads that when woken.

## r.nudged
Sent {key} to {target}. Sent, not confirmed: inspect_session in a few seconds to see the effect.

## r.sessions_header
Sessions of "{crew}" (id · number · name · role · state):

## r.session_line
- {id} · {address} · {name} · {role} · {state}

## r.sessions_none
(no sessions yet)

## r.sessions_tail
To see one's state, inspect_session; to give one work, post(to: its number).

## r.no_address
no number

## r.role_captain
captain

## r.role_worker
worker

## r.state_stopped
not running

## r.state_idle
idle

## r.state_working
working

## r.state_waiting
waiting for an answer

## r.state_unknown
state unknown

## r.handoff_one_of
Give exactly one of to_session and runner: to_session hands over to an existing session, runner starts a new one to take over (model / effort / title / opening_brief go with runner only). Fix it and call again.

## r.handoff_already
{target} is already the captain of "{crew}"; nothing was handed over. To change, give to_session another session, or start a new one with runner.

## r.handoff_done
Captain of "{crew}" handed to {new} ({id}). The old captain {old} stays in the crew as a worker. To brief the new captain, post(to: "{id}").

## r.handoff_done_vacant
Captain of "{crew}" handed to {new} ({id}); there was no captain before. To brief it, post(to: "{id}").

## r.handoff_self
You are a worker now: your tool list changes on your next call. Hand what you were coordinating to the new captain, then carry on with your own work.

## r.handoff_default_brief
You have just taken over as this crew's captain: read the lobby's recent messages, check what is in progress and how each worker is doing, then say in the lobby that you have taken over.

## r.default_captain_title
Captain

## r.stop_not_done
Not stopped: {detail}. The reason could not be posted to the lobby, so nothing was stopped; deal with the above, then call again.

## r.stop_failed_after_notice
The lobby already says "Stopping {target}", but the stop failed: {detail}. Nothing was stopped; inspect_session to see its state, then retry or correct the notice in the lobby.

## r.handoff_started_not_handed
Started {new} ({id}), but the captain role was not handed over: {detail}. It is running as a worker; do not start another one: retry with handoff_captain(to_session: "{id}"), or stop_session it if it is not wanted.

## r.not_your_crew_unnumbered
"{crew}" is not yours: you are not its captain. It has no number, so post(to:) cannot reach it; to get something done there, say so in your crew's lobby or ask the human.
