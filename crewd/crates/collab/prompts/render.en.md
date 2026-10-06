Group notices and injected text shown to people and agents (read by crewd-collab render / asks / approvals / restart).
One `## name` per section; `{placeholders}` are filled by code; see REQUIRED in src/prompts.rs.

## notice.approval
{session} wants to use tool {tool}: {summary}
This call was denied on the spot; it will move on to other work. Allow it in the approvals list to let it retry once (one-time grant); deny it and it will be told no.

## notice.restart
crewd restarted. {count} sessions were running before; resume them now? (Or not yet: @-mention one of them in a channel to resume just that one.)

## notice.restart.item
- {name}

## sender.human
Human

## sender.system
crewd

## sender.session
{name} (ext {ext})

## sender.session.unnumbered
{name} (member)

## sender.captain
{name} (captain)

## inject.header
New messages in your channels (only those addressed to or visible to you):

## inject.channel.lobby
[{crew} · lobby]

## inject.channel.human
[{crew} · human channel]

## inject.channel.topic
[channel: {name}]

## inject.channel.dm
[direct message]

## line
[{at}] {sender}: {text}

## line.directed
(addressed to {targets}, not to you)

## line.directed.unknown
(not addressed to you)

## line.reply
(reply to {reply})

## wake.header
You were woken up for the following.

## wake.message
Messages that need your attention:

## wake.timer
A reminder you scheduled is due.

## wake.ask_answer
The human answered your question {ask}.
Question: {question}
Answer: {answer}

## wake.ask_defaulted
Nobody answered your question {ask} in time; going with the default: {answer}
Question: {question}
Actions after the deadline still pass the approval gate: if a call is denied pending approval, move on; you will be woken to retry once the human allows it.

## wake.approval_allowed
The human allowed {tool} ({summary}). Retry that call now and it will go through — once.

## wake.approval_denied
The human did not allow {tool} ({summary}). Find a way that does not need it, or explain in the channel why you need it.

## wake.resume_note
Your note on how to continue: {note}

## wake.restart
A crewd restart interrupted your last turn. Check the current state (working tree, new channel messages) before continuing.

## wake.system
System notice.

## wake.checkpoint
A checkpoint is due.

## wake.stop_check
Before stopping, make sure every work item you own has a next step.

## wake.note
Note: {note}
