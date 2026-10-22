import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { refreshContent } from "./contentMigration";
import { crewLedgers, findReference } from "./selectors";
test("revision 11 gains the three problem groups without resetting previous content", () => {
  const before = initialState();
  before.contentRevision = 11;
  delete before.cache.problems;
  before.scenes = before.scenes.filter((s) => s.id !== "problems");
  before.sceneId = "self-introduction";
  before.cache.website.ui.drafts["website-main"] = "我的草稿";
  const next = refreshContent(before);
  assert.equal(next.contentRevision, 16);
  assert.equal(next.sceneId, "website");
  assert(!next.scenes.some((scene) => scene.id === "self-introduction"));
  assert.equal(next.scenes.find((scene) => scene.id === "future-five-days")?.crew, "它的未来");
  assert.equal(next.cache.website.ui.drafts["website-main"], "我的草稿");
  assert.equal(next.scenes.filter((s) => s.id === "problems").length, 1);
  const record = next.cache.problems;
  assert.deepEqual(
    record.db.conversations.filter((c)=>c.kind==="group").map((c) => c.title),
    [
      "处理持续变化的任务流",
      "多人与多Agent的协作空间",
      "Coding Agent统一远程管理界面",
    ],
  );
  for (const [conv, work] of crewLedgers(record)) {
    if(conv.includes("-session-"))continue;
    assert.equal(work.docs.filter((doc)=>!doc.siteSource).length, 1);
    assert(
      work.docs[0].sources?.every((s) =>
        s.path.startsWith("https://docs.pendingname.com/pendingcrew/"),
      ),
    );
    assert.equal(
      record.db.messages[conv].length,
      conv === "problems-collaboration"
        ? 4
        : conv === "problems-remote"
          ? 2
          : 6,
    );
    for (const m of record.db.messages[conv])
      if (m.ref) assert.equal(findReference(next, m.ref)?.convId, conv);
    assert(work.plans.filter((p)=>!p.id.startsWith("W-demo-")).every((p) => p.status === "open"));
  }
  assert.equal(record.groups?.["problems-remote"].docs[0].kind, "规划设想");
  assert.strictEqual(refreshContent(next), next);
});

test("older browser state gains the roadmap and knowledge crews while retaining collaboration drafts", () => {
  const before = initialState();
  before.contentRevision = 12;
  for (const id of ["current-progress", "future-five-days", "knowledge"])
    delete before.cache[id];
  before.scenes = before.scenes.filter(
    (s) =>
      !["current-progress", "future-five-days", "knowledge"].includes(s.id),
  );
  before.cache.problems.ui.drafts["problems-collaboration"] = "保留我的想法";
  before.cache.problems.db.messages["problems-collaboration"] = [
    { id: "problems-collaboration-0", sender: "组织协作", text: "旧介绍" },
    { id: "message-visitor-keep", sender: "你", text: "我自己的问题" },
  ];
  const next = refreshContent(before);
  assert.equal(
    next.cache.problems.ui.drafts["problems-collaboration"],
    "保留我的想法",
  );
  const messages = next.cache.problems.db.messages["problems-collaboration"];
  assert.equal(messages.length, 5);
  assert.equal(messages[3].text, "如何降低对用户协作能力的要求？");
  assert.equal(messages[4].text, "我自己的问题");
  assert.equal(next.cache["future-five-days"].db.conversations.filter((c)=>c.kind==="group").length, 4);
  assert.equal(next.cache.knowledge.db.conversations.filter((c)=>c.kind==="group").length, 5);
  assert.equal(
    crewLedgers(next.cache.knowledge)
      .flatMap(([, work]) => work.docs)
      .filter((d) => d.dataView).length,
    5,
  );
  assert.equal(next.contentRevision, 16);
  assert.strictEqual(refreshContent(next), next);
});
