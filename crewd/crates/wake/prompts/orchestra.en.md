<!--
Group notices and wake notes of crewd-wake (spec §22.4, §22.7). One paragraph per section;
placeholders are declared in SECTIONS in src/text.rs. Written for agents and humans; never in code.
-->

## restart.interrupted
A crewd restart interrupted your previous turn. Check the current state first (git status, how far the thing you were doing got, new messages in your channels), then decide how to continue; do not resume half-remembered steps blindly.

## resume.refused.group
"{name}" could not get its previous conversation back — claude refused the session id and said: `{said}`. This is a fresh conversation: it still has the channel context, but details discussed in the terminal must be repeated.

## resume.refused.diag.found
Note: the log for this session id does exist on disk ({path}), so the log is not missing; look elsewhere (claude version, file permissions, a corrupted file).

## resume.refused.diag.missing
Note: no log for this session id exists anywhere under claude's projects directory.

## resume.refused.agent
[Note] Your previous session ({id}) could not be resumed — claude itself said "{said}" — so this is a fresh conversation. Channel messages are injected as usual; do not pretend to remember details from the terminal.

## resume.failed.group
"{name}" cannot be started: {detail}. Someone needs to take over its work, or a human should look at it.

## quota.wake
Your quota should have reset. You stopped last time because you hit the usage limit — first check the current state: get_quota for the quota, git log for where main is, whether your branch still exists. Messages that @-ed you meanwhile may already have been handled; verify before deciding which still need an answer. If you are still limited, push this back yourself with schedule_wakeup.

## loop.tripped.human
Possible wake loop: "{origin}" woke "{target}" {count} times within {minutes} minutes and "{target}" produced nothing new meanwhile (no tool call, no work-item change, replies only went back to whoever woke it). Only this one link is stopped: "{origin}" will no longer wake "{target}" automatically; human messages still arrive. If this is fine, press "Recover" on this breaker in the client.

## origin.system
system

## captain.changed.new
You are now the captain of this crew. Captain-only tools are available to you; read the unread messages in the lobby and the human channel before deciding what to do next.

## captain.changed.old
You are no longer the captain of this crew (new captain: {name}). Finish what you are doing; assigning work and reporting to the human now belong to the new captain.

## decision.session
(session: {session})

## hibernate.resume
You were suspended by a hibernate: the human is restarting the computer (or pausing crewd as a whole), so crewd let every session stop at a safe point before exiting. Your conversation has been resumed as it was; channel messages from the meantime are injected as usual.

## hibernate.doing
The wakes you were handling before you stopped: {doing}

## hibernate.doing.unknown
You were working before you stopped, but crewd did not record which batch of wakes it was — look at the last few turns of your own conversation.

## hibernate.finished
You finished that turn within the hibernate deadline and were not interrupted. Check the current state before you continue.

## hibernate.interrupted
The hibernate deadline passed before you finished that turn, so crewd interrupted you with Esc. Whatever command was running at that moment (build, test, install, migration…) did not complete, and its output and result cannot be trusted: check the current state (git status, how far you got), run the interrupted command again, and only then continue.

## decision.first
"{name}" is waiting for a human decision:

## decision.howto
Open this session and choose, or choose in the pending list.

## notice.recovered
"{name}" recovered.

## notice.give_up.crew
Wakes were given up in this crew: {sessions}. Last error: {error}.

## notice.quota.captain
Captain "{name}" is blocked by {executor} quota; recovery is scheduled.

## notice.binary_replaced
The crewd binary was replaced. Restart crewd before starting new sessions.

## notice.input_draft
A human input draft has held delivery to "{name}" for ten minutes. Clear or submit the draft to resume delivery.

## session.input_draft_unknown
Delivery to "{name}" has waited ten minutes: human input was detected, but the input box cannot be identified as a draft or placeholder. Check the terminal; delivery stays held.

## brief.undelivered
The opening brief for "{name}" was not delivered. Re-dispatch the brief.
