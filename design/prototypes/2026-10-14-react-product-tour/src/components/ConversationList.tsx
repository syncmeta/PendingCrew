import { productText } from "../domain/displayText";
import { conversationTodoCounts } from "../domain/todoModel";
import { useTour } from "../state/TourContext";
import { Avatar, Button } from "./primitives";
import { ResizeHandle } from "./ResizeHandle";
export function ConversationList() {
  const {
    data,
    data: { db, ui },
    scene,
    setUi,
  } = useTour();
  const conv =
    db.conversations.find((c) => c.id === ui.conv) ?? db.conversations[0];
  return (
    <aside className="conversation-list">
      <div className="list-filter">会话</div>
      <div className="conv-scroll">
        {db.conversations
          .filter(
            (c) =>
              !c.id.includes("-session-") ||
              c.id === ui.conv ||
              db.messages[c.id]?.some((m) => m.id.startsWith("message-")),
          )
          .map((c) => {
            const counts = conversationTodoCounts(data, c.id);
            return (
              <Button
                key={c.id}
                action="conversation"
                data-id={c.id}
                className={"conv-row " + (c.id === conv.id ? "selected" : "")}
                onClick={() =>
                  setUi({ conv: c.id, mobileChat: true, query: "" })
                }
              >
                <span className="conversation-avatar">
                  <Avatar
                    seed={c.id}
                    group={c.kind === "group"}
                    glyph={
                      c.members?.find((member) => member.name === c.title)
                        ?.glyph ?? scene.glyph
                    }
                  />
                  {counts.red > 0 && (
                    <span
                      className="conversation-badge red"
                      aria-label={`${counts.red} 个AI待办`}
                    >
                      {counts.red > 99 ? "99+" : counts.red}
                    </span>
                  )}
                  {counts.yellow > 0 && (
                    <span
                      className="conversation-badge yellow"
                      aria-label={`${counts.yellow} 个人待办`}
                    >
                      {counts.yellow > 99 ? "99+" : counts.yellow}
                    </span>
                  )}
                </span>
                <span className="conv-content">
                  <span className="conv-name">
                    <span className="conv-title">{productText(c.title)}</span>
                    <small>{c.id === conv.id ? "13:20" : "13:10"}</small>
                  </span>
                  <span className="conv-sub">
                    {productText(db.messages[c.id]?.at(-1)?.text ?? c.subtitle)
                      .replace(/\*\*/g, "")
                      .replace(/\n+/g, " ")}
                  </span>
                </span>
              </Button>
            );
          })}
      </div>
      <ResizeHandle name="conversation" />
    </aside>
  );
}
