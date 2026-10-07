Group notices and injected text shown to people and agents (read by crewd-collab render / asks / restart).
One `## name` per section; `{placeholders}` are filled by code; see REQUIRED in src/prompts.rs.

## notice.restart
crewd restarted. {count} sessions were running before; resume them now? (Or not yet: @-mention one of them in a channel to resume just that one.)

## notice.restart.item
- {name}

## notice.ask.crew_removed
The crew was removed

## sender.human
Human

## sender.system
crewd

## sender.session
(ext {ext}) {name}

## sender.session.unnumbered
(member) {name}

## sender.captain
(captain) {name}

## inject.header
New messages from others in your channels (all of them, not only those that @-mention you; each line marks who was @-mentioned):

## inject.channel.lobby
[{crew} · lobby]

## inject.channel.human
[{crew} · human channel]

## inject.channel.topic
[channel: {name}]

## inject.channel.dm
[direct message]

## line
#{seq} [{at}] {sender}: {text}

## line.at
(@{targets})

## line.at.you
you

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
Actions after the deadline still go through your own permission prompts (claude / codex); if one is blocked, move on to other work.

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
