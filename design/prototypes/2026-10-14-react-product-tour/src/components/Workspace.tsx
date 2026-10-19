import { DocumentContent } from "./DocumentContent";
import { Fragment } from "react";
import { useTour } from "../state/TourContext";
import { filtered, folderOf, crewLedgers } from "../domain/selectors";
import type { HumanAsk, Plan, TodoKind } from "../domain/types";
import { Button, Icon, Reference, StatusBadge } from "./primitives";
import { FolderRail } from "./FolderControls";
export function PlanTable({
  progress = false,
  source,
}: {
  progress?: boolean;
  source?: string;
}) {
  const { data, dispatch, reference } = useTour();
  let plans = data.db.plans.filter(
    (p) => !source || p.sources.includes(source),
  );
  if (!source) plans = filtered(data, plans, progress ? "progress" : "todo");
  const ids = new Set(plans.map((p) => p.id));
  function rows(p: Plan, depth: number): React.ReactNode {
    const children = plans.filter((c) => c.parent === p.id);
    return (
      <>
        <tr key={p.id} data-plan={p.id}>
          <td>
            <div className="plan-title" data-depth={depth}>
              {children.length ? (
                <Button
                  action="fold-plan"
                  data-id={p.id}
                  aria-label={
                    (data.ui.collapsed.includes(p.id) ? "展开" : "折叠") +
                    " " +
                    p.code
                  }
                  onClick={() => dispatch({ type: "foldPlan", id: p.id })}
                >
                  {data.ui.collapsed.includes(p.id) ? "▸" : "▾"}
                </Button>
              ) : (
                <span className="chevron" />
              )}
              <Button
                action="reference"
                data-id={p.id}
                className="plan-open"
                onClick={() => reference(p.id)}
              >
                {p.title}
                <span className="plan-code">
                  {p.code} · {p.id}
                </span>
              </Button>
            </div>
          </td>
          <td>
            <StatusBadge status={p.status} />
          </td>
          <td>{p.owner || "待认领"}</td>
          {progress ? (
            <>
              <td className="plan-note">{p.note || "—"}</td>
              <td>
                {p.blockedOn.length
                  ? p.blockedOn.map((id) => <Reference key={id} id={id} />)
                  : p.nextCheck || "—"}
              </td>
            </>
          ) : (
            <td>
              {p.dependsOn.length
                ? p.dependsOn.map((id) => <Reference key={id} id={id} />)
                : "—"}
            </td>
          )}
        </tr>
        {!data.ui.collapsed.includes(p.id) &&
          children.map((c) => (
            <Fragment key={c.id}>{rows(c, depth + 1)}</Fragment>
          ))}
      </>
    );
  }
  if (!plans.length) return <div className="empty">这个文件夹还没有计划。</div>;
  return (
    <>
      {progress && (
        <div className="progress-summary">
          <strong>工作结构</strong>
          <span>{plans.length} 个计划项</span>
          <span>
            {plans.filter((p) => p.status === "active").length} 进行中
          </span>
          <span>{plans.filter((p) => p.status === "done").length} 已完成</span>
          <span>
            {plans.filter((p) => p.status === "blocked").length} 等待判断
          </span>
          <Button
            action="expand-plans"
            onClick={() => dispatch({ type: "ui", patch: { collapsed: [] } })}
          >
            全部展开
          </Button>
          <Button
            action="collapse-plans"
            onClick={() =>
              dispatch({
                type: "ui",
                patch: {
                  collapsed: plans
                    .filter((p) => plans.some((c) => c.parent === p.id))
                    .map((p) => p.id),
                },
              })
            }
          >
            收起层级
          </Button>
        </div>
      )}
      <div className="table-wrap">
        <table className="plan-table">
          <thead>
            <tr>
              <th>计划 · 最多三层</th>
              <th>状态</th>
              <th>负责人</th>
              {progress ? (
                <>
                  <th>下一步／备注</th>
                  <th>卡点／下一检查</th>
                </>
              ) : (
                <th>依赖</th>
              )}
            </tr>
          </thead>
          <tbody>
            {plans
              .filter((p) => !p.parent || !ids.has(p.parent))
              .map((p) => (
                <Fragment key={p.id}>{rows(p, 0)}</Fragment>
              ))}
          </tbody>
        </table>
      </div>
      <p className="table-caption">
        {progress
          ? "点击标题查看目标、完成层级、验收标准和产物；这里与 Todo 的计划共用一份数据。"
          : "层级表示拆解，依赖单独显示。文件夹只负责分类。"}
      </p>
    </>
  );
}
export function AskCard({ ask, origin }: { ask: HumanAsk; origin?: string }) {
  const { state, commit, setModal } = useTour();
  const sceneId = origin ?? state.sceneId;
  const finish = (kind: "answer" | "askLater" | "askWithdraw", text = "") => {
    if (
      commit(
        kind === "answer"
          ? { type: kind, id: ask.id, text, sceneId }
          : { type: kind, id: ask.id, sceneId },
      )
    )
      setModal(null);
  };
  return (
    <section className="ask-card">
      <span className="badge blocked">
        {ask.id} ·{" "}
        {ask.status === "open"
          ? "等待你的答复"
          : ask.status === "later"
            ? "稍后处理"
            : ask.status === "answered"
              ? "已答复"
              : "已撤回"}
      </span>
      <h4>{ask.question}</h4>
      <p>{ask.context}</p>
      <Reference id={ask.item} />
      {["open", "later"].includes(ask.status) ? (
        <>
          <p style={{ marginTop: 12 }}>推荐：{ask.recommendation}</p>
          <div className="ask-actions">
            {ask.options.map((o, i) => (
              <Button
                key={o}
                action="answer"
                data-id={ask.id}
                data-scene={sceneId}
                data-choice={i}
                className={i === 0 ? "recommended" : ""}
                onClick={() => finish("answer", o)}
              >
                {o}
              </Button>
            ))}
            <Button
              action="ask-later"
              data-id={ask.id}
              data-scene={sceneId}
              onClick={() => finish("askLater")}
            >
              稍后
            </Button>
            <Button
              action="ask-withdraw"
              data-id={ask.id}
              data-scene={sceneId}
              onClick={() => finish("askWithdraw")}
            >
              不要了
            </Button>
          </div>
        </>
      ) : (
        <p style={{ marginTop: 12 }}>
          {ask.answer || "已经撤回，不再等待答复。"}
        </p>
      )}
    </section>
  );
}
function Todos() {
  const {
    state,
    data: { db, ui },
    data,
    setUi,
    setModal,
    reference,
  } = useTour();
  return (
    <>
      <div className="segmented todo-kind">
        {(
          [
            ["requests", "原始需求"],
            ["plans", "计划"],
            ["asks", "待人类处理"],
          ] as [TodoKind, string][]
        ).map(([kind, label]) => (
          <Button
            key={kind}
            action="todo-kind"
            data-kind={kind}
            className={ui.kind === kind ? "active" : ""}
            onClick={() => setUi({ kind })}
          >
            {label}
          </Button>
        ))}
      </div>
      {ui.kind === "plans" ? (
        <PlanTable />
      ) : ui.kind === "asks" ? (
        <>
          <div className="segmented todo-kind">
            {(["crew", "all"] as const).map((scope) => (
              <Button
                key={scope}
                action="ask-scope"
                data-scope={scope}
                className={ui.askScope === scope ? "active" : ""}
                onClick={() => setUi({ askScope: scope })}
              >
                {scope === "crew" ? "当前机组" : "全部机组 · 唯一收件箱"}
              </Button>
            ))}
          </div>
          {ui.askScope === "all"
            ? state.scenes.flatMap((s) =>
                crewLedgers(state.cache[s.id])
                  .flatMap(([, db]) => db.asks)
                  .filter((a) => ["open", "later"].includes(a.status))
                  .map((a) => (
                    <div key={a.id}>
                      <p
                        className="quiet"
                        style={{ fontSize: 11, margin: "12px 0 7px" }}
                      >
                        来源：{s.crew}
                      </p>
                      <AskCard ask={a} origin={s.id} />
                    </div>
                  )),
              )
            : filtered(data, db.asks, "todo").map((a) => (
                <AskCard key={a.id} ask={a} />
              ))}
        </>
      ) : (
        filtered(data, db.requests, "todo").map((request) => {
          const roots = db.plans.filter(
            (p) => p.sources.includes(request.id) && !p.parent,
          );
          return (
            <section key={request.id} className="request-card">
              <div className="request-top">
                <strong>{request.code} 原始需求</strong>
                <StatusBadge
                  status={
                    roots.length
                      ? roots.every((p) => p.status === "done")
                        ? "done"
                        : "active"
                      : "open"
                  }
                />
              </div>
              <Button
                action="reference"
                data-id={request.id}
                className="original-text request-open"
                onClick={() => reference(request.id)}
              >
                {request.text}
              </Button>
              {roots.map((p) => (
                <Reference key={p.id} id={p.id} />
              ))}
              <details open>
                <summary>关联计划 · 展开三层</summary>
                <PlanTable source={request.id} />
              </details>
              <div style={{ marginTop: 10 }}>
                <Button
                  action="move"
                  data-id={request.id}
                  data-scope="todo"
                  onClick={() =>
                    setModal({ kind: "move", id: request.id, scope: "todo" })
                  }
                >
                  移动到文件夹
                </Button>
              </div>
            </section>
          );
        })
      )}
    </>
  );
}
function Documents() {
  const { data, state, setUi, setModal, reference, dispatch } = useTour();
  const d = data.db.docs.find((d) => d.id === data.ui.doc);
  return d ? (
    <div className="doc-reader">
      <Button action="back-documents" onClick={() => setUi({ doc: null })}>
        ← 返回文档
      </Button>
      <h3>{d.title}</h3>
      <div className="byline">
        {d.author} · {d.kind ?? "产品说明"} · {folderOf(data.db, d, "document")}
      </div>
      {d.summary && <p className="document-summary">{d.summary}</p>}
      {d.kind === "已敲定设计" && (
        <p className="document-status">
          设计约定，不代表所有安装版本均已交付。
        </p>
      )}
      {d.kind === "规划设想" && (
        <p className="document-status">
          后续方向或契约草案，当前网页仅展示说明。
        </p>
      )}
      <DocumentContent body={d.body} />
      {d.related?.length ? (
        <section className="document-related">
          <h4>相关文档</h4>
          {d.related.map((id) => (
            <Reference key={id} id={id} />
          ))}
        </section>
      ) : null}
      {d.sources?.length ? (
        <details className="document-sources">
          <summary>依据与来源 · {d.sources.length}</summary>
          {d.sources.map((source) => (
            <a
              key={source.path}
              href={
                "https://github.com/syncmeta/PendingCrew/blob/main/" +
                source.path.split("/").map(encodeURIComponent).join("/")
              }
              target="_blank"
              rel="noopener noreferrer"
            >
              {source.label} ↗
            </a>
          ))}
        </details>
      ) : null}
      {state.sceneId === "research" && (
        <p>
          <a
            href="https://docs.pendingname.com/pendingcrew"
            target="_blank"
            rel="noopener"
          >
            完整使用文档 ↗
          </a>
          　
          <a
            href="https://github.com/syncmeta/PendingCrew/releases"
            target="_blank"
            rel="noopener"
          >
            下载 PendingCrew ↗
          </a>
        </p>
      )}
      <Reference id={data.db.plans[0].id} />
      <Button
        action="share-document"
        className="link-pill"
        onClick={() => {
          dispatch({ type: "shareReference", id: d.id });
          setUi({ tab: "message", mobileChat: true });
        }}
      >
        引用到当前群
      </Button>
      <Button
        action="move"
        data-id={d.id}
        data-scope="document"
        onClick={() => setModal({ kind: "move", id: d.id, scope: "document" })}
      >
        移动到文件夹
      </Button>
    </div>
  ) : (
    <>
      <div className="document-library-note">
        本群文档 · {filtered(data, data.db.docs, "document").length} 份
        <span>产品说明、设计约定与规划资料</span>
      </div>
      {filtered(data, data.db.docs, "document").map((d) => (
        <Button
          key={d.id}
          action="reference"
          data-id={d.id}
          className="doc-row"
          onClick={() => reference(d.id)}
        >
          <span className="doc-symbol">
            <Icon name="file" />
          </span>
          <span>
            {d.title}
            <small>
              {d.kind ?? "产品说明"} · {folderOf(data.db, d, "document")}
              {d.summary && (
                <span className="document-row-summary">{d.summary}</span>
              )}
            </small>
          </span>
        </Button>
      ))}
    </>
  );
}
export function Workspace() {
  const {
    data: { ui },
    setModal,
  } = useTour();
  const title =
    ui.selectedFolders[ui.tab] === "all"
      ? {
          message: "消息",
          todo: "Todo",
          document: "本群文档",
          progress: "本群进展",
        }[ui.tab]
      : ui.selectedFolders[ui.tab];
  return (
    <>
      <FolderRail />
      <article className="workspace">
        <header className="workspace-head">
          <div>
            <h3>{title}</h3>
            <p>
              {ui.tab === "progress"
                ? "同一份计划的进展视图 · 三层可折叠"
                : ui.tab === "todo"
                  ? "一本账 · 原始需求、计划、待人类处理"
                  : "说明、笔记与工作产物"}
            </p>
          </div>
          <div>
            {ui.tab === "todo" ? (
              <Button
                action="new-request"
                onClick={() => setModal({ kind: "newRequest" })}
              >
                ＋ 原始需求
              </Button>
            ) : ui.tab === "progress" ? (
              <Button
                action="history"
                onClick={() => setModal({ kind: "history" })}
              >
                更新记录
              </Button>
            ) : (
              <Button
                action="new-doc"
                onClick={() => setModal({ kind: "newDoc" })}
              >
                ＋ 文档
              </Button>
            )}
          </div>
        </header>
        <div className="workspace-body">
          {ui.tab === "todo" ? (
            <Todos />
          ) : ui.tab === "progress" ? (
            <PlanTable progress />
          ) : (
            <Documents />
          )}
        </div>
      </article>
    </>
  );
}
