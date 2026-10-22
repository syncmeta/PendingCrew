import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { applyContentCompletion } from "./contentCompletion";
import { crewLedgers, findReference, switchConversation } from "./selectors";
import { reducer } from "./reducer";
import cases from "./contentCases.json";

test("captain work moves once into actual development with stable references", () => {
  const state = initialState(),
    intro = state.cache.website;
  assert(!state.scenes.some((scene) => scene.id === "captain"));
  assert.equal(
    intro.db.conversations.find((c) => c.id === "website-development-practice")
      ?.title,
    "实际开发演示",
  );
  for (const id of ["N90001", "H90001", "D90001"])
    assert.equal(
      findReference(state, id)?.convId,
      "website-development-practice",
    );
  assert.equal(state.cache.captain.db.requests.length, 0);
  const ids = state.scenes.flatMap((scene) =>
    crewLedgers(state.cache[scene.id]).flatMap(([, db]) =>
      [...db.requests, ...db.asks, ...db.plans, ...db.docs].map(
        (item) => item.id,
      ),
    ),
  );
  assert.equal(ids.length, new Set(ids).size);
  assert.strictEqual(applyContentCompletion(state), state);
});

test("legacy captain migration preserves visitor edits, messages and draft", () => {
  const state = initialState(),
    intro = state.cache.website;
  switchConversation(intro, "website-development-practice");
  const seed = structuredClone(state.captainDemoSeed!);
  const ids = new Set(
    [
      ...seed.work.requests,
      ...seed.work.asks,
      ...seed.work.docs,
      ...seed.work.plans,
    ].map((item) => item.id),
  );
  for (const key of ["requests", "asks", "docs", "plans"] as const)
    intro.db[key] = intro.db[key].filter((item) => !ids.has(item.id)) as never;
  intro.db.messages["website-development-practice"] = intro.db.messages[
    "website-development-practice"
  ].filter((message) => !seed.messages.some((m) => m.id === message.id));
  const source = state.cache.captain;
  Object.assign(source.db, seed.work);
  source.ui.conv = "captain-main";
  source.ui.drafts["captain-main"] = "保留未发送的任务";
  source.db.requests[0].text = "我修改过的历史需求";
  source.db.requests[0].owner = "远程与配对";
  source.db.messages["captain-main"] = [
    ...seed.messages,
    { id: "message-visitor-captain", sender: "你", text: "我新增的消息" },
  ];
  state.scenes.push({
    ...state.scenes[0],
    id: "captain",
    initialLedger: structuredClone(source.db),
  });
  state.sceneId = "captain";
  delete state.contentCompletionRevision;
  const migrated = applyContentCompletion(state);
  assert.equal(migrated.sceneId, "website");
  assert.equal(migrated.cache.website.ui.conv, "website-development-practice");
  assert.equal(
    migrated.cache.website.db.requests.find(
      (request) => request.id === seed.work.requests[0].id,
    )?.text,
    "我修改过的历史需求",
  );
  assert.equal(
    migrated.cache.website.db.requests.find(
      (request) => request.id === seed.work.requests[0].id,
    )?.owner,
    "远程与配对",
  );
  assert(
    migrated.cache.website.ui.drafts["website-development-practice"].includes(
      "保留未发送的任务",
    ),
  );
  assert(
    migrated.cache.website.db.messages["website-development-practice"].some(
      (m) => m.id === "message-visitor-captain",
    ),
  );
  assert.strictEqual(applyContentCompletion(migrated), migrated);
});

test("reset restores authored captain examples inside introduction only", () => {
  let state = initialState();
  state.sceneId = "website";
  switchConversation(state.cache.website, "website-development-practice");
  state = reducer(state, { type: "completeTodo", id: "N90001" });
  state = reducer(state, { type: "reset" });
  assert(!state.scenes.some((scene) => scene.id === "captain"));
  assert.equal(
    findReference(state, "N90001")?.convId,
    "website-development-practice",
  );
  const restored = findReference(state, "N90001");
  assert(restored?.kind === "request");
  assert.notEqual(restored.record.status, "done");
});

test("topic cases connect AI work, human decisions and plans with relevant original documents", () => {
  let state = initialState();
  for (const item of cases) {
    const request = findReference(state, "N-demo-" + item.conv)!;
    assert(request);
    const work = crewLedgers(state.cache[request.sceneId]).find(
      ([conv]) => conv === item.conv,
    )![1];
    assert(work.docs.some((doc) => doc.siteSource && !doc.archived));
    const ask = work.asks.find((a) => a.id === "H-demo-" + item.conv)!;
    assert.equal(work.plans.find((p) => p.id === ask.item)?.status, "blocked");
  }
  const ask = findReference(state, "H-demo-trip-main")!;
  state.sceneId = ask.sceneId;
  switchConversation(state.cache[ask.sceneId], ask.convId);
  const work = state.cache[ask.sceneId].db,
    decision = work.asks.find((a) => a.id === "H-demo-trip-main")!;
  const sibling = work.plans.find((p) => p.id === "W-demo-trip-main-2")!.status;
  state = reducer(state, {
    type: "answer",
    id: decision.id,
    text: decision.options[0],
  });
  const unlocked = findReference(state, decision.item);
  assert(unlocked?.kind === "plan");
  assert.equal(unlocked.record.status, "active");
  assert.equal(
    state.cache[ask.sceneId].db.plans.find((p) => p.id === "W-demo-trip-main-2")
      ?.status,
    sibling,
  );
});
