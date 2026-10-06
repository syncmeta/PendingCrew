Group notices and injected text shown to people and agents (read by crewd-collab render / asks / approvals / restart).
One `## name` per section; `{placeholders}` are filled by code; see REQUIRED in src/prompts.rs.

## notice.approval
{session} wants to use tool {tool}: {summary}
This call was denied on the spot; it will move on to other work. Allow it in the approvals list to let it retry once (one-time grant); deny it and it will be told no.

## notice.restart
crewd restarted. {count} sessions were running before; resume them now? (Or not yet: @-mention one of them in a channel to resume just that one.)

## notice.restart.item
- {name}
