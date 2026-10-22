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
          ["Todo介绍", "Claude Code Session", "🦊"],
          ["人", "人类", "👤"],
        ];
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
                  kind:
                    state.sceneId === "website"
                      ? "requests"
                      : db.asks.some((a) =>
                            ["open", "later"].includes(a.status),
                          )
                        ? "asks"
                        : "plans",
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
          <h4 className="inspector-todo-section">给真人 <span>{db.asks.filter((a)=>["open","later"].includes(a.status)).length}</span></h4>
          {db.asks.filter((a)=>["open","later"].includes(a.status)).map((ask)=><Button key={ask.id} className="mini-todo" onClick={()=>reference(ask.id)}><span className="todo-status-label human">待答复</span><span>{ask.question}</span></Button>)}
          {!db.asks.some((a)=>["open","later"].includes(a.status)) && <p className="inspector-empty">暂无待处理事项</p>}
          <h4 className="inspector-todo-section">给 AI <span>{db.requests.filter((r)=>!["done","dropped"].includes(requestStatus(db,r))).length}</span></h4>
          {db.requests.map((request)=><Button key={request.id} action="reference" data-id={request.id} className="mini-todo" onClick={()=>reference(request.id)}><span className={"state-ring "+requestStatus(db,request)}/><span><span className="todo-number">{request.code}</span>{request.text}<span className="sub">负责人：{request.owner ?? db.plans.find((p)=>p.sources.includes(request.id))?.owner ?? "机长"}</span></span></Button>)}
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
            本群成员 {members.length}
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
