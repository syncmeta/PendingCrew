import { useTour } from "../state/TourContext";
import { Avatar, Button } from "./primitives";
import { ResizeHandle } from "./ResizeHandle";
export function ConversationList() {
  const {
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
        {db.conversations.map((c) => (
          <Button
            key={c.id}
            action="conversation"
            data-id={c.id}
            className={"conv-row " + (c.id === conv.id ? "selected" : "")}
            onClick={() => setUi({ conv: c.id, mobileChat: true, query: "" })}
          >
            <Avatar
              seed={c.id}
              group={c.kind === "group"}
              glyph={scene.glyph}
            />
            <span className="conv-content">
              <span className="conv-name">
                <span className="conv-title">{c.title}</span>
                <small>{c.id === conv.id ? "13:20" : "13:10"}</small>
              </span>
              <span className="conv-sub">
                {db.messages[c.id]?.at(-1)?.text ?? c.subtitle}
              </span>
            </span>
          </Button>
        ))}
      </div>
      <div className="conv-bottom">当前机组的群聊与单聊</div>
      <ResizeHandle name="conversation" />
    </aside>
  );
}
