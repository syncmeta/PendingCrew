import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { reducer } from "./reducer";
import { current, crewLedgers, findReference } from "./selectors";
test("group work is independent, persists when switching and remains referenceable", () => {
  let state = initialState();
  const mainId = current(state).db.plans[0].id;
  state = reducer(state, { type: "ui", patch: { conv: "website-todo" } });
  const todoId = current(state).db.plans[0].id;
  assert.notEqual(todoId, mainId);
  state = reducer(state, {
    type: "savePlan",
    id: todoId,
    status: "active",
    owner: "机长",
    note: "独立进展",
  });
  state = reducer(state, {
    type: "newDocument",
    title: "群内文档",
    body: "只在 Todo介绍",
  });
  const docId = current(state).db.docs[0].id;
  state = reducer(state, { type: "ui", patch: { conv: "website-main" } });
  assert.equal(current(state).db.plans[0].id, mainId);
  assert(!current(state).db.docs.some((d) => d.id === docId));
  assert.equal(findReference(state, docId)?.convId, "website-todo");
  state = reducer(state, { type: "ui", patch: { conv: "website-todo" } });
  assert.equal(current(state).db.plans[0].note, "独立进展");
  assert(current(state).db.docs.some((d) => d.id === docId));
  const ids = Object.values(state.cache).flatMap((s) =>
    crewLedgers(s).flatMap(([, db]) =>
      [...db.plans, ...db.requests, ...db.docs, ...db.asks].map((x) => x.id),
    ),
  );
  assert.equal(new Set(ids).size, ids.length);
});
