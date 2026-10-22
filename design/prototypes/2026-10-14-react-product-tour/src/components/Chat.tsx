import { productText } from "../domain/displayText";
import { useLayoutEffect, useRef, useState } from "react";
import { useTour } from "../state/TourContext";
import { folderOf } from "../domain/selectors";
import { Avatar, Button, Icon, Reference } from "./primitives";
import { Inspector } from "./Inspector";
import { MessageText } from "./MessageText";
export function Chat() {
  const {
    data: { db, ui },
    scene,
    state,
    setUi,
    setModal,
    dispatch,
  } = useTour();
  const conv =
    db.conversations.find((c) => c.id === ui.conv) ?? db.conversations[0];
  const messages = db.messages[conv.id] ?? [];
  const visibleMessages = messages.filter(
    (m) =>
      (ui.selectedFolders.message === "all" ||
        folderOf(db, m, "message") === ui.selectedFolders.message) &&
      (!ui.query || m.text.includes(ui.query)) &&
      (!ui.mentions ||
        m.sender === "你" ||
        m.ref?.startsWith("H") ||
        m.text.includes("@你")) &&
      (ui.mode === "all" || !m.internal),
  );
  const scroll = useRef<HTMLDivElement>(null),
    textarea = useRef<HTMLTextAreaElement>(null),
    last = useRef({ conv: "", count: 0 });
  useLayoutEffect(() => {
    const el = scroll.current;
    if (!el) return;
    if (last.current.conv !== conv.id)
      el.scrollTop = ui.scrolls[conv.id] ?? el.scrollHeight;
    else if (messages.length > last.current.count)
      el.scrollTop = el.scrollHeight;
    last.current = { conv: conv.id, count: messages.length };
  }, [conv.id, messages.length, ui.scrolls]);
  const [todoMode, setTodoMode] = useState(false);
  const [showActions, setShowActions] = useState(false);
  const draft = ui.drafts[conv.id] ?? "";
  const send = () => {
    if (!draft.trim()) return;
    if (todoMode) {
      dispatch({ type: "newRequest", text: draft });
      setUi({ tab: "message", drafts: { ...ui.drafts, [conv.id]: "" } });
    } else dispatch({ type: "send", text: draft });
  };
  return (
    <>
      <article className="chat">
        <div className="chat-top">
          <div>
            <Button
              action="back-conversations"
              className="icon-button mobile-back"
              aria-label="返回会话列表"
              onClick={() => setUi({ mobileChat: false })}
            >
              <Icon name="arrow" />
            </Button>
            <h3 title={productText(conv.title)}>{productText(conv.title)}</h3>
          </div>
          <div className="chat-toolbar">
            <div className="chat-options">
              <div className="segmented">
                {(["human", "all"] as const).map((mode) => (
                  <Button
                    key={mode}
                    action="communication"
                    data-mode={mode}
                    className={ui.mode === mode ? "active" : ""}
                    onClick={() => setUi({ mode })}
                  >
                    {mode === "human" ? "人类沟通" : "完整记录"}
                  </Button>
                ))}
              </div>
              <Button
                action="mentions"
                className={"text-button " + (ui.mentions ? "on" : "")}
                aria-pressed={ui.mentions}
                onClick={() => setUi({ mentions: !ui.mentions })}
              >
                仅@你
              </Button>
            </div>
            {state.sceneId !== "website" && db.plans.length > 0 && (
              <>
                <Button
                  action="simulate"
                  title="更新计划状态与关联记录"
                  onClick={() => dispatch({ type: "simulate" })}
                >
                  更新计划
                </Button>
              </>
            )}
            <Button
              action="inspector"
              className="icon-button"
              aria-label={(ui.inspector ? "收起" : "展开") + " 待办 和成员"}
              title={(ui.inspector ? "收起" : "展开") + " 待办 和成员"}
              onClick={() => setUi({ inspector: !ui.inspector })}
            >
              <Icon name="panel" />
            </Button>
          </div>
        </div>
        <div
          className="messages"
          id="messages"
          ref={scroll}
          onScroll={(e) => {
            const top = e.currentTarget.scrollTop;
            if (ui.scrolls[conv.id] !== top)
              setUi({ scrolls: { ...ui.scrolls, [conv.id]: top } });
          }}
        >
          <div className="time-separator">今天 13:20</div>
          {visibleMessages.map((m, index) => (
            <div
              key={m.id}
              className={
                "message " +
                (m.sender === "你" ? "own" : "") +
                (index > 0 && visibleMessages[index - 1].sender === m.sender
                  ? " consecutive"
                  : "")
              }
            >
              {m.sender !== "你" &&
                !(
                  index > 0 && visibleMessages[index - 1].sender === m.sender
                ) && (
                  <Avatar
                    seed={m.sender}
                    glyph={
                      conv.members?.find((member) => member.name === m.sender)
                        ?.glyph ??
                      (m.sender === scene.agent ? scene.glyph : "🦉")
                    }
                  />
                )}
              <div className="message-body">
                {!(
                  index > 0 && visibleMessages[index - 1].sender === m.sender
                ) && <div className="sender">{productText(m.sender)}</div>}
                <div className="bubble">
                  <MessageText text={m.text} />
                  {m.link && (
                    <a
                      className="ref-link"
                      href={m.link.href}
                      target="_blank"
                      rel="noopener"
                    >
                      {m.link.label}
                    </a>
                  )}
                  {m.ref && <Reference id={m.ref} className="ref-link" />}
                </div>
                <Button
                  action="move"
                  data-id={m.id}
                  data-scope="message"
                  className="text-button message-menu"
                  aria-label="整理这条消息"
                  onClick={() =>
                    setModal({ kind: "move", id: m.id, scope: "message" })
                  }
                >
                  <Icon name="more" />
                </Button>
              </div>
            </div>
          ))}
          {messages.filter(
            (m) =>
              (ui.selectedFolders.message === "all" ||
                folderOf(db, m, "message") === ui.selectedFolders.message) &&
              (!ui.query || m.text.includes(ui.query)) &&
              (!ui.mentions ||
                m.sender === "你" ||
                m.ref?.startsWith("H") ||
                m.text.includes("@你")) &&
              (ui.mode === "all" || !m.internal),
          ).length === 0 && (
            <div className="empty">
              这个范围还没有消息。
              <br />
              <Button
                action="clear-filters"
                onClick={() =>
                  setUi({
                    mentions: false,
                    query: "",
                    selectedFolders: { ...ui.selectedFolders, message: "all" },
                  })
                }
              >
                看全部
              </Button>
            </div>
          )}
        </div>
        <form
          className="composer"
          id="composer"
          onSubmit={(e) => {
            e.preventDefault();
            send();
          }}
        >
          {showActions && (
            <div className="composer-actions">
              <Button
                action="attachment"
                onClick={() => {
                  setModal({ kind: "references" });
                  setShowActions(false);
                }}
              >
                <Icon name="file" />
                引用文档或计划
              </Button>
              <Button
                action="mention"
                onClick={() => {
                  setUi({
                    drafts: {
                      ...ui.drafts,
                      [conv.id]: draft + "@" + scene.agent + " ",
                    },
                  });
                  setShowActions(false);
                  textarea.current?.focus();
                }}
              >
                @ 提及成员
              </Button>
              <Button
                action="new-request"
                onClick={() => {
                  setModal({ kind: "newRequest" });
                  setShowActions(false);
                }}
              >
                新建 待办
              </Button>
            </div>
          )}
          <div className="composer-row">
            <Button
              action="composer-actions"
              className="composer-circle"
              aria-label="更多输入操作"
              aria-expanded={showActions}
              onClick={() => setShowActions(!showActions)}
            >
              {showActions ? "×" : <Icon name="plus" />}
            </Button>
            <Button
              action="todo-mode"
              className={"composer-circle " + (todoMode ? "active" : "")}
              aria-label="待办 模式"
              aria-pressed={todoMode}
              title="待办 模式：点亮后发送 = 记一条 待办"
              onClick={() => setTodoMode(!todoMode)}
            >
              <Icon name="checklist" />
            </Button>
            <div className="composer-capsule">
              <textarea
                id="draft"
                ref={textarea}
                aria-label="输入消息"
                placeholder={todoMode ? "记一条 待办…" : "输入消息…"}
                value={draft}
                onChange={(e) =>
                  setUi({ drafts: { ...ui.drafts, [conv.id]: e.target.value } })
                }
                onKeyDown={(e) => {
                  if (
                    e.key === "Enter" &&
                    !e.shiftKey &&
                    !e.nativeEvent.isComposing
                  ) {
                    e.preventDefault();
                    send();
                  }
                }}
              />
              <button
                type="submit"
                className="composer-send"
                aria-label={todoMode ? "发送 待办" : "发送"}
                disabled={!draft.trim()}
              >
                <Icon name="send" />
              </button>
            </div>
          </div>
        </form>
      </article>
      <Inspector />
    </>
  );
}
