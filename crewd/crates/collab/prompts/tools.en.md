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
A dm with one session by its directory number: 7-3 (crew 7, extension 3), 7 (crew 7's current captain), 0 (the management crew's captain); look numbers up with directory. A pair has one dm, reused when it exists. Not together with channel or reply_to.

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
Create a channel (topic channel or two-person dm). Use it whenever you need to collaborate with someone on something or discuss a topic separately — do not create a child crew for that. You become the owner and a member. Give it a name and a purpose, e.g. "release", "scroll bug". A name equal to the human labels (人类 / Human) or to crewd's own system labels is rejected.

## channel_create.name
Short channel name.

## channel_create.purpose
One sentence: what the channel is for.

## channel_create.members
Members to add: session ids, directory numbers (like 7-3), or human.

## channel_create.kind
topic (default) or dm (exactly two members, at most one of them the human; if a dm for the same pair already exists it is reused, not created again).

## channel_invite
Add members to a channel. You must be a member yourself; the lobby and a dm cannot take invites.

## channel_invite.channel
Channel id or name.

## channel_invite.members
Session ids, directory numbers (like 7-3), or human.

## channel_leave
Leave a channel. You cannot leave the lobby; a dm cannot be left (archive it instead); the owner of a topic channel must transfer ownership first.

## channel_leave.channel
Channel id or name.

## channel_archive
Archive a channel (messages kept, no new posts; the human can unarchive). Only the owner or the captain can archive; the lobby and the human channel cannot be archived.

## channel_archive.channel
Channel id or name.

## channel_transfer
Hand channel ownership to someone else. The new owner must accept with channel_accept; the captain can force it. The owner acts as the channel's captain: when the human posts without naming anyone and the dispatcher cannot tell who should answer, the owner is woken.

## channel_transfer.channel
Channel id or name.

## channel_transfer.to
New owner: session id or directory number. Must be a member.

## channel_transfer.force
Captain only: true = transfer without waiting for acceptance.

## channel_accept
Accept ownership of a channel offered to you.

## channel_accept.channel
Channel id or name.

## channel_listen
Listen to a channel: new messages there wake you even without a mention. Use it while waiting for activity; end your turn normally while listening instead of polling read_channel.

## channel_listen.channel
Channel id or name.

## channel_listen.minutes
How long (minutes, 1–480). Omitted = until channel_unlisten.

## channel_listen.senders
Only these senders: human, captain, or session ids. This only filters whose messages wake you; everything visible in the channel is still injected. Omitted = everyone.

## channel_unlisten
Stop listening to a channel.

## channel_unlisten.channel
Channel id or name.

## directory
Look up the directory: every crew and session on this machine has a short number — crews are integers (7), members are extensions (7-3), and -1 is always that crew's captain. Each entry returns: number, kind (crew or member), name, crew / org path, session id; plus the channels you are in. The human has no number; use ask.

## directory.query
Filter by number prefix, name or keyword; omitted = everything.

## contact
Reach another crew or session by number. A crew number (7) posts to its lobby mentioning its captain, with your origin attached; an extension (7-3) opens a dm with that session (created if missing). Your own lobby gets a "contacted …" line. For your own crew just post.

## contact.to
Target number: 7 (crew) or 7-3 (session).

## contact.message
What you want to say: who you are and what you need.

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
Withdraw an unanswered question that no longer applies. You can withdraw your own; the captain can withdraw any in the crew. A question still blocking a work item cannot be withdrawn.

## withdraw_ask.ask
The H number, like H3.

## withdraw_ask.reason
Why it no longer applies: what changed.

## respond_todo
Respond to an item in the Agent Todo list (work the human assigned): claim, progress, result. Append-only; does not change status.

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
ISO8601 time in the future. Either this or after_minutes.

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
(Captain) Start a worker in this crew for a clear coding task.

## start_session.brief
The task, with goal and done criteria.

## start_session.title
At most 18 characters, no project name; the worker's display name.

## start_session.runner
claude or codex; omitted = crew default.

## start_session.isolation
Required. false = use the crew's shared directory; true = create a separate git worktree for it. Decide per call from parallel conflicts, change scope and how the tasks relate; if creation fails you get an error, never a silent fallback to the shared directory.

## start_session.model
Optional model.

## start_session.effort
Optional reasoning effort.

## stop_session
(Captain) Stop a session's process in this crew. The reason is posted to the lobby first; irreversible. To interrupt one turn, nudge_session with Esc instead.

## stop_session.session_id
Target session id.

## stop_session.reason
Why; posted publicly to the lobby.

## inspect_session
(Captain) See a session's state and recent terminal output (codex: transcript tail). Use it when a mention gets no answer.

## inspect_session.session_id
Target session id.

## nudge_session
(Captain) Send keys or text to a session's terminal: Enter / Esc send that key, anything else is submitted as input. Inspect first.

## nudge_session.session_id
Target session id.

## nudge_session.input
Enter, Esc, or text to send.

## list_sessions
(Captain) List this crew's sessions: id, number, name, role, state.

## handoff_captain_to_session
(Captain) Hand the captain role to an existing session. target_crew_id omitted = this crew; otherwise only a direct child crew.

## handoff_captain_to_session.session_id
The successor's session id; must belong to the target crew.

## handoff_captain_to_session.target_crew_id
Optional: a direct child crew id.

## create_and_handoff_captain
(Captain) Start a new session and hand it the captain role. target_crew_id omitted = this crew; otherwise only a direct child crew.

## create_and_handoff_captain.runner
claude or codex; must be explicit.

## create_and_handoff_captain.model
Optional model.

## create_and_handoff_captain.effort
Optional reasoning effort.

## create_and_handoff_captain.title
Optional display name; default "Captain".

## create_and_handoff_captain.opening_brief
Optional first task for the new captain.

## create_and_handoff_captain.target_crew_id
Optional: a direct child crew id.

## report_to_parent
(Captain) Report upward: posts to each direct parent crew's lobby mentioning its captain. For milestones, decisions or resources you need from above, or being blocked. Lead with the conclusion.

## report_to_parent.message
The report.

## message_child_crew
(Captain) Send a message to a direct child crew: posted to its lobby mentioning its captain.

## message_child_crew.crew
Child crew name, id or number.

## message_child_crew.message
What to pass on.

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

## r.not_on_management
{name} is not available in the management crew.

## r.bad_args
Invalid arguments: {detail}

## trace.worker
{label} stopped this turn on a question and is waiting for an answer:
{question}
Captain: inspect_session to see the state, nudge_session to answer for it; if you cannot decide, ask the human. (session: {session})

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
