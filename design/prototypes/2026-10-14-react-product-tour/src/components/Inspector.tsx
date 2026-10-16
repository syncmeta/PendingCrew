import { useTour } from "../state/TourContext";
import { statusLabels } from "../domain/types";
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
            本群 Todo
          </Button>
          <span>
            <Button
              action="todo-all"
              onClick={() =>
                setUi({
                  tab: "todo",
                  kind: state.sceneId === "website" ? "requests" : "asks",
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
        <div className="panel-content">
          <div className="quiet">同一账本的相关事项</div>
          {state.sceneId === "website" ? (
            db.requests.map((request) => (
              <Button
                key={request.id}
                action="reference"
                data-id={request.id}
                className="mini-todo"
                onClick={() => reference(request.id)}
              >
                <span className="todo-number">
                  <span className="state-ring" />
                  {request.code}
                </span>
                <span className="todo-body">
                  {request.text}
                  <span className="sub">原始需求</span>
                  <span className="todo-detail">放大看</span>
                </span>
              </Button>
            ))
          ) : (
            <>
              {db.asks
                .filter((a) => ["open", "later"].includes(a.status))
                .map((a) => (
                  <Button
                    key={a.id}
                    action="reference"
                    data-id={a.id}
                    className="mini-todo"
                    onClick={() => reference(a.id)}
                  >
                    <span className="state-ring blocked" />
                    <span>
                      {a.id} {a.question}
                      <div className="sub">
                        待人类处理 ·{" "}
                        {a.status === "later" ? "已稍后" : "等你答复"}
                      </div>
                    </span>
                  </Button>
                ))}
              {db.plans
                .filter((p) => !p.parent || p.parent === db.plans[0].id)
                .slice(0, 3)
                .map((p) => (
                  <Button
                    key={p.id}
                    action="reference"
                    data-id={p.id}
                    className="mini-todo"
                    onClick={() => reference(p.id)}
                  >
                    <span className={"state-ring " + p.status} />
                    <span>
                      {p.code} {p.title}
                      <div className="sub">
                        {statusLabels[p.status]} · {p.owner}
                      </div>
                    </span>
                  </Button>
                ))}
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
            <span className="chevron">{ui.membersClosed ? "▸" : "▾"}</span>{" "}
            本群成员 3
          </Button>
        </div>
        <div className="panel-content">
          {[
            ["机长", "Codex Session", "🦉"],
            ["Todo介绍", "Claude Code Session", "🦊"],
            ["人", "人类", "👤"],
          ].map(([name, sub, glyph]) => (
            <Button
              key={name}
              action="member"
              data-name={name}
              className="member-row"
              onClick={() => dispatch({ type: "member", name })}
            >
              <Avatar seed={name} glyph={glyph} />
              <span>
                <span className="name">{name}</span>
                <span className="sub" style={{ display: "block" }}>
                  {sub}
                </span>
              </span>
            </Button>
          ))}
        </div>
      </div>
    </aside>
  );
}
