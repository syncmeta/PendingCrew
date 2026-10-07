Refusal texts shown to humans and agents (the collab rules read these). One `## code` section per code (`area.name`), matching `Code` in src/refusal.rs.
Every text says what to do next. `{placeholders}` are filled by code; which ones are allowed is declared in `refusal_codes!` and must match across languages.

## ask.id_not_found
Crew {crew} has no {id}. Check the id first (asks start with H, work items with W, entries with #).

## ask.bad_id
"{input}" is not a valid ask id. Write H followed by a positive integer, e.g. H3.

## ask.system_cannot_withdraw
The system cannot withdraw an ask. Withdraw it as a session or as the human.

## ask.remove_blocked
Ask {ask} in crew {crew} still blocks a live work item. Answer it, or have the owner reroute, then remove the crew.

## channel.not_found
There is no channel {channel}. List the existing channels and check the id.

## channel.system_cannot_act
The system cannot act as a member here. Do it as a session or as the human.

## channel.dm_one_human
A DM has at most one human side. To talk to the human, post in the crew's human channel.

## channel.create_kind_fixed
Only topic and dm channels can be created by hand. The lobby and the human channel are created by crewd.

## channel.name_empty
The channel name cannot be empty. Give the channel a name.

## channel.name_is_sender_label
The channel name {value} collides with a speaker label (human / crewd) and would pass for one in message logs. Pick another name.

## channel.dm_needs_two
A DM has exactly two parties (at most one of them the human). For more people, create a topic channel.

## channel.dm_wrong_crew
A DM belongs to the initiator's crew {owner}, not {crew}. Create it under crew {owner}.

## channel.manage_forbidden
Only the owner, the captain or the human can change this channel. Ask one of them, or have the owner transfer ownership.

## channel.fixed_name
The lobby and the human channel cannot be renamed; their names follow the crew. Rename the crew instead.

## channel.fixed_no_archive
The lobby and the human channel cannot be archived by hand; they are frozen when their crew is removed. To retire them, remove the whole crew (crew.remove, reversible).

## channel.lobby_cannot_add
The lobby members are the whole crew, so nobody is added by hand. To get someone into the lobby, make them a session of this crew.

## channel.dm_cannot_grow
A DM is limited to two people. To bring in more people, create a topic channel.

## channel.archived
Channel {channel} is archived and takes no messages. Unarchive it to continue, or start a new channel; if its crew was removed, ask the human to restore the crew first (crew.restore).

## channel.add_members_member_only
Only members of the channel can add people. Ask a member to add them, or have a member add you first.

## channel.no_human_channel
{crew} is not a top-level crew and has no human channel of its own. The human channel is {home}/human; post there.

## channel.human_channel_other_home
The human channel of {session} is {home}/human, so it cannot join channel {channel}. To talk across crews, use a DM.

## channel.lobby_cannot_remove
The lobby members are the whole crew, so nobody is removed by hand. To take someone out of the lobby, take them out of the crew.

## channel.dm_cannot_leave
A DM cannot be left. To end it, archive it.

## channel.leave_forbidden
You can only leave on your own, or be removed by the owner, the captain or the human. Ask one of them to remove the member.

## channel.owner_must_transfer
The owner must transfer ownership to another member before leaving.

## channel.not_explicit_member
Not on this channel's member list. Implicit members (the lobby, the captain's human channel) cannot be removed; to take someone out, take them out of the crew, or change the captain.

## channel.owner_follows_captain
The owner of the lobby and the human channel is the crew captain. To change the owner, change the captain.

## channel.transfer_owner_only
Only the owner can start a transfer. Ask the owner, or ask the captain or the human to set the owner by force.

## channel.already_owner
You are already the owner; there is nothing to transfer to yourself. Pick someone else.

## channel.new_owner_must_be_member
The new owner must be a member of the channel. Add them to the channel first.

## channel.transfer_target_left
You are no longer in this channel, so you cannot accept ownership. Ask the owner to pick someone else.

## channel.transfer_not_for_you
This transfer is not addressed to you; only the named member can accept it.

## channel.no_pending_transfer
There is no ownership transfer waiting to be accepted. The owner must start one first.

## channel.transfer_withdraw_forbidden
Only the owner, the member it was offered to, the captain or the human can withdraw this transfer. Ask one of them to do it.

## channel.force_owner_forbidden
Only the captain or the human can set the owner by force. Ask one of them, or have the owner start a transfer.

## crew.not_found
There is no crew {crew}. List the crews and check the id.

## crew.removed
Crew {crew} has been removed (hidden and frozen, not deleted) and is read-only. Ask the human to restore it (crew.restore; only the human can, from the client) before changing it.

## crew.management_fixed
The headquarters crew is the management crew: it cannot be hidden, removed or permanently deleted. Leave it as it is; to tidy the sidebar, hide or remove other crews.

## crew.management_not_in_tree
The headquarters crew is outside the crew tree: it cannot have a parent or children, and no crew can be placed under it. Pick another crew as the parent, or leave parentId out to make a top-level crew.

## crew.title_empty
The crew title cannot be empty. Give the crew a title.

## crew.working_dir_empty
The working directory cannot be empty. Give the directory path the crew's sessions should work in.

## crew.cannot_parent_self
A crew cannot be placed under itself. Pick a different parent.

## crew.would_cycle
{parent} is below {crew}, so adopting it would create a cycle. Pick another parent, or first release {parent} from {crew}.

## crew.not_direct_child
{crew} is not a direct child of {from}. Check the parent links before releasing.

## crew.running_sessions
Crew {crew} still has {running} running sessions. Stop them first.

## crew.live_children
Crew {crew} still has child crews that are not removed: {children}. Move them away (crew.release) or remove them first.

## crew.wakes_inflight
Crew {crew} still has {count} wakes being delivered. Wait for delivery to finish, then remove it again.

## crew.parent_removed
The parent crews of {crew} ({parents}) have been removed, and nothing live may hang under a removed crew. Ask the human to restore {parents} first; once that is done, release {crew} from them if you no longer want it there.

## crew.not_removed
Crew {crew} has not been removed. Only removed crews can be permanently deleted; remove it first (crew.remove). Permanent deletion really deletes everything of the crew; the snapshot taken before it is kept for 30 days only.

## crew.session_not_in_crew
{session} is not in crew {crew}. The captain has to be chosen from the crew's own sessions.

## crew.reorder_mismatch
crewIds must include every crew on this level that is not removed, without repeats and without other crews (removed ones are optional). Currently: {current}

## crew.no_captain
Crew {crew} has no captain right now. Appoint a captain first, or address a specific session.

## directory.bad_address
The address "{address}" is not valid. Write a crew number or crew-extension, e.g. 3 or 3-2.

## directory.no_such_crew
The directory has no crew number {number}. Check the number.

## directory.crew_removed
Crew number {number} has been removed and takes no messages. Ask the human to restore it before contacting it.

## directory.no_such_extension
Crew number {number} has no extension {ext}. Check the extension number.

## message.not_found
There is no message {message}. Check the message id.

## message.seq_not_found
There is no message with seq={seq}. Check the sequence number.

## message.bad_time
The time "{value}" is not RFC 3339 ({why}). Use a format like 2026-01-02T03:04:05Z.

## message.search_empty
The search query cannot be empty. Enter the text to look for.

## message.not_a_member
{who} is not a member of channel {channel}. Members: {members}

## message.captain_not_a_member
The crew captain {captain} is not a member of channel {channel}, so mentioning the captain would not wake anyone. Members: {members}

## message.empty
The message cannot be empty. Write what you want to say.

## message.too_long
A message body is at most {max} bytes; this one is {actual} bytes. Split it up, or put the long content in a file and refer to it.

## message.reserved_category
The category {category} is reserved for system notices and cannot be set on a message. Remove the category and send again.

## message.attachments_unsupported
Attachment upload is not implemented yet. Paste the content into the message, or save it as a file and give the path.

## message.sender_not_member
{session} is not a member of channel {channel}. Ask a member to add you first, or post in a channel you belong to.

## message.mention_needs_target
An @session mention needs a targetId. Say which session you mean to wake.

## message.reply_other_channel
You can only reply to a message in the same channel. To answer a message elsewhere, post in that channel.

## message.id_conflict
The new message id collided with an existing one. Send it again.

## restart.not_found
There is no restart prompt {id}. Check the id.

## restart.already_resumed
The prompt was already answered with "resume"; there is nothing more to answer.

## restart.superseded
Restart prompt {id} was replaced by a newer one. Answer the latest one instead.

## session.not_found
There is no session {session}. Check the session id.

## session.bad_executor
The executor {executor} is not known. Use claude, codex or acp:<id>.

## session.title_too_long
A session title is at most {max} characters (this one has {actual}). Shorten it and put the details in the brief.

## session.title_or_brief_required
A session needs a title (at most {max} characters) or a brief; with neither there is nothing to name it by. Provide one.

## session.title_is_sender_label
The title {value} collides with a speaker label (human / crewd) and would pass for one in message logs. Pick another title.

## session.display_name_is_sender_label
The display name {value} collides with a speaker label (human / crewd) and would pass for one in message logs. Pick another name.

## session.captain_exists
Crew {crew} already has a captain. To change the captain, use set_captain.

## settings.bad_locale
meta.locale holds {value}, but only zh and en are accepted. Set it to zh or en.

## settings.bad_max_live
meta.maxLiveSessions holds {value}, which is not a positive integer. Set a positive integer, or clear it.

## settings.max_live_min
The limit must be at least 1. To remove the limit, pass null.

## todo.response_empty
The response cannot be empty. Write the response text.

## todo.not_found
This crew has no Agent Todo #{number}. Check the number.

## wake.source_empty
A wake's source cannot be empty. State where the wake comes from.

## wake.not_found
There is no wake {wake}. Check the wake id.

## wake.bad_transition
Wake {wake} is {from} and cannot become {to}. Check its current state first.

## wake.latch_bad_scope
A circuit-breaker ledger id cannot be empty or contain an arrow character. Got: {scope}. Use an id made of ordinary characters.

## wake.latch_row_unreadable
wake_latches holds a row that cannot be read: scope={scope} id={id}. This is bad data in the database and needs a human.

## listen.until_in_past
The listen deadline must be in the future. Give a later time, or set no deadline.

## listen.not_readable
You cannot read channel {channel}, so you cannot listen to it: listening never widens what you can read. Listen only to channels you can read (your crew's channels, channels you are in, your home's human channel); to follow this one, ask the captain of the crew it belongs to to add you.
