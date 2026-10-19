import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { crewLedgers } from "./selectors";
import { reducer } from "./reducer";
import { validDataView } from "./dataViews";
import { refreshContent } from "./contentMigration";
const docs = (s: ReturnType<typeof initialState>) =>
  Object.values(s.cache).flatMap((c) =>
    crewLedgers(c).flatMap(([, db]) => db.docs),
  );
test("table examples and shared document blocks have valid typed structures", () => {
  const state = initialState(),
    views = docs(state).filter((d) => d.dataView);
  assert.equal(views.length, 3);
  for (const d of views) assert(validDataView(d.dataView));
  assert(
    !validDataView({
      kind: "composition",
      description: "bad",
      blocks: [],
      views: [
        { id: "v", title: "x", placements: [{ id: "p", blockId: "missing" }] },
      ],
    }),
  );
});
test("editing a shared block updates both document placements and keeps work state untouched", () => {
  const before = initialState(),
    document = docs(before).find(
      (d) => d.catalogKey === "shared-block-example",
    )!,
    view = document.dataView!;
  assert(view.kind === "composition");
  const changed = {
    ...view,
    blocks: view.blocks.map((b) =>
      b.id === "block-work-model" ? { ...b, body: "共享内容已更新" } : b,
    ),
  };
  const after = reducer(before, {
    type: "saveDataView",
    id: document.id,
    dataView: changed,
  });
  assert.deepEqual(after.cache.website.db.plans, before.cache.website.db.plans);
  assert.equal(
    view.blocks
      .find((b) => b.id === "block-work-model")
      ?.body.includes("一本账"),
    true,
  );
  const saved = docs(after).find((d) => d.id === document.id)!.dataView!;
  assert(saved.kind === "composition");
  assert.equal(
    saved.views
      .flatMap((v) => v.placements)
      .filter((p) => p.blockId === "block-work-model").length,
    2,
  );
  assert.equal(
    saved.blocks.find((b) => b.id === "block-work-model")?.body,
    "共享内容已更新",
  );
  after.contentRevision = 6;
  const restored = refreshContent(after);
  assert.deepEqual(
    docs(restored).find((d) => d.id === document.id)?.dataView,
    saved,
  );
});
