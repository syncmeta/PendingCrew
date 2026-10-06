<!--
Agent-facing tool text (spec §22.3). Everything before the first section heading is a comment (no line in it may start with "## ").
Section names: <tool> = tool description; <tool>.<param> = parameter text; r.* = receipts; trace.* = relay posts; inject.* = injection extras.
Placeholders {name} are filled by code (M3a's prompts::fill). zh and en must have exactly the same section names (tested).
-->

## post
Post key moments to a channel: started, done, blocked, handing off, important findings. Keep terminal detail (what you read or ran) out of channels.
Without channel: if reply_to is given the post goes to that message's channel, otherwise to your crew's lobby.
Mentions: @session / @captain narrow visibility — only the named agents see it in their injected context and get woken; add @broadcast to make it "all channel members see it, only the named wake"; @human does not narrow, it only flags the message for the human. @human in a channel the human is not watching lights the badge but may be seen much later — for anything urgent post in the root crew's human channel or use ask.
A message body is at most 256 KiB.

## post.channel
Channel id, or a channel name that is unique among channels you are in. See the tool text when omitted.

## post.message
Use for a single message. Mutually exclusive with messages.

## post.messages
Several messages at once (at most 6), one bubble each with its own category. If any item is invalid nothing is posted and the error names the item.

## post.mentions
Directed mentions: {kind:"session", target_id:"<session id>"} / {kind:"captain"} / {kind:"human"} / {kind:"broadcast"}. Omit for everyone in the channel. A mentioned session must be a member of the channel; @captain likewise requires the captain to be a member, and @captain in a crew without a captain is an error.

## post.reply_to
The message you are replying to: its number (#128 or 128) or its id. Same channel only. The original sender is mentioned automatically as [broadcast, sender]: all channel members still see it, the sender is woken; replying to your own message adds no mention; replying to a system message adds broadcast only.

## post.category
What this is: progress / done / blocked / question / finding / handoff / ack / note.


## read_channel
Read a channel page by page (oldest first). Reading up to the newest message marks the channel read for you, so those messages are not injected again — only for channels you are in or listen to (reading other channels does not move your read position). The #number before each message can go into reply_to.

## read_channel.channel
Channel id or name; omitted = your crew's lobby. Any channel of your crew is readable, membership not required.

## read_channel.limit
Number of messages, 1–200, default 50.

## read_channel.before
Only messages before this one (number or id). Omitted = newest page.

## search
Search messages in channels you are in or listening to. The whole trimmed query is matched as ONE case-insensitive substring (ASCII case folding only), not split into terms; newest first.

## search.query
The text to find: trimmed, then matched as one substring (ASCII case-insensitive).

## search.channel
Search only this channel (id or name).

## search.after
Only at or after this time (ISO8601).

## search.before
Only strictly before this time (ISO8601, exclusive: created_at < before).

## search.limit
Maximum results, 1–200, default 50.

## list_channels
List channels you are in (lobby, joined channels, channels you listen to) and the other channels of your crew: id, name, purpose, owner, members, whether the human is watching.

## human_watching
Where the human is now: channels marked "watching", plus the root crew's human channel. Post there when you need the human to see or decide; @human elsewhere may be seen much later.

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
Ask the human for a decision. Non-blocking: the question becomes a pending item (an H number), is posted to the root crew's human channel mentioning the human, and you get a receipt right away — go do something else or end your turn. When the human answers you are woken and resume_note is read back to you verbatim.

## ask.question
The decision you need. State the options and your preference — "A / B, I lean A because …".

## ask.resume_note
What you will do once answered (where you are, what is next). Always write it for questions asked mid-task.

## ask.fallback_after_minutes
Optional: if nobody answers within this many minutes, you are woken to act on fallback. Only for reversible things; production, money, privacy or credentials still hit the approval gate.

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
(Captain) Adopt a crew that has no parent/child relation with yours, making it your child. No cycles; the built-in chief crew cannot be adopted.

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

## r.not_on_builtin
{name} is not available in the chief crew.

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
