<!--
Agent-facing tool text (spec §22.3). Everything before the first section heading is a comment (no line in it may start with "## ").
Section names: <tool> = tool description; <tool>.<param> = parameter text; r.* = receipts; trace.* = relay posts; inject.* = injection extras.
Placeholders {name} are filled by code (M3a's prompts::fill). zh and en must have exactly the same section names (tested).
-->

## instructions
Crew tools are how you talk to your crew and the human.
Post key moments with post; use ask when you need a human decision, then keep working without blocking.

## post
Post key moments to a channel: started, done, blocked, handing off, important findings. Write the conclusion in one sentence plus the one thing you need decided; long tables and lists go into a file, post the path. Keep terminal detail (what you read or ran) out of channels.
Say where it goes; there is no default: channel = post in a channel; to = a dm with one session by its directory number (also across crews); reply_to alone = the channel of that message. With none of the three you get an error listing your channels.
@ only wakes people. Without an @, nobody is woken (except sessions listening to the channel). In a dm, the other side is always woken. @session / @captain wake the named agents; @human flags the message for the human. Use @human only to ask a question (use ask when the human needs to decide), never to report progress. Whoever you @, everyone who can read the channel sees the message. @human in a channel the human is not watching may be seen much later: for anything urgent post in your home (top-level crew) human channel, and use ask for a decision.
Posting too fast, repeating the same text, or @human too often is held back (the receipt says what to do): merge your updates into one message.
The receipt predicts whom the post will wake under current membership and listens; prediction does not mean it was queued, delivered or started working. A message body is at most 256 KiB.

## post.channel
Channel id, or a channel name that is unique among the channels you can read. Not together with to.

## post.to
A dm with one session by its directory number: 7-3 (crew 7, extension 3), 7 (crew 7's current captain), 0 (the management crew's captain); look numbers up with directory. A pair has one dm, reused when it exists. The human has no dm (to: "human" points you to your home human channel): reach the human there, and use ask for a decision. Not together with channel or reply_to.

## post.message
Use for a single message. Mutually exclusive with messages.

## post.messages
Several messages at once (at most 6), one bubble each with its own category. If any item is invalid nothing is posted and the error names the item. mentions and reply_to apply to the first message only.

## post.mentions
Who to wake: {kind:"session", target_id:"<session id or directory number>"} / {kind:"captain"} / {kind:"human"}. Without an @, nobody is woken (except sessions listening to the channel). In a dm, the other side is still woken. Everyone who can read the channel still sees it. A mentioned session must be a member of the channel; @captain likewise requires the captain to be a member, and @captain in a crew without a captain is an error.

## post.reply_to
The message you are replying to: its number (#128 or 128) or its id. Same channel only. The original sender is mentioned automatically (while still in the channel): all channel members still see it, the sender is woken; replying to your own message or to a system message adds no mention.

## post.category
What this is: progress / done / blocked / question / finding / handoff / ack / note.


## read_channel
Read a channel page by page (oldest first). Use it for older messages or for channels you are not in. Reading up to the newest message marks the channel read for you, so those messages are not injected again — only for channels you are in or listen to (reading other channels does not move your read position). The #number before each message can go into reply_to.

## read_channel.channel
Channel id or name; omitted = your crew's lobby. Readable: your crew's channels (dms between its members included — there are no private channels), your home human channel, channels you are in or listen to; management crew sessions read the whole organisation.

## read_channel.limit
Number of messages, 1–200, default 50.

## read_channel.before
Only messages before this one (number or id). Omitted = newest page.

## search
Search organization objects and currently readable messages, grouped by kind and individual table with references. Hosts supporting unified search default to in=all; older hosts retain message search. Objects keep their existing read policy, while H and messages recheck their original scope, B15 and current listens. Match the trimmed query as one literal substring with ASCII case folding, without FTS operators. Current/open items come first, then recent changes. The initial search fixes the ordering scope; new messages do not interrupt pagination, while content and permissions remain current. indexState=building means indexing is incomplete, not that nothing matches. Expand one hop with organization_search_expand; do not generate a model summary or wake anyone.

## search.query
The text to find: trimmed, then matched as one substring (ASCII case-insensitive).

## search.channel
Search messages in this channel only (id or name). Conflicting object filters are refused, never widened.

## search.crew
Search objects owned by this crew and its readable channel messages (crew number, id or name). A shared artifact without an owner is not assigned to a crew based on a reference.

## search.after
Only at or after this time (ISO8601, inclusive): creation time for messages, actual change time within the ordering scope for objects.

## search.before
Only strictly before this time (ISO8601, exclusive): creation time for messages, actual change time within the ordering scope for objects.

## search.limit
Maximum results, 1–200, default 50.

## list_channels
List channels, to find a channel id or see which channels the human is watching: first your home human channel (post there what the human must see), then channels you are in or listen to, the other channels of your crew, and channels the human is watching. Each row: id · name · type · purpose · owner · members · state (member / listening / human watching / archived / you own it).

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
Members to add right away (optional; channel_invite later works too): session ids, directory numbers (like 7-3; 7 = crew 7's current captain), or human. Leave yourself out.

## channel_invite
Add members to a channel, when others should follow or discuss the matter too. Usually you must be a member yourself; the lobby and a dm cannot take invites.

## channel_invite.channel
Channel id or name.

## channel_invite.members
Session ids, directory numbers (like 7-3), or human.

## channel_leave
Leave a channel, when its matter no longer concerns you. You cannot leave the lobby; a dm cannot be left (archive it instead); the owner of a topic channel must hand ownership on with channel_owner first.

## channel_leave.channel
Channel id or name.

## channel_archive
Archive a channel when its matter is done (messages kept, no new posts; the human can unarchive). Only the owner, the captain of the channel's crew or the human can archive; the lobby and the human channel cannot be archived.

## channel_archive.channel
Channel id or name.

## channel_owner
Channel ownership. The owner acts as the channel's captain: when the human posts without naming anyone, the owner is usually the one woken and takes it — answers, or hands it to the right person. Three uses: with to = the owner offers it (you mention them in the channel under your own name; it takes effect once they call channel_owner(channel=…)); without to = accept an ownership offered to you; with to and force=true = the captain of the channel's crew (the management captain counts) moves it at once, without acceptance. The new owner must be a member.

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
Only these senders: human; captain (whoever holds that role when posting, so this follows captain changes); or a session id or directory number (resolved once—for example, 7-1 resolves to crew 7's current captain and does not follow later changes). This affects only whose messages wake you; all messages in the channel are still injected. Omitted = everyone.

## channel_listen.off
true = stop listening to this channel (not together with minutes or senders). Not listening already is not an error.

## directory
Use the directory to find someone or talk across crews. Every crew and session on this machine has a short number: crews are integers (7), members are extensions (7-3), 7-1 is crew 7's current captain, and 0 is the management crew (0 = its current captain). Each row: number, name, crew or parent path, session id. Numbers go straight into post's to (a dm, across crews too) and into member arguments of the tools. Channels are listed by list_channels; the human has no number: post what the human must see in your home human channel, and use ask for a decision.

## directory.query
Filter by number prefix, name or org path; omitted = everything.

## ask
Use it when the human has to decide: ask the human a question (workers too — not a post in the human channel). Non-blocking: the question becomes a pending item (an H number), is posted to your home (top-level crew; the management crew is its own home) human channel mentioning the human, and you get a receipt right away — go do something else or end your turn. When the human answers you are woken and resume_note is read back to you verbatim.

## ask.question
The decision you need. State the options and your preference — "A / B, I lean A because …".

## ask.resume_note
What you will do once answered (where you are, what is next). Always write it for questions asked mid-task.

## ask.fallback_after_minutes
Optional: if nobody answers within this many minutes, you are woken to act on fallback. Only for reversible things.

## ask.fallback
What you will do if nobody answers in time ("go with A"). Give together with fallback_after_minutes.

## ask_decision
Ask the Human to select one option for your current crew. A selected answer or applied default creates an organization-readable decision. Include only question, context, options, and links that the organization may read. Use ask for a private or free-text question. Retain the original request UUID and scope if the result is uncertain.

## ask_decision.scope
Copy the current dbId and restoreEpoch exactly from managed_object_scope_get.

## ask_decision.requestId
Generate one canonical UUID for this exact request and retain it for lookup.

## ask_decision.draft
Provide the question, the background, two to eight different single-line options in order, and the exact related object IDs. Option IDs run from A in order. If specifying a default, give one of those IDs and the delay in whole minutes.

## ask_decision_request_get
Look up the original structured question opening receipt for this session. The result records the original opening. Check the Human Ask or exact decision separately for the current outcome. A missing or historical receipt does not authorize another execution.

## ask_decision_request_get.scope
The original dbId and restoreEpoch from the opening request.

## ask_decision_request_get.requestId
The exact original request UUID.

## withdraw_ask
Withdraw an unanswered question that no longer applies. You can withdraw your own; the captain can withdraw any in the crew (the management crew's captain counts as the captain of every crew). A question still blocking a work item cannot be withdrawn.

## withdraw_ask.ask
The H number, like H3.

## withdraw_ask.reason
Why it no longer applies: what changed.

## withdraw_ask.crew
Whose book holds the question (crew number or id); omitted = your own crew.

## respond_todo
Respond to an item in your crew's Agent Todo list (work the human gave the crew): when you claim it, when there is progress, and when it is done. Append-only; does not change status.

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
Give the crew a short label like "auth refactor". Do it once you know what the crew is doing. crew omitted = this crew; another crew only if you act as its captain.

## rename_crew.name
A few-word label, no punctuation.

## rename_crew.crew
Optional: which crew (number or id); omitted = this crew.

## change_workdir
Change crews' working directory. Call without confirm for a preview, then with confirm: true. You may change a crew you act as captain of (the management crew's captain: any crew) and the crews below yours. Removed crews are left alone.

## change_workdir.new_path
Absolute path of an existing directory.

## change_workdir.crew
Optional: which crew (number or id); omitted = this crew.

## change_workdir.include_children
Also change child crews; default true.

## change_workdir.confirm
true = apply; omitted = preview only.

## set_crew_status
One sentence on how the whole crew is doing, shown on the crew's sidebar row; update it when things change. Over 200 characters is refused; an empty string clears it. crew omitted = this crew.

## set_crew_status.text
One-sentence status.

## set_crew_status.crew
Optional: which crew (number or id); omitted = this crew.

## delegation_policy
The crew's delegation policy: read it before handing out work; replace it when the policy changes. Without text = read it; with text = replace it with that text. crew omitted = this crew.

## delegation_policy.text
Optional: the full new text (replaces the old one); omitted = read only.

## delegation_policy.crew
Optional: which crew (number or id); omitted = this crew.

## start_session
Start a worker for a clear coding task. crew omitted = this crew; another crew only if you act as its captain (the management crew's captain counts as the captain of every crew).

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
Stop a session's process: the reason is posted in your name to its crew's lobby first, then it is stopped; irreversible. To interrupt one turn only, nudge_session with esc. You cannot stop yourself.

## stop_session.session_id
Target session: id or directory number (like 7-3).

## stop_session.reason
Why; posted publicly to the lobby.

## inspect_session
See a session's state and recent terminal output (codex: transcript tail). Use it when a mention gets no answer.

## inspect_session.session_id
Target session: id or directory number (like 7-3).

## nudge_session
Interrupt a session's current turn: only the keys esc / interrupt. To tell it something, post(to: …) to it; it reads that when woken. Sending is not confirmation: inspect_session a few seconds later to see the effect.

## nudge_session.session_id
Target session: id or directory number (like 7-3).

## nudge_session.key
esc or interrupt; both interrupt its current turn.

## list_sessions
List a crew's sessions (id, number, name, role, state), before handing out work or to find a session. crew omitted = this crew.

## list_sessions.crew
Optional: crew number or id; omitted = this crew.

## handoff_captain
Hand over a crew's captain role: to_session hands it to an existing session (which must belong to that crew), or runner starts a new session to take over; give exactly one. crew omitted = this crew; it may also be a direct child crew of yours, or any crew you act as captain of. The old captain stays in the crew as a worker.

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

## crew_structure
Change where crews sit in the organisation tree. action is one of five: create_child (a new child crew under crew: it inherits the working directory and gets its own captain started, whose first task is brief); create_parent (a new parent crew above crew, with its own captain started); adopt (put target under crew); adopt_parent (put crew under target); release (detach crew's direct child target, optionally moving it under to; with no other parent left it becomes top-level). New or adopted crews go at the end of their siblings. Removing, restoring, purging and ordering crews are the human's and are not here. Child crews only express organisational structure — a lasting department or team with its own captain. To collaborate on a topic or a task, create a channel with channel_create instead. crew omitted = this crew; another crew only if you act as its captain.

## crew_structure.action
One of create_child / create_parent / adopt / adopt_parent / release.

## crew_structure.crew
Optional: the crew acted on (number or id); omitted = this crew.

## crew_structure.target
Required for adopt / adopt_parent / release: the other crew (number or id). The target of adopt / adopt_parent and the to of release must be a crew you act as captain of, or a direct child of one; moving crews across other branches of the tree is an organisation-level change: ask the management crew's captain (post(to: "0")) or the human.

## crew_structure.to
Optional, release only: where to move target (the same reach as target: a crew you act as captain of, or a direct child of one).

## crew_structure.title
Optional, for create_child / create_parent: the new crew's short label; omitted = from a template.

## crew_structure.brief
Required for create_child: the child crew captain's first task, with goal and done criteria.

## crew_structure.runner
Optional, create_child only: claude or codex for the child crew's captain; omitted = default.

## r.unknown_tool
There is no tool named {name}. {hint}Check your tool list and use one that does the job.

## r.tool_removed
The tool {name} is gone: {replacement} does its job now. Call that instead.

## r.captain_only
{name} is for the captain only, and you are not the captain now. To get it done, ask your captain in the lobby with @captain.

## r.bad_args
Invalid arguments: {detail}
Fix them per the tool's description and call again.

## trace.worker
(auto-relayed) {label} stopped this turn on a question and is waiting for an answer:
{question}
{reply}

## trace.captain
(auto-relayed) {label} of {crew} stopped this turn on a question and is waiting for your answer:
{question}
{reply}

## trace.crew
{title} (crew {number})

## trace.reply
Reply to this message to answer it.

## trace.reply_elsewhere
It is not in this channel, so a reply here does not wake it: send it a dm (session {session}) or answer in its terminal.

## r.failed
Did not go through: {detail}
Deal with the cause and call again; if it is unclear what to do, say so in the lobby.

## r.no_handler
{name} is not implemented yet. Do it another way, or say so in the lobby.

## r.internal
Internal error #{n} (details are in the crewd log). Try again; if it keeps failing, say so in the lobby with this number.

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
Item {index} has an unknown category "{value}"; choose from: {choices}. Fix it and post again.

## r.bad_mention
Invalid mention (kind = {kind}): kind must be session / captain / human, and session needs target_id (a session id or directory number). Fix it and post again.

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
Planned wake targets: {names}. This is not delivery confirmation.

## r.posted_wakes_none
Under current membership and listens, no agent will be woken. @ whoever needs to respond.

## r.posted_wakes_unavailable
The messages were committed, but wake prediction is unavailable ({detail}). Use read_channel on {channel} to check the posted content; do not resend these messages.

## r.posted_wakes_internal
internal error #{n}; details are in the crewd log

## r.posted_not_member
You are not a member of {channel}: replies will not wake you; channel_listen it to hear them.

## r.post_throttled
Posting too fast, so this was not posted: merge your update into one message and post it again in {retry_secs} s.

## r.post_duplicate
Same text as your #{seq}, so it was not posted again. Write what is new, or reply_to #{seq}.

## r.human_mention_throttled
Too many @human from you in the last 30 minutes, so this was not posted. For a decision use ask; otherwise post without @human.

## r.human_not_watching
The human is not watching {channel}: the @human badge is lit but may be seen much later. For urgent things post in your home human channel {home} ({home_id}, the first line of list_channels); for decisions use ask.

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
{channel} is a dm: only its two members can post there (the management crew too; a dm takes no one else). To reach one of them, open your own dm with to and that member's number.

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
Added to {channel}; it now has {count} members. New members get its new messages from now on; to wake someone, mention them in a post.

## r.left
Left {channel}: its new messages are no longer injected for you. read_channel still shows it; to come back ask a member to channel_invite you.

## r.archived
Archived {channel}: messages kept, no new posts. Ask the human to unarchive it if needed.

## r.owner_offer_notice
I would like to hand ownership of {channel} to you: to take it, call channel_owner(channel="{id}").

## r.owner_offered
Asked {to} to take over {channel} and mentioned them there; it takes effect once they call channel_owner(channel="{id}"). Until then you stay the owner.

## r.owner_accepted
You now own {channel}: when the human posts there without naming anyone, you are usually the one woken. Take it: answer, or hand it to the right person.

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
Started {new} ({id}), but the captain role was not handed over: {detail}. It is running as a worker; do not start another one: retry with handoff_captain(crew: {crew_arg}, to_session: {session_arg}). {cleanup}

## r.handoff_cleanup_self
If it is not wanted, stop_session(session_id: {session_arg}, reason: "No longer needed").

## r.handoff_cleanup_authority
To stop session {session_arg}, ask its crew captain or the management captain, or use ask for the human to arrange cleanup. Your child-crew handoff reach does not let you stop its sessions.

## r.not_your_crew_unnumbered
"{crew}" is not yours: you are not its captain. It has no number, so post(to:) cannot reach it; to get something done there, say so in your crew's lobby or ask the human.
## r.renamed
"{old}" is now called "{name}" in the sidebar and the directory. Carry on with your work.

## r.status_too_long
The status has {count} characters, over the {max} limit: cut it to one sentence and call again.

## r.status_set
Status of "{crew}" updated: {text}
Update it again when things change.

## r.status_cleared
Status of "{crew}" cleared. Write a new one with set_crew_status when there is news.

## r.policy
Delegation policy of "{crew}":
{text}
To change it, delegation_policy(text: the full new text).

## r.policy_empty
"{crew}" has no delegation policy yet. To set one, delegation_policy(text: the full text).

## r.policy_blank
text is empty. To read, leave text out; to change, give the full new text and call again.

## r.policy_updated
The delegation policy of "{crew}" was replaced. To check it, call delegation_policy() to read it.

## r.workdir_missing
{path} is not the absolute path of an existing directory: make sure it exists, use an absolute path and call again.

## r.workdir_none
(none)

## r.workdir_line
- "{crew}" (before: {old})

## r.workdir_preview
Preview (nothing changed yet): the working directory of these {count} crews would become {path}:
{crews}
If that is right, call again with confirm: true and the same other arguments.

## r.workdir_done
The working directory of these {count} crews is now {path}:
{crews}
Sessions started from now on work there; running processes keep their directory: stop and start one again to move it.

## r.structure_args
crew_structure(action: "{action}") takes only these besides crew: {params} (? = optional); a missing required one or any other argument is refused. Fix the call and try again.

## r.default_child_title
{parent} · child

## r.default_parent_title
{child} · parent

## r.child_brief_notice
{by} created this crew. Its captain's first task:
{brief}

## r.child_created
Child crew "{crew}" (number {number}) created under "{parent}", at the end of its children; its captain {captain} ({id}) is started and the brief is in its lobby. To tell it anything later, post(to: "{number}").

## r.child_created_no_captain
Child crew "{crew}" (number {number}) was created, but its captain did not start: {detail}. Do not create it again: give it a captain with handoff_captain(crew: "{number}", runner: …).

## r.parent_notice
{by} created this crew as the parent of "{child}".

## r.parent_brief
You are the captain of a newly created parent crew; "{child}" is your first child crew. Contact its captain with post(to: {child_arg}, message: "Share your current goals and progress.", mentions: {mentions_arg}); get the context from them, then decide whether to adopt other crews. Being its parent does not grant access to its lobby.

## r.parent_brief_contact
You are the captain of a newly created parent crew; "{child}" is your first child crew, but has no directory number. Contact its creation-time captain session with post(to: {child_arg}, message: "Share the child goals and progress you know.", mentions: {mentions_arg}) to get context. This is a fixed session contact, not a claim that it is still captain; if contact fails, use ask for human context and keep working on what does not need the answer. Being its parent does not grant access to the child lobby.

## r.parent_brief_no_captain
You are the captain of a newly created parent crew; "{child}" is your first child crew, but has no captain. Use ask(question: "How should I obtain goals and progress for my child crew without a captain?") to get a human decision or context; keep working on what does not need the answer. Being its parent does not grant access to its lobby.

## r.parent_created
Parent crew "{crew}" (number {number}) created above "{child}"; it is top-level, at the end of the top level, and its captain {captain} ({id}) is started. To tell it anything later, post(to: "{number}").

## r.parent_created_no_captain
Parent crew "{crew}" (number {number}) was created with "{child}" under it, but its captain did not start: {detail}. Do not create it again. It is a level above your reach: ask the management crew's captain (post(to: "0")) to give it a captain with handoff_captain(crew: "{number}", runner: …), or ask the human to set one up in the app; if you are the management crew's captain, do that yourself.

## r.no_longer_top
"{crew}" is no longer top-level: its human channel is archived (messages kept), and its asks now go to the human channel of "{home}". Keep using ask as before.

## r.no_longer_top_unnamed
"{crew}" is no longer top-level: its human channel is archived (messages kept), and its asks now go to the human channel of its new top-level parent. Keep using ask as before.

## r.adopted
"{child}" is now under "{parent}", at the end of its children. To reach its captain, look up its number with directory and post(to: number).

## r.already_child
"{child}" was already under "{parent}"; nothing changed. Carry on with your work.

## r.released_top
"{child}" was detached from "{from}" and is now top-level, at the end of the top level; its asks go to its own human channel from now on. Carry on with your work.

## r.released
"{child}" was detached from "{from}"; it is still under another parent. Carry on with your work.

## r.released_same_parent
"{child}" stays under "{from}"; the source and destination are the same. Nothing changed, including its position. Carry on with your work.

## r.released_existing_parent
"{child}" was detached from "{from}". It was already under "{to}" and stays there in the same position; no destination edge was added or reordered. Carry on with your work.

## r.released_to
"{child}" was moved from "{from}" to "{to}", at the end of its children. Carry on with your work.

## r.helper_build_skew
The helper and crewd daemon were built from different commits; this call may continue. Notify the human to choose an update, hibernate and restart time; do not restart automatically.

## my_work_items
Read concise summaries of work you own across the organization; unfinished only by default. Use the returned id/revision to update progress. Fetch long goals and acceptance criteria with work_item_get only when needed.

## my_work_items.after
The previous page's nextAfter global plan ID; omitted starts the first page.

## my_work_items.limit
Page size 1–200, default 50.

## my_work_items.include_finished
true also includes done/dropped plans; default false.

## work_item_get
Read details by global plan ID. Organization work is readable, including removed crews. Reading does not wake anyone.

## work_item_get.id
Global W ID returned by a work tool, not a legacy crew-local alias.

## work_item_create
Register real planned work, owned by you by default. Set the actual goal, completion level and acceptance criteria. Parent/dependency links currently require the same crew. State, events, identity and progress commit together. Retain the returned id/revision; do not duplicate a plan to retry an unknown outcome.

## work_item_create.title
Clearly state what this item will accomplish.

## work_item_create.goal
The actual goal; detail is loaded on demand.

## work_item_create.level
Completion level local/public/production, default local.

## work_item_create.acceptance
How to judge that completion level was achieved.

## work_item_create.owner
Owner session ID, defaults to you. Assigning someone else requires captain authority.

## work_item_create.crew
Crew number, ID or unique name, defaults to your crew. Target crew authority is checked independently.

## work_item_create.parent
The same crew parent's global W ID.

## work_item_create.depends_on
Global W IDs of dependencies in the same crew.

## work_item_create.sources
Global # IDs of registered original requirements.

## work_item_create.next_check_at
RFC3339 check time; active work must have a valid next check.

## work_item_update
Report progress or completion using the revision you just read; stale revisions cannot overwrite new work. Progress requires a next-step note and active requires a valid check time. The owner marks done; evidence is optional, with no extra verification gate. On conflict read the latest summary/detail and reassess rather than blindly looping.

## work_item_update.id
The global W plan ID.

## work_item_update.expected_revision
The revision string from a summary or the previous operation, passed unchanged.

## work_item_update.status
open/active/done/dropped; omitted updates only the note/check time. Dropped is a business decision and requires a nonblank reason in note; it cannot abandon work that still has live dependents. Blocking and transfer are separate actions.

## work_item_update.next_check_at
RFC3339 check time, later than now and within work limits; not accepted for done or dropped.

## work_item_update.note
Overwrite this item's next-step/progress note; required and nonblank except for completion.

## work_item_update.evidence
Optional completion evidence: {kind:"commit"|"url"|"text", value:"nonblank text"}; only for done.

## r.work_result
{result}

## r.work_coordination_assignment
Original recorded assignment for this coordination request:
{result}

## r.work_coordination_split
Original recorded split for this coordination request.
{result}

## my_work_items.include_removed
true also reads work in removed crews, default false. Objects with frozen:true are read-only; do not repeatedly try to advance them.

## work_item_assign
Assign an owner or claim unowned work. Workers may only claim an unowned item for themselves; captains may assign only in crews they manage. Requires the current revision and a reason; success durably notifies the new owner. Assignment by a captain or human does not fabricate the recipient's consent.

## work_item_assign.id
Global W ID.

## work_item_assign.expected_revision
Observed current revision as a string; review returned facts on conflict.

## work_item_assign.owner
New owner's actual session ID, not a role or human identity.

## work_item_assign.note
Nonempty assignment or claim reason.

## work_item_assign.coordination_decision_id
Current captain only. Use the delivered coordination request ID from the same crew to link the actual owner change to that request. Review the original work and current revision, and ensure no transfer awaits acceptance. An exact repeat returns the original receipt.

## work_item_coordination_split
Create two to eight open children of the original work in response to a delivered coordination request. The current captain keeps the parent's fields and direct requirement links intact. Choose this action or a linked owner change once per request. An exact repeat returns the original child creation receipt.

## work_item_coordination_split.id
Global W ID of the original parent work.

## work_item_coordination_split.expected_revision
Observed current revision of the original parent as a string.

## work_item_coordination_split.coordination_decision_id
Exact request ID from the delivered coordination notice.

## work_item_coordination_split.children
Two to eight scoped children with title, goal, acceptance and an explicit owner. Use a same-crew session ID or null for intentionally unowned work. List any prerequisites as existing work IDs in the same crew.

## work_item_reassign_lost
Reassign only after the daemon verifies the current owner cannot recover, or real delivered checkpoints exceed the existing silence threshold and are due. A caller cannot declare loss. Healthy, merely stopped or unknown owners refuse; ordinary workers cannot take another owner's work. The current revision and actual target session are required. Success records lost-owner history and a guarded compact notification; human stopping still applies.

## work_item_reassign_lost.id
Global W ID.

## work_item_reassign_lost.expected_revision
Observed current revision as a string; review returned facts on conflict.

## work_item_reassign_lost.owner
New owner's actual session ID, not a role or human identity.

## work_item_reassign_lost.note
Nonempty assignment or claim reason.

## work_item_history
Read original handoff history only when needed; latest changes first. Full reasons are returned, without goal or acceptance text. This read does not wake anyone or change progress. The recorded actor role describes that historical action, not current authority.

## work_item_history.id
Global W ID.

## work_item_history.before
Use nextBefore from the previous page to read older entries; omit for latest. Sequence cursor is a positive canonical string.

## work_item_history.limit
Maximum entries, 1 to 200, default 20.

## work_item_block

Atomically block work on W items, existing H questions, or new human questions. State explicitly whether anything can continue; split work stays under the original item and inherits its owner unless specified. Uses the current revision and actual object permissions.

## work_item_block.id

Public W identity.

## work_item_block.expected_revision

Current revision as a decimal string; conflict returns current facts.

## work_item_block.on

One to sixteen work/human/new_human blockers. New defaults use text and after_minutes.

## work_item_block.can_continue

Required: type nothing, or type split with one to twenty work_item_create items; parent cannot be set.

## todo_get

Read one original requirement by its issued public # ID. Returns the unchanged human text and current derived lifecycle. Fetch only the relevant requirement rather than scanning all Todo rows.

## todo_get.id

Issued requirement ID, for example #7.

## todo_ruling_check
Review why the direct work for a requirement was dropped. Only the current captain of the requirement's crew may use this tool. Read every page at the same revision. The final page provides closeRevision for an explicit decision to end the requirement.

## todo_ruling_check.id
Original public requirement # ID.

## todo_ruling_check.after
Copy nextAfter from the preceding page of the same review.

## todo_ruling_check.expected_revision
Copy revision from the first page when continuing the same review.

## todo_ruling_close
Record the captain's explicit decision to end a requirement after all direct work was dropped. Use the final page's closeRevision and the original request ID. The requirement and work history remain available.

## todo_ruling_close.id
Original public requirement # ID.

## todo_ruling_close.expected_revision
Use closeRevision from the final page of this requirement's current review.

## todo_ruling_close.request_id
Create a canonical UUID for this decision and retain it for an exact retry.

## todo_ruling_close.note
Explain why work on this requirement is ending, in up to 4096 UTF-8 bytes.

## work_item_transfer_request

Offer this work to another session; only its current owner can request. Ownership changes only after the target accepts.

## work_item_transfer_request.id

Issued public W ID.

## work_item_transfer_request.expected_revision

Exact current revision string.

## work_item_transfer_request.to

Actual target session ID.

## work_item_transfer_request.note

Nonblank handoff reason, retained in full detail and ownership history; receipts carry only the offer reference.

## work_item_transfer_accept

Accept a current transfer offered to you. Refresh the work revision if ordinary progress changed it.

## work_item_transfer_accept.id

Issued public W ID.

## work_item_transfer_accept.expected_revision

Exact current revision string.

## work_item_transfer_withdraw

Withdraw a current offer as its owner, target or actual work captain.

## work_item_transfer_withdraw.id

Issued public W ID.

## work_item_transfer_withdraw.expected_revision

Exact current revision string.

## r.resource_result
{result}

## document_create
Create shared crew document with immutable UUID request witness; retries return current metadata without repeating body or waking models.

## document_create.request_id
Stable original creation request UUID; uncertain retries preserve exact original payload.

## document_create.crew_id
Actual crew ID.

## document_create.title
Nonblank title, at most512 UTF-8 bytes.

## document_create.body
Exact document body, at most1MiB UTF-8 bytes.

## document_get
Read an explicit bounded UTF-8 byte chunk. Subsequent chunks require the observed revision; retained deleted history is readable.

## document_get.id
Stable object UUID.

## document_get.offset
UTF-8 byte offset at a character boundary, default0.

## document_get.limit
Bounded page or body-chunk size within schema limits.

## document_get.expected_revision
Observed current document revision, never guessed.

## document_list
List bounded metadata, never automatically include bodies.

## document_list.crew_id
Actual crew ID.

## document_list.after
Opaque cursor from this exact query scope and revision.

## document_list.limit
Bounded page or body-chunk size within schema limits.

## document_list.include_deleted
Explicitly include soft-deleted metadata, defaultfalse.

## document_update
Replace exact-revision title/body; use document_patch for small edits to avoid repeating full content.

## document_update.id
Stable object UUID.

## document_update.expected_revision
Observed current document revision, never guessed.

## document_update.title
Nonblank title, at most512 UTF-8 bytes.

## document_update.body
Exact document body, at most1MiB UTF-8 bytes.

## document_delete
Soft delete at observed revision, retaining body/audit without automatic wake.

## document_delete.id
Stable object UUID.

## document_delete.expected_revision
Observed current document revision, never guessed.

## document_patch
Patch exact UTF-8 byte spans of the observed revision: at most20 nonoverlapping edits and16KiB combined new text; returns metadata only.

## document_patch.id
Stable object UUID.

## document_patch.expected_revision
Observed current document revision, never guessed.

## document_patch.edits
Exact byte spans {start,end,text}; end exclusive, no overlap.

## view_folder_list
Read one bounded page. Cursor binds crew/view/part/revision; expired pages conflict without silently reloading all.

## view_folder_list.crew_id
Actual crew ID.

## view_folder_list.view
messages/todo/documents/progress; independent from work graph parentage.

## view_folder_list.part
folders or assignments, defaultfolders.

## view_folder_list.after
Opaque cursor from this exact query scope and revision.

## view_folder_list.limit
Bounded page or body-chunk size within schema limits.

## view_folder_create
Create shared classification folder using immutable request UUID; independent of work lifecycle/parentage. Compact receipt only.

## view_folder_create.crew_id
Actual crew ID.

## view_folder_create.view
messages/todo/documents/progress; independent from work graph parentage.

## view_folder_create.request_id
Stable original creation request UUID; uncertain retries preserve exact original payload.

## view_folder_create.expected_tree_revision
Observed whole crew/view revision; review finite conflict facts.

## view_folder_create.name
Shared manual name: trimmed nonblank, at most256 UTF-8 bytes.

## view_folder_create.parent_id
Same-scope parent UUID; omit for root.

## view_folder_update
Rename/move/reorder within one crew/view tree under whole-scope revision CAS.

## view_folder_update.crew_id
Actual crew ID.

## view_folder_update.view
messages/todo/documents/progress; independent from work graph parentage.

## view_folder_update.id
Stable object UUID.

## view_folder_update.expected_tree_revision
Observed whole crew/view revision; review finite conflict facts.

## view_folder_update.name
Shared manual name: trimmed nonblank, at most256 UTF-8 bytes.

## view_folder_update.parent_id
Same-scope parent UUID; omit for root.

## view_folder_update.position
Zero-based sibling position after removing the moved folder.

## view_folder_delete
Promote child folders to this parent and unclassify direct contents. Never delete underlying work/document/messages or history.

## view_folder_delete.crew_id
Actual crew ID.

## view_folder_delete.view
messages/todo/documents/progress; independent from work graph parentage.

## view_folder_delete.id
Stable object UUID.

## view_folder_delete.expected_tree_revision
Observed whole crew/view revision; review finite conflict facts.

## view_folder_assign
Classify one visible stable object, or omit folder_id to unclassify; live ACL, no work lifecycle change or automatic wake.

## view_folder_assign.crew_id
Actual crew ID.

## view_folder_assign.view
messages/todo/documents/progress; independent from work graph parentage.

## view_folder_assign.expected_tree_revision
Observed whole crew/view revision; review finite conflict facts.

## view_folder_assign.object
Visible stable {kind,id}; kind must match selected view.

## view_folder_assign.folder_id
Same-scope folder UUID; omit to remove classification.

## channel_work_list
Read current work references for a readable channel. Returns bounded labels and authoritative status over the same global #/W/H. Pagination binds link and source revisions; an expired cursor is a conflict. No model wake.

## channel_work_link
Associate one existing global work object with a readable active channel, with link-revision CAS. Does not change work parent, owner or status and does not wake agents.

## channel_work_unlink
Remove only a channel work reference with link-revision CAS. The requirement, work or human question and its history remain. Does not wake agents.

## channel_work_list.channel_id
Actual channel ID; current membership determines visibility.

## channel_work_list.after
Opaque next cursor returned by this channel view; never construct it.

## channel_work_list.limit
Items per page, 1–100; default 50.

## channel_work_link.channel_id
Actual readable channel ID.

## channel_work_link.expected_link_revision
Current channel view linkRevision, as decimal text.

## channel_work_link.object
Existing {kind:requirement|work|humanTask,id:global #/W/H}. No new lifecycle object.

## channel_work_unlink.channel_id
Actual readable channel ID.

## channel_work_unlink.expected_link_revision
Current channel view linkRevision, as decimal text.

## channel_work_unlink.object
Existing global #/W/H reference to remove from this channel only.

## work_item_create.request_id
Optional immutable UUID creation key. Required with channel_id. Retry the original payload only; returns the same current work and current association without re-linking after removal.

## work_item_create.channel_id
Optional actual readable channel ID; creates and associates the plan in the same transaction. Requires request_id. Association is a creation receipt, not a permanent work field.

## work_item_close
Close an existing work item on verified facts: done, duplicate, or superseded. Requires the current revision, nonblank evidence and an explanation. Uses your actual permission on the object; it grants no administrative scope or business cancellation authority. It does not submit a progress report. Read the returned closure and current dependencies; on revision conflict, reread and reassess rather than retrying blindly. Evidence is your recorded claim, not automatic independent verification.

## work_item_close.id
The original global W work ID; no replacement work is created.

## work_item_close.expected_revision
The unchanged current revision from work_item_get.

## work_item_close.kind
done for verified completion; duplicate or superseded for factual closure. Never use these to mean the work is not worth doing.

## work_item_close.evidence
Required nonblank evidence: {kind:"commit"|"url"|"text",value:"..."}. State the verifiable source supporting this specific closure.

## work_item_close.note
Required explanation of what was verified and why this closure is justified.

## work_closure_history
Read authoritative factual closure events for the original work, newest first. This is separate from owner-change history. A recorded evidence claim is not independent verification.

## work_closure_history.id
The global W work ID.

## work_closure_history.before_seq
The previous page's nextBeforeSeq, passed unchanged; omitted for the first page.

## work_closure_history.limit
Page size from 1 to 200.

## channel_archive_check
Read current topic-channel archive facts as canonical JSON: revision, canArchive/canRestore, blockers and last action. A check is not an automatic archive instruction and uses no default inactivity threshold. Verify work, requirements, unanswered questions, pending/inflight notifications and delivery windows. Incomplete or unknown facts block verified archive. Archived channels remain queryable by exact ID.

## channel_archive_check.channel
Readable channel exact ID or unique name, including archived channels.

## channel_archive_verified
Revalidate the latest channel_archive_check revision and your verified evidence atomically before archiving a topic channel. Related work must be terminal, with no unresolved facts or inflight delivery. Only the current owner or a legitimate captain may act; Admin role alone grants no authority. Returns canonical JSON and preserves messages, members and associations. The existing channel_archive remains the manual management entry; this tool adds evidence and version checks.

## channel_archive_verified.channel
Readable topic-channel exact ID or unique name.

## channel_archive_verified.expected_revision
The complete revision from the latest check. Recheck after conflicts; never reuse an old version.

## channel_archive_verified.evidence
Concrete facts or evidence you actually verified; nonblank. Do not present model guesses as verification.

## channel_archive_verified.note
Your explicit reason for this archive action; nonblank.

## channel_restore_verified
Explicitly restore an archived topic channel using its latest checked revision. Preserve messages, members and associations and allow new posts without replaying old wakes. Only the current owner or a legitimate captain may act. Returns canonical JSON; the owning crew must remain available.

## channel_restore_verified.channel
Archived topic-channel exact ID or unique name; prefer the ID from the earlier receipt.

## channel_restore_verified.expected_revision
The complete revision returned by channel_archive_check before restoration.

## channel_restore_verified.note
Your explicit reason for this restore action; nonblank.

## admin_case_handoff_request
Request the actual captain of your bound administrative case's crew to take responsibility for unresolved coordination. The recipient is fixed by the system. Delivery is not acceptance or work completion. Retain the original request UUID; after an unknown outcome, query it instead of resending or substituting a new UUID.

## admin_case_handoff_request.request_id
A unique UUID identifying this exact request and its unchanged contents.

## admin_case_handoff_request.reason
The unresolved problem, facts checked and reason captain coordination is required; nonblank.

## admin_case_handoff_get
Read a handoff request you are authorized to inspect and its current revision. Waiting, delivered, explicitly accepted and stale are distinct. Reading does not accept responsibility or complete work.

## admin_case_handoff_get.request_id
The original UUID from the request receipt.

## admin_case_handoff_list
Page through administrative handoff responsibilities and history you are authorized to inspect. A stale record requires review and does not silently release responsibility.

## admin_case_handoff_list.after
The previous response's unchanged nextAfter cursor; never construct one yourself.

## admin_case_handoff_list.limit
Page size from 1 to 100; default 50.

## admin_case_handoff_accept
As the actual current captain and fixed recipient, explicitly accept responsibility for coordinating this administrative case. First inspect the delivered request and current revision. Acceptance does not transfer original Work ownership or mark unresolved work complete.

## admin_case_handoff_accept.request_id
The original UUID of the delivered request you inspected.

## admin_case_handoff_accept.expected_revision
The complete current revision from the request lookup.

## admin_case_handoff_accept.note
Your explicit acceptance statement; nonblank.

## admin_case_custody_get
Read administrative custody, its accountable holder and any succession proposal you may access. Stale custody still requires explicit handling. Reading neither accepts nor discharges responsibility and does not rewrite the original case outcome.

## admin_case_custody_get.crew_id
The original case crew identifier. Access is checked against your actual identity.

## admin_case_custody_get.case_id
The original administrative case identifier, copied from its receipt.

## admin_case_custody_list
Page through administrative custody transfers involving your actual identity. Historical participation grants audit access, not permission to accept new responsibility or broaden authority.

## admin_case_custody_list.after
The previous page's nextAfter, passed unchanged. Do not construct a cursor.

## admin_case_custody_list.limit
Page size, 1–100, default 50.

## admin_case_custody_accept
As the actual current captain, explicitly accept a Human-proposed administrative coordination responsibility after its real delivery. Check the request and current revision first. The former responsibility remains until acceptance. Acceptance changes neither Work ownership nor business completion and never restores the original Admin's authority. Preserve the original UUID after an unknown result and inspect records before any further operation.

## admin_case_custody_accept.request_id
The original, actually delivered Human proposal UUID.

## admin_case_custody_accept.expected_revision
The proposal receipt's complete current revision, not the case or Work revision.

## admin_case_custody_accept.note
Your explicit acceptance statement. Must be nonblank.

## admin_case_work_unlink
Remove one factually incorrect parent or dependency edge between two current targets of this administrative case. Requires both current revisions and evidence. Does not remove a blocker, close work, or grant access outside this case. Preserve request_id and the exact arguments when recovering a result.

## admin_case_work_unlink.request_id
One UUID for this correction. An exact repeat recovers its original receipt.

## admin_case_work_unlink.id
The W ID of the work item whose outgoing edge is incorrect.

## admin_case_work_unlink.expected_revision
The current revision of the work item, from work_item_get.

## admin_case_work_unlink.related_id
The W ID of its existing parent or dependency. It must also belong to this case.

## admin_case_work_unlink.expected_related_revision
The current revision of that parent or dependency, from work_item_get.

## admin_case_work_unlink.edge
The exact structural edge to remove. Use parent or dependency.

## admin_case_work_unlink.evidence
The factual basis for the correction, as a commit, URL, or text.

## admin_case_work_unlink.note
Why the existing edge is factually incorrect.

## managed_object_scope_get
Read the current managed-object database and restore scope and your own crew ID. Use the complete returned scope for new requests and keep the original scope with every request ID.

## managed_object_get
Read a managed table, row, decision, risk, or lesson by its exact organization T, O, D, R, or L ID. Archived-crew objects are still readable and marked frozen.

## managed_object_get.id
The exact T table, O row, D decision, R risk, or L lesson ID.

## managed_object_get.includeSystemDefaults
Include decisions recorded by an applied default when true. Omit or use false for the earlier read behavior.

## managed_tables_list
Read one page of managed tables visible across the organization.

## managed_tables_list.after
The previous page's nextAfter T ID. Omit for the first page.

## managed_rows_list
Read one page of rows for one managed table.

## managed_rows_list.tableId
The exact T ID returned by managed_tables_list or managed_object_get.

## managed_rows_list.after
The previous page's nextAfter O ID for this table. Omit for the first page.

## managed_decisions_list
Read one page of managed decisions visible across the organization, including superseded and withdrawn records.

## managed_decisions_list.after
The previous page's nextAfter D ID. Omit for the first page.

## managed_decisions_list.includeSystemDefaults
Include decisions recorded by an applied default when true. Keep the same value across the pages of one browse.

## managed_risks_list
Read one page of managed risks visible across the organization, newest first.

## managed_risks_list.after
The previous page's nextAfter R ID. Omit for the first page.

## managed_lessons_list
Read one page of managed lessons visible across the organization, newest first.

## managed_lessons_list.after
The previous page's nextAfter L ID. Omit for the first page.

## managed_object_submit
Create a managed table, add or edit a row, or record a decision, risk, or lesson for your own active crew. A decision's author or the owning crew's current captain can replace or withdraw it using the latest D revision. Use the current scope, one original UUID, and the full typed intent. Row edits require the latest row revision and replace the full row. Risk and lesson changes require the latest object revision. You may confirm an active lesson you can read, including one from another crew. After an uncertain response, look up the original request using its UUID and scope.

## managed_object_submit.scope
Copy dbId and restoreEpoch exactly from managed_object_scope_get.

## managed_object_submit.crewId
Use your current crew ID returned by managed_object_scope_get when creating an object. When changing an existing risk, decision, or lesson, supply that object's owning crew ID.

## managed_object_submit.requestId
One original canonical UUID for this exact intent. Keep it for receipt lookup.

## managed_object_submit.change
Provide a typed table definition or the full row values in field order. Integers use decimal strings and null is valid only for a nullable field. To record or replace a decision, supply the question, chosen option, rationale, and exact related object IDs. Supply the reason when withdrawing a decision, changing a risk status, or marking a lesson stale. When a risk actually occurs, use recordRiskOccurrence with the lesson draft. Core creates the linked lesson and records the occurrence together. In lesson input, leave the related risk reference to Core.

## managed_object_request_get
Read the original receipt for a request submitted by this session. Lookup is read-only. A missing or historical receipt does not authorize another execution.

## managed_object_request_get.scope
The exact original dbId and restoreEpoch for the original request.

## managed_object_request_get.crewId
The original owner crew ID of this request.

## managed_object_request_get.requestId
The exact original request UUID.


## managed_object_from_message
Explicitly record a currently readable message as an organization decision, risk, lesson or table row. Object content is organization-readable; source visibility still follows channel permissions. Core captures the real excerpt. One object per message and kind (rows per table); created=false means an existing object was returned and this draft was not saved. Retain the draft. Recover uncertainty only through managed_message_request_get, never resubmit through ordinary object creation.

## managed_object_from_message.scope
Copy the current dbId and restoreEpoch from managed_object_scope_get exactly.

## managed_object_from_message.crewId
Use the owning crew of the currently attached session.

## managed_object_from_message.requestId
Retain one original UUID for this exact message and draft.

## managed_object_from_message.sourceMessageId
The exact currently readable source message ID; never provide a fabricated excerpt.

## managed_object_from_message.change
Only recordDecision, recordRisk, recordLesson or addRow. New content must be suitable for organization-wide reading. Rows require full typed values for a current unfrozen table owned by your crew.

## managed_message_request_get
Read the original message-recording receipt only. This is separate from managed_object_request_get. Source access is rechecked. known/historical never authorize replay, and notFound does not prove the write was not submitted.

## managed_message_request_get.scope
The exact original message-recording dbId and restoreEpoch.

## managed_message_request_get.crewId
The crew ID of the original message-recording request.

## managed_message_request_get.requestId
The original message-recording UUID, never a replacement request ID.

## managed_object_source_get
Read an object source. none means no recorded source; restricted contains no IDs or excerpt. deleted retains only an authorized excerpt. available identifies the actual crew, channel, message and excerpt. Organization-readable objects do not grant access to their source messages.

## managed_object_source_get.id
The exact D/R/L/O object ID.

## managed_objects_from_message
Read a bounded page of organization objects already recorded from a currently readable message. Does not create objects or notify anyone.

## managed_objects_from_message.messageId
The exact source message ID.

## managed_objects_from_message.after
The exact nextAfter returned by the previous page; omit for the first page.


## artifact_preview
Read the canonical artifact reference without registering, verifying, accessing repositories or networking. Use your current crew. Full SHAs and absolute URLs are global; short SHAs, files and tests without an explicit opaque repository ID remain unassigned in your crew. PRs and ordinary links normalize to the same url: pointer.

## artifact_preview.crewId
The exact current or owning crew ID, never inferred from a working directory.

## artifact_preview.input
Explicit kind and value; optional repository is an explicitly declared opaque repository ID, never guessed from cwd.

## artifacts_list
Read the organization artifact directory in pages of at most 16. All artifacts remain unverified.

## artifacts_list.after
Copy the exact nextAfter from the previous page; omit for the first page.

## artifact_get
Read an unverified artifact by its exact canonical reference. Does not check whether a file or external link exists.

## artifact_get.reference
Copy namespace and key exactly from preview or a previously read artifact.

## artifact_backlinks
Read bounded backlinks to an artifact, preserving owning crew and manual/automatic provenance. A link is not system verification.

## artifact_backlinks.reference
Copy namespace and key exactly from preview or a previously read artifact.

## artifact_backlinks.after
Copy the exact nextAfter from the previous page; omit for the first page.

## artifact_subject
Read bounded artifact links of the exact W/#/D/L/R/O/T without changing that work or object.

## artifact_subject.subject
The exact kind and global ID of the existing subject.

## artifact_subject.after
Copy the exact nextAfter from the previous page; omit for the first page.

## artifact_submit
Explicitly register, link or remove a manual artifact link. First use artifact_preview and retain the exact scope, original UUID and full intent. Existing ownership/frozen authority applies. Automatic Work completion and lesson evidence links cannot be removed. Registration records a pointer without verification, git scanning, chat or wakes. Recover uncertainty only through artifact_request_get, never resubmit.

## artifact_submit.scope
Copy dbId and restoreEpoch exactly from managed_object_scope_get.

## artifact_submit.crewId
The exact current or owning crew ID, never inferred from a working directory.

## artifact_submit.requestId
Retain one canonical original UUID for the exact intent; never replace it during recovery.

## artifact_submit.change
register explicitly pins a pointer; link lazily creates and links atomically; unlink removes only the exact manual expectedEdgeId. L appliesTo accepts only files, W/# allow evidence or relates, and other objects allow relates. linkExisting links an already-read canonical reference without creating or reassigning it, including another crew's unassigned pointer.

## artifact_request_get
Read the immutable receipt of the original artifact request, separate from ordinary objects and message recording. Historical or absent results never authorize replay.

## artifact_request_get.scope
Copy dbId and restoreEpoch exactly from managed_object_scope_get.

## artifact_request_get.crewId
The exact current or owning crew ID, never inferred from a working directory.

## artifact_request_get.requestId
Retain one canonical original UUID for the exact intent; never replace it during recovery.


## artifact_history_status
Read the retained artifact-evidence indexing job of your own current live crew. Distinguish fixed cuts, genuine legacy completion events, frozen legacy snapshots, organization events and lesson evidence. An advanced cursor does not prove that your request succeeded.

## artifact_history_status.crewId
The exact current unarchived crew of this session; Agents cannot maintain another crew.

## artifact_history_status.jobId
The exact job UUID to inspect; status reads the default job when omitted.

## artifact_history_begin
Explicitly fix a history cut for your own current live crew, or reuse an existing job. This only begins a job, never processes its next page, scans git, rewrites business facts/events, chats or wakes. Retain the original UUID. Jobs in an older restore epoch are historical and never auto-resumed.

## artifact_history_begin.scope
Copy dbId and restoreEpoch exactly from managed_object_scope_get.

## artifact_history_begin.crewId
The exact current unarchived crew of this session; Agents cannot maintain another crew.

## artifact_history_begin.requestId
Retain one canonical original UUID for this exact Begin or Step; never replace it during recovery.

## artifact_history_step
Explicitly process one page of 1–64 retained sources using the exact job revision and cursor just read. Pointers remain unverified, and frozen snapshots are not original completion events. Recover uncertainty only by the original UUID; never infer success from current progress, resend a step, or auto-continue across restore epochs.

## artifact_history_step.scope
Copy dbId and restoreEpoch exactly from managed_object_scope_get.

## artifact_history_step.crewId
The exact current unarchived crew of this session; Agents cannot maintain another crew.

## artifact_history_step.requestId
Retain one canonical original UUID for this exact Begin or Step; never replace it during recovery.

## artifact_history_step.jobId
The exact job UUID to inspect; status reads the default job when omitted.

## artifact_history_step.expectedRevision
Copy the job revision just read; reread status after a conflict instead of resending.

## artifact_history_step.expectedCursor
Copy the entire job cursor including phase and after; never calculate it yourself.

## artifact_history_step.limit
The explicit source budget for this page, an integer from 1 to 64.

## artifact_history_request_get
Read the immutable receipt of your own original history-index request, independently of ordinary artifact receipts. known/historical/notFound never authorize replay.

## artifact_history_request_get.scope
Copy dbId and restoreEpoch exactly from managed_object_scope_get.

## artifact_history_request_get.crewId
The exact current unarchived crew of this session; Agents cannot maintain another crew.

## artifact_history_request_get.requestId
Retain one canonical original UUID for this exact Begin or Step; never replace it during recovery.

## artifact_backlinks.includeLegacySources
Set true to include genuine legacy Done and frozen-snapshot provenance. Never treat snapshots as original events. Defaults to false for the legacy Origin set.

## artifact_subject.includeLegacySources
Set true to include genuine legacy Done and frozen-snapshot provenance. Never treat snapshots as original events. Defaults to false for the legacy Origin set.


## managed_table_v2_get
Read a real V2 organization table, stable field IDs, current schemaRevision and write policy. Never omit link columns or substitute SQL when V1 cannot represent it. Omitted contractVersion or explicit 2 preserves V2. Explicit 3 requires action=table/defineCheck/history/historyValue. Check the current name and purpose first; read history values by the original key/path in bounded chunks, with current authority checked every time.

## managed_table_v2_get.id
Actual T identity.

## managed_tables_v2_list
Read one bounded page of currently readable organization tables, including archived read-only tables; later pages remain unread. Omitted version or explicit 2 preserves the V2-expressible catalog. Explicit 3 reads all supported types; includeArchived controls independently archived tables. Never drop unknown fields and claim a complete definition.

## managed_tables_v2_list.after
Return the preceding nextAfter verbatim; omit for the first page.

## managed_row_v2_get
Read a real O row with current field IDs and authorized link projections. A hiddenSet never reveals hidden count or a transferable target. Omitted version or explicit 2 preserves V2. Explicit 3 uses action=row for scalarCells/linkFieldIds. Unloaded links are not empty; fetch action=links with the real field ID and exact table/row witnesses.

## managed_row_v2_get.id
Actual O identity.

## managed_rows_v2_list
Read one bounded V2 row page together with the exact T definition and schemaRevision. Omitted version or explicit 2 preserves V2. Explicit 3 reads typed scalar row pages; load link fields explicitly rather than treating unloaded fields as empty sets.

## managed_rows_v2_list.tableId
Actual T identity.

## managed_rows_v2_list.after
Previous nextAfter; omit on first read.

## managed_table_v2_submit
Explicitly create a table, perform one safe schema change, add a row, or patch stable field IDs. Read real schema/row revisions first; omitted fields remain unchanged. Hidden links can only be kept as the original whole cell set: never count, copy or clear them. With hiddenSet, visible links cannot increase. Rename/hide/restore/single-to-many never change business object state; title field must be active scalar and must be replaced before hiding it. Preserve the original UUID; after uncertainty only look it up, never resubmit automatically. Omitted version or explicit 2 uses V2. Explicit 3 adds complete field types, options and independent table/row archive/restore. PatchRow/AlterTable/lifecycle require a reason and exact revisions. Obtain a current defineCheck proof and use confirmNew when needed; archiving preserves business state. Distinguish false, Null and Keep. Decimal is a canonical string and never passes through floating point.

## managed_table_v2_submit.scope
Exact current Core scope from managed_object_scope_get; an old restore epoch is historical lookup only.

## managed_table_v2_submit.crewId
Actual request crew; do not infer from a path, target owner or label.

## managed_table_v2_submit.requestId
Original UUID persisted before submission; resolve uncertainty with managed_table_v2_request_get only.

## managed_table_v2_submit.operation
Closed typed operation. Patch only edited field IDs against exact schema/row revisions; resolve targets through the live read API. No narrowing, cross-cell hidden Keep, or Work/D/R/L/H state mutation.

## managed_table_v2_request_get
Read only this original V2 request. known is its immutable effect; historical is historical; notFound does not prove no submission. Current rows never substitute for a receipt. Use the original journal version: omission means 2; explicit 3 reads only the original V3 codec. A wrong version conflicts; never guess another version or treat notFound as permission to resubmit.

## managed_table_v2_request_get.scope
Original exact scope; never replace restoreEpoch.

## managed_table_v2_request_get.crewId
Original request crew.

## managed_table_v2_request_get.requestId
Original UUID.

## managed_link_resolve_target
Resolve an exact target through current authority and actual Core summary. Message access is checked by B15 each time; never scan message pages to infer an old message. Does not create artifacts/namespaces/targets. Readability grants no write permission.

## managed_link_resolve_target.target
Complete typed target. Knowledge/document targets require the explicitly advertised managedLinks.knowledgeTargets.v1 capability and their real qualified SQLite source; they are unavailable without it. Never substitute an old Core mirror or SQL table name. Folder is unsupported.

## managed_links_list
Read a bounded page of manual/column edges, optionally real read-only business adapters. Outgoing hidden sets expose only one marker and no count; unreadable incoming sources are omitted. Use original business tools for adapter effects.

## managed_links_list.query
Exact outgoing source or incoming target with explicit includeAdapters. Artifact source includes metadataCrewId and never transfers the global pointer or another crew’s edges.

## managed_links_list.after
Previous nextAfter; omit for the first page.

## managed_links_list.limit
Explicit page bound 1–64; read subsequent pages explicitly.

## managed_link_submit
Explicitly add/remove manual relates with current source authority, target readability and manualLinkRevision CAS. Does not change Work/Ask/D/R/L state. Column/adapter edges cannot be edited here. Artifact source is per request crew metadata. Unknown results require original UUID lookup, never automatic resubmission.

## managed_link_submit.scope
Current exact Core scope; lookup uses the original scope.

## managed_link_submit.crewId
Actual request crew; artifact metadataCrewId must match.

## managed_link_submit.requestId
Original UUID saved before dispatch; never retry an unknown action with a new UUID.

## managed_link_submit.operation
link/unlink with exact source/manualLinkRevision; removal requires expectedEdgeId and manual relates. Hidden marker is never a target.

## managed_link_request_get
Read the original manual-relates request and immutable effect. Lost target authority never echoes old target content; current edge presence cannot prove this action succeeded.

## managed_link_request_get.scope
Original exact scope.

## managed_link_request_get.crewId
Original request crew.

## managed_link_request_get.requestId
Original UUID.

## search.in
messages searches messages, objects searches organization objects, all searches both (the new-host default). This never expands permissions; older hosts do not support objects/all.

## search.type
Filter decision, risk, lesson, work, requirement, ask, table, row, artifact or message. A real table ID such as T2 searches that table's rows. Empty intersections with other filters are refused, never widened.

## search.cursor
Pass the previous nextCursor unchanged with the original query and all filters. after/before remain dates. An empty page with nextCursor can continue; after expiry, start a new search.

## organization_search_expand
Expand real one-hop links of a currently readable target, outgoing where supported and incoming, with current/open and recently changed neighbors first. Return original edges, direction, neighbor references and current summaries. Different relations may reach the same neighbor; a self-edge appears once. This does not create editable copies of business relations. Private Knowledge/Document cross-database targets are not supported yet.

## organization_search_expand.target
The complete typed target from a search result. Shared artifacts can have outgoing links maintained by different crews; preserve each edge's actual source and owner.

## organization_search_expand.cursor
The previous nextCursor with the same target and other filters. After expiry, expand again.

## organization_search_expand.limit
Requested result maximum, 1-200, default 20. A page can be shorter or empty; nextCursor indicates continuation.

## organization_search_expand.include_system_defaults
Include system-default decisions, default false. Current read permission is still rechecked.

## managed_table_v2_get.action

Explicit version 3 only: table reads a definition; defineCheck checks name/purpose; history reads bounded action metadata; historyValue reads complete value chunks by original key and typed path.

## managed_table_v2_get.scope

For defineCheck, preserve the exact current scope returned by managed_object_scope_get.

## managed_table_v2_get.crewId

Actual owner crew for defineCheck; never infer it from a name or path.

## managed_table_v2_get.excludingTableId

Real T only when checking that table for rename/purpose changes. A new table may not exclude another table.

## managed_table_v2_get.title

Unmodified proposed title. Core checks current crew-local uniqueness and near names; historical aliases do not stand in for current names.

## managed_table_v2_get.purpose

Unmodified proposed purpose. Creation/purpose changes require nonempty text. Only renaming a legacy table with no purpose may use null; empty text is not null.

## managed_table_v2_get.cursor

Original continuation token for this defineCheck/history/historyValue read. Re-read after source/authority/proof changes; never invent a token.

## managed_table_v2_get.source

Real table or row history source. Source readability does not grant access to every historical link target.

## managed_table_v2_get.key

Original event/legacy receipt locator returned by history; never invent an old event UUID or change its codec.

## managed_table_v2_get.path

Typed scalar/links/reason/schema path and before/after side. Reassemble complete values using their returned type; restricted links expose only an aggregate restriction marker.

## managed_row_v2_get.action

Explicit version 3 only: row reads scalars and unloaded link field IDs; links reads one field through current target authority.

## managed_row_v2_get.rowId

Actual O for action=links.

## managed_row_v2_get.fieldId

Stable p field ID belonging to the actual table of that O.

## managed_row_v2_get.expectedSchemaRevision

Current schemaRevision of the original table; re-read after source changes.

## managed_row_v2_get.expectedRowRevision

Current revision of the original O, never a witness from another row or a later result.

## managed_tables_v2_list.includeArchived

Version 3 only: include independently archived tables. Archived objects remain read-only.

## managed_rows_v2_list.includeArchived

Version 3 only: include independently archived rows. Parent table/crew freeze still applies.

## managed_table_v2_get.contractVersion

Contract selector: omitted or integer 2 uses V2; integer 3 uses the new contract. Null, strings, fractional-number representations and unknown versions are rejected without fallback.

## managed_tables_v2_list.contractVersion

Contract selector: omitted or integer 2 uses V2; integer 3 uses the new contract. Null, strings, fractional-number representations and unknown versions are rejected without fallback.

## managed_row_v2_get.contractVersion

Contract selector: omitted or integer 2 uses V2; integer 3 uses the new contract. Null, strings, fractional-number representations and unknown versions are rejected without fallback.

## managed_rows_v2_list.contractVersion

Contract selector: omitted or integer 2 uses V2; integer 3 uses the new contract. Null, strings, fractional-number representations and unknown versions are rejected without fallback.

## managed_table_v2_submit.contractVersion

Contract selector: omitted or integer 2 uses V2; integer 3 uses the new contract. Null, strings, fractional-number representations and unknown versions are rejected without fallback.

## managed_table_v2_request_get.contractVersion

Contract selector: omitted or integer 2 uses V2; integer 3 uses the new contract. Null, strings, fractional-number representations and unknown versions are rejected without fallback.
