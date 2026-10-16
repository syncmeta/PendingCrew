import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { reducer } from "./reducer";
import { current, findReference, folderOf } from "./selectors";
test("a human answer updates the unique ask and only related blockers", () => {
  const before = initialState();
  const after = reducer(before, {
    type: "answer",
    id: "H201",
    sceneId: "trip",
    text: "先查看三层计划",
  });
  assert.equal(before.cache.trip.db.asks[0].status, "open");
  assert.equal(after.cache.trip.db.asks[0].status, "answered");
  assert.equal(
    after.cache.trip.db.plans.find((p) => p.id === "W205")?.status,
    "active",
  );
  assert.equal(
    after.cache.research.db.plans.find((p) => p.id === "W305")?.status,
    "blocked",
  );
  assert.equal(findReference(after, "H201")?.sceneId, "trip");
});
test("an invalid completion cannot partially write note or owner", () => {
  const before = initialState();
  const after = reducer(before, {
    type: "savePlan",
    id: "W101",
    status: "done",
    owner: "向导",
    note: "should not write",
  });
  assert.strictEqual(after.cache, before.cache);
  assert.match(after.notice!.text, /先结束子计划/);
});
test("unresolved human blocker cannot be bypassed", () => {
  const before = reducer(initialState(), { type: "selectScene", id: "trip" });
  const after = reducer(before, {
    type: "savePlan",
    id: "W205",
    status: "active",
    owner: "机长",
    note: "bypass",
  });
  assert.strictEqual(after.cache, before.cache);
  assert.match(after.notice!.text, /先处理/);
});
test("classification and rename preserve identity and all work relations", () => {
  let state = initialState();
  const before = structuredClone(current(state).db.plans[2]);
  state = reducer(state, {
    type: "saveFolder",
    scope: "progress",
    name: "阅读记录",
  });
  state = reducer(state, {
    type: "move",
    id: "W103",
    scope: "progress",
    folder: "阅读记录",
  });
  state = reducer(state, {
    type: "saveFolder",
    scope: "progress",
    old: "阅读记录",
    name: "归档",
  });
  assert.deepEqual(current(state).db.plans[2], before);
  assert.equal(folderOf(current(state).db, before, "progress"), "归档");
  assert.notEqual(folderOf(current(state).db, before, "todo"), "归档");
});
test("new IDs are unique across crews and sharing does not erase a draft", () => {
  let state = initialState();
  state = reducer(state, {
    type: "ui",
    patch: { drafts: { "website-main": "草稿保留" } },
  });
  state = reducer(state, { type: "shareReference", id: "D101" });
  assert.equal(current(state).ui.drafts["website-main"], "草稿保留");
  state = reducer(state, { type: "newRequest", text: "一个新需求" });
  const ids = Object.values(state.cache).flatMap((s) =>
    s.db.requests.map((r) => r.id),
  );
  assert.equal(new Set(ids).size, ids.length);
  assert.equal(current(state).db.requests[0].text, "一个新需求");
});
test("a custom crew remains resettable and keeps an independent seed", () => {
  let state = initialState();
  state = reducer(state, {
    type: "newCrew",
    id: "custom-test",
    name: "读书计划",
    brief: "整理书的章节。",
  });
  assert.equal(state.sceneId, "custom-test");
  assert.equal(current(state).db.requests[0].text, "整理书的章节。");
  const count = current(state).db.messages["custom-test-main"].length;
  state = reducer(state, { type: "send", text: "增加一条消息" });
  assert.equal(
    current(state).db.messages["custom-test-main"].length,
    count + 1,
  );
  state = reducer(state, { type: "reset" });
  assert.equal(current(state).db.messages["custom-test-main"].length, count);
  assert.equal(state.cache.website.db.requests[0].id, "N101");
});
test("progress is a view of the same plan, not a separate copy", () => {
  let state = initialState();
  state = reducer(state, {
    type: "savePlan",
    id: "W103",
    status: "active",
    owner: "向导",
    note: "同源备注",
  });
  state = reducer(state, { type: "ui", patch: { tab: "progress" } });
  assert.equal(
    current(state).db.plans.find((p) => p.id === "W103")?.note,
    "同源备注",
  );
  state = reducer(state, { type: "ui", patch: { tab: "todo" } });
  assert.equal(
    current(state).db.plans.find((p) => p.id === "W103")?.note,
    "同源备注",
  );
});
test("withdrawal terminates the related portion without answering another ask", () => {
  let state = initialState();
  state = reducer(state, {
    type: "askWithdraw",
    sceneId: "research",
    id: "H301",
  });
  assert.equal(
    current(state).db.plans.find((p) => p.id === "W305")?.status,
    "dropped",
  );
  assert.equal(state.cache.trip.db.asks[0].status, "open");
});
