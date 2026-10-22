import { productText } from "../domain/displayText";
import { useTour } from "../state/TourContext";
import { requestStatus } from "../domain/todoModel";
import { Avatar, Button } from "./primitives";
import { ResizeHandle } from "./ResizeHandle";
export function Inspector() {
  const {
    state,
    data: { db, ui },
    setUi,
    dispatch,
    reference,
  } = useTour();
  const members = db.conversations.find((c) => c.id === ui.conv)?.members
    ? db.conversations
        .find((c) => c.id === ui.conv)!
        .members!.map((m) => [m.name, m.detail, m.glyph])
    : state.sceneId === "self-introduction"
      ? [["人", "个人介绍", "👤"]]
      : [
          ["机长", "Codex Session", "🦉"],
          ["待办介绍", "Claude Code Session", "🦊"],
          ["人", "人类", "👤"],
        ];
  const kind = ui.inspectorKind ?? "requests";
  const asks = db.asks.filter((ask) => ["open", "later"].includes(ask.status));
  const requests = db.requests.filter(
    (request) => !["done", "dropped"].includes(requestStatus(db, request)),
  );
  return (
    <aside
      className={"inspector " + (ui.inspector ? "force-visible" : "hidden")}
    >
      <div className={"panel todo-panel " + (ui.todoClosed ? "closed" : "")}>
        <div className="panel-head">
          <Button
            action="fold-todo"
            onClick={() => setUi({ todoClosed: !ui.todoClosed })}
          >
            待办
          </Button>
          <span>
            <Button
              action="todo-all"
              onClick={() =>
                setUi({
                  tab: "todo",
                  kind,
                  query: "",
                  todoTag: "all",
                  selectedFolders: { ...ui.selectedFolders, todo: "all" },
                })
              }
            >
              详细
            </Button>
            <Button
              action="inspector"
              className="text-button inspector-close"
              aria-label="收起辅助面板"
              onClick={() => setUi({ inspector: false })}
            >
              ×
            </Button>
          </span>
        </div>
        <div
          className="inspector-kind segmented"
          role="tablist"
          aria-label="待办对象"
        >
          {(
            [
              ["asks", "人"],
              ["requests", "AI"],
            ] as const
          ).map(([value, label]) => (
            <Button
              key={value}
              role="tab"
              action="inspector-kind"
              data-kind={value}
              aria-selected={kind === value}
              className={kind === value ? "active" : ""}
              onClick={() => setUi({ inspectorKind: value })}
            >
              {label}
              <span>{value === "asks" ? asks.length : requests.length}</span>
            </Button>
          ))}
        </div>
        <div
          className="panel-content"
          role="tabpanel"
          aria-label={kind === "asks" ? "人待办" : "AI待办"}
        >
          {kind === "asks" ? (
            <>
              {asks.map((ask) => (
                <Button
                  key={ask.id}
                  action="reference"
                  data-id={ask.id}
                  className="mini-todo"
                  onClick={() => reference(ask.id)}
                >
                  <span className="todo-status-label human">待答复</span>
                  <span>{productText(ask.question)}</span>
                </Button>
              ))}
              {!asks.length && (
                <p className="inspector-empty">暂无待处理事项</p>
              )}
            </>
          ) : (
            <>
              {requests.map((request) => (
                <Button
                  key={request.id}
                  action="reference"
                  data-id={request.id}
                  className="mini-todo"
                  onClick={() => reference(request.id)}
                >
                  <span
                    className={"state-ring " + requestStatus(db, request)}
                  />
                  <span>
                    <span className="todo-number">{request.code}</span>
                    {productText(request.text)}
                    <span className="sub">
                      负责人：
                      {productText(
                        request.owner ??
                          db.plans.find((p) => p.sources.includes(request.id))
                            ?.owner ??
                          "机长",
                      )}
                    </span>
                  </span>
                </Button>
              ))}
              {!requests.length && (
                <p className="inspector-empty">暂无待处理事项</p>
              )}
            </>
          )}
        </div>
      </div>
      <ResizeHandle name="todo-height" />
      <div
        className={"panel member-panel " + (ui.membersClosed ? "closed" : "")}
      >
        <div className="panel-head">
          <Button
            action="fold-members"
            onClick={() => setUi({ membersClosed: !ui.membersClosed })}
          >
            <span className="chevron">{ui.membersClosed ? "▸" : "▾"}</span> 成员{" "}
            {members.length}
          </Button>
        </div>
        <div className="panel-content">
          {members.map(([name, sub, glyph]) => (
            <Button
              key={name}
              action="member"
              data-name={name}
              className="member-row"
              onClick={() => dispatch({ type: "member", name })}
            >
              <Avatar seed={name} glyph={glyph} />
              <span>
                <span className="name">{productText(name)}</span>
                <span className="sub" style={{ display: "block" }}>
                  {productText(sub)}
                </span>
              </span>
            </Button>
          ))}
        </div>
      </div>
    </aside>
  );
}
