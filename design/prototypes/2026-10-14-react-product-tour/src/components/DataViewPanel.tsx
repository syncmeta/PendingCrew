import { useState } from "react";
import { useTour } from "../state/TourContext";
import { crewLedgers } from "../domain/selectors";
import type {
  DataView,
  TableDataView,
  CompositionDataView,
} from "../domain/types";
import { Button, Reference } from "./primitives";
function RelatedData({ keys }: { keys: string[] }) {
  const { state } = useTour();
  const docs = Object.values(state.cache).flatMap((c) =>
    crewLedgers(c).flatMap(([, db]) => db.docs),
  );
  return (
    <div className="data-related">
      {keys.map((key) => {
        const d = docs.find((d) => d.catalogKey === key);
        return d ? <Reference key={key} id={d.id} /> : null;
      })}
    </div>
  );
}
function TableView({ value }: { value: TableDataView }) {
  const [query, setQuery] = useState(""),
    [sort, setSort] = useState(value.primaryKey),
    [descending, setDescending] = useState(false),
    [selected, setSelected] = useState<string | null>(null),
    [schema, setSchema] = useState(false);
  const cell = (row: TableDataView["rows"][number], key: string) =>
    row[key] === null ? "NULL" : String(row[key] ?? "");
  const rows = value.rows
    .filter((row) =>
      Object.values(row).some((v) =>
        String(v ?? "")
          .toLowerCase()
          .includes(query.toLowerCase()),
      ),
    )
    .sort(
      (a, b) =>
        cell(a, sort).localeCompare(cell(b, sort), "zh-CN") *
        (descending ? -1 : 1),
    );
  const row = value.rows.find((r) => cell(r, value.primaryKey) === selected);
  return (
    <section className="data-view">
      <header>
        <div>
          <strong>数据表</strong>
          <small>示意数据 · {value.rows.length} 条</small>
        </div>
        <Button
          action="data-schema"
          className="link-pill"
          aria-pressed={schema}
          onClick={() => setSchema(!schema)}
        >
          {schema ? "返回数据" : "表结构 / SQLite"}
        </Button>
      </header>
      <p>{value.description}</p>
      <label className="data-search">
        筛选
        <input
          aria-label="筛选数据"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="按名称、类型或负责人搜索"
        />
      </label>
      {schema ? (
        <div className="data-schema">
          <table>
            <thead>
              <tr>
                <th>字段</th>
                <th>类型</th>
              </tr>
            </thead>
            <tbody>
              {value.columns.map((c) => (
                <tr key={c.key}>
                  <td>{c.key}</td>
                  <td>{c.type}</td>
                </tr>
              ))}
            </tbody>
          </table>
          <pre>{value.schema}</pre>
          <small>表结构示意，不执行 SQL。</small>
        </div>
      ) : (
        <div className="data-table-scroll">
          <table className="data-table">
            <thead>
              <tr>
                {value.columns.map((c) => (
                  <th key={c.key}>
                    <Button
                      onClick={() => {
                        setSort(c.key);
                        setDescending(c.key === sort ? !descending : false);
                      }}
                      aria-label={"按" + c.label + "排序"}
                    >
                      {c.label}
                      {sort === c.key ? (descending ? " ↓" : " ↑") : ""}
                    </Button>
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr
                  key={cell(r, value.primaryKey)}
                  className={
                    cell(r, value.primaryKey) === selected ? "selected" : ""
                  }
                >
                  {value.columns.map((c, i) => (
                    <td key={c.key}>
                      {i === 0 ? (
                        <Button
                          className="data-row-select"
                          onClick={() => setSelected(cell(r, value.primaryKey))}
                        >
                          {cell(r, c.key)}
                        </Button>
                      ) : r[c.key] === null ? (
                        <code className="data-null">NULL</code>
                      ) : (
                        cell(r, c.key)
                      )}
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
          {!rows.length && <div className="empty">没有匹配的数据。</div>}
        </div>
      )}
      {row && (
        <aside className="data-detail">
          <strong>{String(row.name ?? row.title ?? selected)}</strong>
          <p>{String(row.detail ?? "")}</p>
          <RelatedData
            keys={Array.isArray(row.relatedKeys) ? row.relatedKeys : []}
          />
        </aside>
      )}
    </section>
  );
}
function CompositionView({
  value,
  onSave,
}: {
  value: CompositionDataView;
  onSave: (value: DataView) => void;
}) {
  const [viewId, setViewId] = useState(value.views[0]?.id),
    [selected, setSelected] = useState(value.blocks[0]?.id),
    [draft, setDraft] = useState(value.blocks[0]?.body ?? "");
  const view = value.views.find((v) => v.id === viewId),
    block = value.blocks.find((b) => b.id === selected);
  const select = (id: string) => {
    setSelected(id);
    setDraft(value.blocks.find((b) => b.id === id)?.body ?? "");
  };
  const usages = value.views.flatMap((v) =>
    v.placements
      .filter((p) => p.blockId === selected)
      .map((p) => ({ view: v.title, id: p.id })),
  );
  return (
    <section className="data-view composition-view">
      <header>
        <strong>内容块与文档组合</strong>
        <small>本地交互示例</small>
      </header>
      <p>{value.description}</p>
      <div className="segmented composition-tabs">
        {value.views.map((v) => (
          <Button
            key={v.id}
            action="composition-view"
            data-id={v.id}
            className={v.id === viewId ? "active" : ""}
            onClick={() => {
              setViewId(v.id);
              if (v.placements[0]) select(v.placements[0].blockId);
            }}
          >
            {v.title}
          </Button>
        ))}
      </div>
      <div className="composition-blocks">
        {view?.placements.map((placement) => {
          const b = value.blocks.find((b) => b.id === placement.blockId)!;
          return (
            <Button
              key={placement.id}
              action="select-block"
              data-id={b.id}
              className={
                "composition-block " + (selected === b.id ? "selected" : "")
              }
              onClick={() => select(b.id)}
            >
              <span>
                {b.title}
                <small>
                  {b.id} · {placement.id}
                </small>
              </span>
              <p>{b.body}</p>
            </Button>
          );
        })}
      </div>
      {block && (
        <div className="block-editor">
          <strong>{block.title}</strong>
          <small>
            内容 ID：{block.id} · 来源：{block.source}
          </small>
          <div className="block-usages">
            引用位置：
            {usages.map((u) => (
              <span key={u.id}>
                {u.view} · {u.id}
              </span>
            ))}
          </div>
          <textarea
            aria-label="编辑共享内容块"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
          />
          <Button
            action="save-block"
            className="primary"
            disabled={draft === block.body}
            onClick={() =>
              onSave({
                ...value,
                blocks: value.blocks.map((b) =>
                  b.id === block.id ? { ...b, body: draft } : b,
                ),
              })
            }
          >
            保存共享块
          </Button>
        </div>
      )}
    </section>
  );
}
export function DataViewPanel({
  value,
  onSave,
}: {
  value: DataView;
  onSave: (value: DataView) => void;
}) {
  return value.kind === "table" ? (
    <TableView value={value} />
  ) : (
    <CompositionView value={value} onSave={onSave} />
  );
}
