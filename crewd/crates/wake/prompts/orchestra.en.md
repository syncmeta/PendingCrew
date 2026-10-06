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

## quota.scheduled
"{name}" hit its usage limit; a wake is scheduled for the quota reset (in about {minutes} minutes).

## quota.fallback
"{name}" hit its usage limit and no reliable reset time is known; a back-off wake is scheduled (in about {minutes} minutes).

## quota.wake
Your quota should have reset. You stopped last time because you hit the usage limit — first check the current state: get_quota for the quota, git log for where main is, whether your branch still exists. Messages that @-ed you meanwhile may already have been handled; verify before deciding which still need an answer. If you are still limited, push this back yourself with schedule_wakeup.

## delivery.failed.group
A wake for "{name}" failed to arrive {attempts} times in a row (last error: {error}) and was given up. The related messages are still in the channel and it will see them when it next wakes; if this is urgent, ask a human to look at the session.

## loop.tripped.human
Possible wake loop: "{origin}" woke "{target}" {count} times within {minutes} minutes and "{target}" produced nothing new meanwhile (no tool call, no work-item change, replies only went back to whoever woke it). Only this one link is stopped: "{origin}" will no longer wake "{target}" automatically; human messages still arrive. If this is fine, press "Recover" on this breaker in the client.

## loop.tripped.owner
Automatic wakes "{origin}" → "{target}" looked like a loop and were stopped (only this link; details in the human channel). If they need to keep working together, ask a human to recover it in the client.

## origin.system
system

## captain.changed.new
You are now the captain of this crew. Captain-only tools are available to you; read the unread messages in the lobby and the human channel before deciding what to do next.

## captain.changed.old
You are no longer the captain of this crew (new captain: {name}). Finish what you are doing; assigning work and reporting to the human now belong to the new captain.

## decision.first.worker
{name} is stuck waiting for a decision:

## decision.first.captain
{name} (captain) is stuck waiting for an answer:

## decision.escalate
{name} is still stuck; nobody has handled it for {minutes} minutes:

## decision.options.numbered
How to answer: the options on this screen are numbered; send the matching number with nudge_session.

## decision.options.unnumbered
How to answer: the options on this screen are not numbered, and nudge_session refuses numbers on such a dialog. It knows these bare keys: {keys} (each is pressed once on its own, with no Enter added; esc does not cancel the dialog either: it makes claude exit); anything else is sent into the session as a message (typed into the input box and submitted) — and that submit also lands on whichever option is highlighted right now, which may not be the one you want. If the highlight is already on the option you want → send `enter`; {move}

## decision.move.none
otherwise → hand it to a human.

## decision.move.keys
otherwise → send {keys} to move it there first, then `enter`.

## decision.tail.worker
The captain can look with inspect_session; follow the line above (it was computed from the screen at that moment). If you cannot decide, @ the human.

## decision.tail.captain
A human needs to open this session's terminal and choose.

## decision.tail.escalate
A human needs to decide: open this session's terminal and choose, or have the captain press it with nudge_session.

## decision.session
(session: {session})
