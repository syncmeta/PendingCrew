import { useEffect, useRef, useState, type ReactNode } from "react";
import { useTour } from "../state/TourContext";
import { findReference, referenceLabel } from "../domain/selectors";
import {
  scopeNames,
  statusLabels,
  type ModalState,
  type Plan,
  type Status,
  type Scope,
} from "../domain/types";
import { Button, Icon, Reference } from "./primitives";
import { AskCard } from "./Workspace";
function Dialog({
  title,
  children,
  actions,
}: {
  title: string;
  children: ReactNode;
  actions?: ReactNode;
}) {
  const { setModal, state } = useTour();
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    if (ref.current && !ref.current.open) ref.current.showModal();
  }, []);
  return (
    <dialog
      id="modal"
      ref={ref}
      onCancel={() => setModal(null)}
      onClose={() => setModal(null)}
      onClick={(e) => {
        if (e.target !== e.currentTarget) return;
        const box = e.currentTarget.getBoundingClientRect();
        if (
          e.clientX < box.left ||
          e.clientX > box.right ||
          e.clientY < box.top ||
          e.clientY > box.bottom
        )
          setModal(null);
      }}
    >
      <div id="modal-content">
        <div className="dialog-head">
          <h3>{title}</h3>
          <Button
            action="close"
            className="icon-button"
            aria-label="关闭详情"
            style={{ transform: "rotate(45deg)" }}
            onClick={() => setModal(null)}
          >
            <Icon name="plus" />
          </Button>
        </div>
        <div className="dialog-body">
          {children}
          {state.notice && (
            <p role="status" className="quiet" style={{ marginTop: 12 }}>
              {state.notice.text}
            </p>
          )}
        </div>
        {actions && <div className="dialog-actions">{actions}</div>}
      </div>
    </dialog>
  );
}
function PlanDialog({ plan }: { plan: Plan }) {
  const {
    data: { db, ui },
    scene,
    commit,
    setModal,
  } = useTour();
  const [status, setStatus] = useState(plan.status),
    [owner, setOwner] = useState(plan.owner),
    [note, setNote] = useState(plan.note);
  return (
    <Dialog
      title={plan.code + " · " + plan.title}
      actions={
        <>
          <Button
            action="move"
            data-id={plan.id}
            data-scope={ui.tab === "todo" ? "todo" : "progress"}
            onClick={() =>
              setModal({
                kind: "move",
                id: plan.id,
                scope: ui.tab === "todo" ? "todo" : "progress",
              })
            }
          >
            移动到文件夹
          </Button>
          <Button
            action="save-plan"
            data-id={plan.id}
            className="primary"
            onClick={() => {
              if (
                commit({ type: "savePlan", id: plan.id, status, owner, note })
              )
                setModal(null);
            }}
          >
            保存修改
          </Button>
        </>
      }
    >
      <div className="quiet" style={{ fontSize: 11 }}>
        稳定编号 {plan.id} · 与 Todo／进展引用同一份记录
      </div>
      <div className="dialog-fields" style={{ marginTop: 16 }}>
        <label>
          状态
          <select
            id="plan-status"
            value={status}
            onChange={(e) => setStatus(e.target.value as Status)}
          >
            {(
              [
                "open",
                "active",
                "done",
                ...(plan.status === "blocked" ? ["blocked"] : []),
              ] as Status[]
            ).map((s) => (
              <option key={s} value={s}>
                {statusLabels[s]}
              </option>
            ))}
          </select>
        </label>
        <label>
          负责人
          <select
            id="plan-owner"
            value={owner}
            onChange={(e) => setOwner(e.target.value)}
          >
            {["机长", scene.agent].map((o) => (
              <option key={o}>{o}</option>
            ))}
          </select>
        </label>
      </div>
      <h4>目标</h4>
      <p>{plan.goal || db.plans[0].goal}</p>
      <h4>完成层级与验收标准</h4>
      <p>本地方案 · {plan.acceptance || db.plans[0].acceptance}</p>
      <h4>下一步／备注（覆盖更新）</h4>
      <textarea
        id="plan-note"
        aria-label="计划备注"
        value={note}
        onChange={(e) => setNote(e.target.value)}
      />
      <h4>来源、依赖与卡点</h4>
      <p>
        {plan.sources.map((id) => (
          <Reference key={id} id={id} />
        ))}{" "}
        {plan.dependsOn.map((id) => (
          <Reference key={id} id={id} />
        ))}{" "}
        {plan.blockedOn.length
          ? plan.blockedOn.map((id) => <Reference key={id} id={id} />)
          : "没有等待人类的卡点"}
      </p>
      {plan.evidence && (
        <>
          <h4>产物</h4>
          <Reference id={plan.evidence} />
        </>
      )}
      <h4>下一检查</h4>
      <p>{plan.nextCheck}</p>
    </Dialog>
  );
}
function FolderDialog({ scope, old }: { scope: Scope; old?: string }) {
  const { commit, setModal } = useTour();
  const [name, setName] = useState(old ?? "");
  return (
    <Dialog
      title={old ? "重命名文件夹" : "新建文件夹"}
      actions={
        <Button
          action="save-folder"
          data-old={old}
          className="primary"
          onClick={() => {
            if (commit({ type: "saveFolder", scope, name, old }))
              setModal(null);
          }}
        >
          保存
        </Button>
      }
    >
      <label>
        文件夹名称
        <input
          id="folder-name"
          aria-label="文件夹名称"
          style={{ width: "100%", marginTop: 9 }}
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder="例如：本周讨论"
        />
      </label>
    </Dialog>
  );
}
function SearchDialog() {
  const {
    data: { db },
    reference,
  } = useTour();
  const [query, setQuery] = useState("");
  const items = [
    ...db.plans.map((p) => ({ id: p.id, title: p.title, kind: "计划" })),
    ...db.asks.map((a) => ({
      id: a.id,
      title: a.question,
      kind: "待人类处理",
    })),
    ...db.docs.map((d) => ({ id: d.id, title: d.title, kind: "文档" })),
    ...db.requests.map((r) => ({ id: r.id, title: r.text, kind: "原始需求" })),
  ];
  return (
    <Dialog title="搜索当前机组">
      <input
        id="modal-search"
        aria-label="搜索机组内容"
        placeholder="输入计划、文档或问题中的文字"
        style={{ width: "100%" }}
        autoFocus
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      <div id="search-results" style={{ marginTop: 15 }}>
        {items
          .filter((x) => x.title.includes(query))
          .map((x) => (
            <Button
              key={x.id}
              action="reference"
              data-id={x.id}
              className="search-result"
              onClick={() => reference(x.id)}
            >
              {x.title}
              <small>
                {x.kind} · {x.id}
              </small>
            </Button>
          ))}
      </div>
    </Dialog>
  );
}
function CreateDialog({ kind }: { kind: "newCrew" | "newRequest" | "newDoc" }) {
  const { commit, setModal } = useTour();
  const [title, setTitle] = useState(""),
    [body, setBody] = useState("");
  const save = () => {
    const ok = commit(
      kind === "newCrew"
        ? {
            type: "newCrew",
            name: title,
            brief: body,
            id: "custom-" + crypto.randomUUID(),
          }
        : kind === "newRequest"
          ? { type: "newRequest", text: body }
          : { type: "newDocument", title, body },
    );
    if (ok) setModal(null);
  };
  return (
    <Dialog
      title={
        kind === "newCrew"
          ? "新建机组"
          : kind === "newRequest"
            ? "新建原始需求"
            : "新建文档"
      }
      actions={
        <Button
          action={
            kind === "newCrew"
              ? "save-crew"
              : kind === "newRequest"
                ? "save-request"
                : "save-doc"
          }
          className="primary"
          onClick={save}
        >
          {kind === "newCrew"
            ? "创建机组"
            : kind === "newRequest"
              ? "保存原始需求"
              : "保存文档"}
        </Button>
      }
    >
      {kind === "newCrew" ? (
        <>
          <label>
            机组名称
            <input
              id="crew-name"
              aria-label="机组名称"
              style={{ width: "100%", marginTop: 8 }}
              placeholder="例如：读书计划"
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </label>
          <h4>要交给这个团队的事</h4>
          <textarea
            id="crew-brief"
            aria-label="机组任务"
            placeholder="写下一个想一起完成的目标"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </>
      ) : kind === "newRequest" ? (
        <>
          <p className="quiet">保留你交代的原文，计划通过引用关联到这里。</p>
          <textarea
            id="request-text"
            aria-label="原始需求内容"
            placeholder="你想让团队做什么？"
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </>
      ) : (
        <>
          <input
            id="doc-title"
            aria-label="文档名称"
            placeholder="文档名称"
            style={{ width: "100%" }}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
          />
          <textarea
            id="doc-body"
            aria-label="文档正文"
            placeholder="写下说明或笔记"
            style={{ marginTop: 12 }}
            value={body}
            onChange={(e) => setBody(e.target.value)}
          />
        </>
      )}
    </Dialog>
  );
}
function ModalContent({ modal }: { modal: NonNullable<ModalState> }) {
  const {
    state,
    scene,
    data: { db },
    setUi,
    setModal,
    commit,
    dispatch,
  } = useTour();
  switch (modal.kind) {
    case "request": {
      const ref = findReference(state, modal.id);
      return ref?.kind === "request" ? (
        <Dialog title={ref.record.code + " · Todo详情"}>
          <h4>原始需求</h4>
          <p>{ref.record.text}</p>
          <p className="quiet">
            这里保留需求原文。你可以持续添加
            Todo，相关计划和回复仍引用同一条记录。
          </p>
          <h4>相关计划</h4>
          {db.plans.filter((p) => p.sources.includes(ref.record.id)).length ? (
            db.plans
              .filter((p) => p.sources.includes(ref.record.id))
              .map((p) => <Reference key={p.id} id={p.id} />)
          ) : (
            <p className="quiet">尚未关联计划。</p>
          )}
        </Dialog>
      ) : null;
    }
    case "plan": {
      const ref = findReference(state, modal.id);
      return ref?.kind === "plan" ? <PlanDialog plan={ref.record} /> : null;
    }
    case "ask": {
      const ref = findReference(state, modal.id);
      return ref?.kind === "ask" ? (
        <Dialog title={"待人类处理 · " + modal.id}>
          <AskCard ask={ref.record} origin={ref.sceneId} />
        </Dialog>
      ) : null;
    }
    case "folderEdit":
      return <FolderDialog scope={modal.scope} old={modal.old} />;
    case "search":
      return <SearchDialog />;
    case "newCrew":
    case "newRequest":
    case "newDoc":
      return <CreateDialog kind={modal.kind} />;
    case "move":
      return (
        <Dialog title={"移动到" + scopeNames[modal.scope] + "文件夹"}>
          <p className="quiet">只改变分类，原始记录与引用不变。</p>
          <div className="dialog-list">
            {[...db.folders[modal.scope], "未分组"].map((folder) => (
              <Button
                key={folder}
                action="confirm-move"
                className=""
                data-id={modal.id}
                data-scope={modal.scope}
                data-folder={folder}
                onClick={() => {
                  if (
                    commit({
                      type: "move",
                      id: modal.id,
                      scope: modal.scope,
                      folder,
                    })
                  )
                    setModal(null);
                }}
              >
                <Icon name="folder" /> {folder}
              </Button>
            ))}
          </div>
        </Dialog>
      );
    case "folders":
      return (
        <Dialog
          title={"管理" + scopeNames[modal.scope] + "文件夹"}
          actions={
            <Button
              action="new-folder"
              className="primary"
              onClick={() =>
                setModal({ kind: "folderEdit", scope: modal.scope })
              }
            >
              ＋ 新建文件夹
            </Button>
          }
        >
          <p className="quiet">
            这个视图的分类独立于机组层级和其他视图。已有内容移动后仍保留原编号与引用。
          </p>
          <div className="dialog-list">
            {db.folders[modal.scope].map((folder) => (
              <Button
                key={folder}
                action="rename-folder"
                data-folder={folder}
                onClick={() =>
                  setModal({
                    kind: "folderEdit",
                    scope: modal.scope,
                    old: folder,
                  })
                }
              >
                <Icon name="folder" /> {folder}
                <small>点击重命名</small>
              </Button>
            ))}
          </div>
        </Dialog>
      );
    case "references":
      return (
        <Dialog title="引用工作记录">
          <div className="dialog-list">
            {[...db.docs, ...db.plans].map((record) => (
              <Button
                key={record.id}
                action="insert-reference"
                className=""
                data-id={record.id}
                onClick={() => {
                  dispatch({ type: "shareReference", id: record.id });
                  setModal(null);
                }}
              >
                {referenceLabel(state, record.id)}
              </Button>
            ))}
          </div>
        </Dialog>
      );
    case "history":
      return (
        <Dialog title="计划更新记录">
          {db.history
            .slice()
            .reverse()
            .map((h, i) => (
              <div key={i} className="timeline-entry">
                <time>{h.time}</time>
                <span>{h.text}</span>
              </div>
            ))}
        </Dialog>
      );
    case "reset":
      return (
        <Dialog
          title="重置当前 Demo"
          actions={
            <>
              <Button action="close" onClick={() => setModal(null)}>
                取消
              </Button>
              <Button
                id="confirm-reset"
                className="primary"
                onClick={() => {
                  dispatch({ type: "reset" });
                  setModal(null);
                }}
              >
                重置当前机组
              </Button>
            </>
          }
        >
          <p>
            清除你在「{scene.name}
            」里的消息、答复和修改，恢复预置产品介绍。其他机组不受影响。
          </p>
        </Dialog>
      );
    case "guide":
      return (
        <Dialog
          title="PendingCrew 的协作结构"
          actions={
            <Button
              action="start-guide"
              className="primary"
              onClick={() => {
                setModal(null);
                setUi({ tab: "todo", kind: "plans", collapsed: [] });
              }}
            >
              查看工作结构
            </Button>
          }
        >
          <p className="quiet">{scene.brief}</p>
          {[
            [
              "原始需求",
              "群里的 Todo 持续承接任务流，原文保留下来，工作随时可以追溯。",
            ],
            [
              "计划结构",
              "最多三层拆解，记录负责人、状态、依赖、下一步和验收标准。",
            ],
            [
              "人的判断",
              "需要人判断的事项保留上下文和推荐方案，集中进入同一个收件箱。",
            ],
            [
              "进展与成果",
              "进展与 Todo 引用同一份计划，文档沉淀说明和工作产物。",
            ],
          ].map(([title, text], i) => (
            <div key={title} className="guide-step">
              <span className="guide-number">{i + 1}</span>
              <div>
                <h4>{title}</h4>
                <p>{text}</p>
              </div>
            </div>
          ))}
          <p className="quiet" style={{ marginTop: 16 }}>
            消息、Todo、文档、进展各有自己的文件夹。操作保存在这个浏览器里，可以随时重置。
          </p>
        </Dialog>
      );
  }
}
export function Dialogs() {
  const { modal } = useTour();
  return modal ? (
    <ModalContent key={JSON.stringify(modal)} modal={modal} />
  ) : null;
}
