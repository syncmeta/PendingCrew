import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { initialState } from "./seed";
import { crewLedgers, findReference } from "./selectors";
import { library, refreshDocumentLibrary } from "./documentLibrary";
import { refreshContent } from "./contentMigration";
import { reducer } from "./reducer";
const records = (s: ReturnType<typeof initialState>) =>
  Object.values(s.cache).flatMap((c) =>
    crewLedgers(c).flatMap(([, db]) => db.docs),
  );
test("45 distinct curated documents have real sources, valid related references and separate group ownership", () => {
  const state = initialState(),
    docs = records(state),
    authored = docs.filter((d) => d.catalogKey);
  assert.equal(authored.length, 45);
  assert.equal(new Set(docs.map((d) => d.id)).size, docs.length);
  for (const d of authored) {
    assert(d.body.length > (d.dataView ? 40 : 140));
    assert(d.summary);
    assert(d.sources?.length);
    for (const source of d.sources ?? [])
      assert(existsSync(resolve("../../..", source.path)), source.path);
    for (const id of d.related ?? []) assert(findReference(state, id), id);
  }
  for (const item of library) {
    const d = authored.find((d) => d.catalogKey === item.key)!;
    const ref = findReference(state, d.id)!;
    assert.equal(ref.sceneId, item.scene);
    assert.equal(ref.convId, item.scene + "-" + item.conversation);
  }
  assert(authored.some((d) => d.kind === "规划设想"));
  assert(authored.some((d) => d.kind === "已敲定设计"));
});
test("catalog migration preserves all existing work, visitor content, drafts and selected group, and resolves ID collisions globally", () => {
  const state = initialState();
  state.contentRevision = 5;
  for (const c of Object.values(state.cache))
    for (const db of [c.db, ...Object.values(c.groups ?? {})]) {
      db.docs = db.docs.filter((d) => !d.catalogKey);
      for (const p of db.plans)
        if (p.evidence?.startsWith("D800")) delete p.evidence;
    }
  state.cache.research.db.docs.push({
    id: "D800001",
    title: "访客资料",
    body: "不覆盖",
    author: "你",
  });
  state.cache.website.ui.drafts["website-main"] = "我的草稿";
  state.cache.website.db.plans[0].note = "我的下一步";
  const next = refreshContent(state),
    docs = records(next);
  assert.equal(next.contentRevision, 16);
  assert.equal(docs.find((d) => d.id === "D800001")?.body, "不覆盖");
  assert.notEqual(docs.find((d) => d.catalogKey === "position")?.id, "D800001");
  assert.equal(new Set(docs.map((d) => d.id)).size, docs.length);
  assert.equal(next.cache.website.ui.drafts["website-main"], "我的草稿");
  assert.equal(next.cache.website.db.plans[0].note, "我的下一步");
  for (const d of docs.filter((d) => d.catalogKey))
    for (const id of d.related ?? []) assert(findReference(next, id));
  const again = refreshDocumentLibrary(next);
  assert.equal(records(again).length, docs.length);
  assert.deepEqual(
    again.cache.website.db.messages,
    next.cache.website.db.messages,
  );
});
test("reset resolves related documents across crews after rebuilding its catalog", () => {
  let state = initialState();
  state = reducer(state, { type: "reset" });
  const d = records(state).find((d) => d.catalogKey === "human-role")!;
  assert(
    d.related?.some(
      (id) => findReference(state, id)?.sceneId === "administration",
    ),
  );
});
