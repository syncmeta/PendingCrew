import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { refreshContent } from "./contentMigration";
import { crewLedgers, findReference, switchConversation } from "./selectors";

test("only the introduction repeats the role demo; other crews present source documents in topical conversations", () => {
  const state = initialState();
  assert.deepEqual(
    state.cache.website.db.conversations.filter((c)=>c.kind==="group").map((c) => c.title),
    ["总群", "Todo介绍", "实际开发演示", "经验与复盘", "大工程"],
  );
  for (const id of ["trip", "research", "administration"]) {
    const record = state.cache[id];
    assert(record.db.conversations.some((c) => c.kind === "group"));
    assert(
      !record.db.conversations.some((c) =>
        ["机长", "Todo介绍", "演示策划", "Demo开发"].includes(c.title),
      ),
    );
    for (const [conv, work] of crewLedgers(record)) {
      if(conv.includes("-session-"))continue;
      const conversation = record.db.conversations.find((c) => c.id === conv)!;
      assert(
        !conversation.members?.some(
          (m) => m.name === "机长" || m.name === "Todo介绍",
        ),
      );
      for (const message of record.db.messages[conv]) {
        assert(!message.text.includes("这个 Demo 使用 React"));
        assert(message.ref && findReference(state, message.ref));
      }
      if (!conv.endsWith("-main")) {
        assert(work.docs.some((d) => d.catalogKey));
        assert(
          record.db.messages[conv].some(
            (m) =>
              m.text.includes("已敲定设计") ||
              m.text.includes("规划设想") ||
              m.text.includes("产品说明"),
          ),
        );
      }
    }
  }
});

test("revision 10 moves source documents with stable IDs and preserves visitor work and selected conversations", () => {
  const state = initialState();
  state.contentRevision = 10;
  const record = state.cache.research;
  switchConversation(record, "research-development");
  const doc = record.db.docs.find(
    (d) => d.catalogKey === "document-lifecycle",
  )!;
  const id = doc.id;
  doc.body += "\n我的补充";
  record.db.docs = record.db.docs.filter((d) => d.id !== id);
  record.groups!["research-main"].docs.push(doc);
  record.groups!["research-main"].placements["document:" + id] = "我的分类";
  record.ui.drafts["research-development"] = "保留草稿";
  record.db.plans[1].title = "我自己改的工作标题";
  record.db.plans[1].note = "保留备注";
  record.db.messages["research-development"].push({
    id: "message-visitor-1",
    sender: "你",
    text: "我的问题",
  });
  record.db.conversations.forEach((c) => {
    c.title = c.id.endsWith("-main") ? "总群" : "Todo介绍";
    delete c.members;
  });
  const next = refreshContent(state);
  const after = next.cache.research;
  assert.equal(after.ui.conv, "research-development");
  assert.equal(after.ui.drafts["research-development"], "保留草稿");
  assert(after.db.docs.find((d) => d.id === id)?.body.includes("我的补充"));
  assert.equal(after.db.placements["document:" + id], "我的分类");
  assert.equal(after.db.plans[1].title, "我自己改的工作标题");
  assert.equal(after.db.plans[1].note, "保留备注");
  assert(
    after.db.messages["research-development"].some(
      (m) => m.text === "我的问题",
    ),
  );
  assert.equal(findReference(next, id)?.convId, "research-development");
  assert.equal(
    crewLedgers(after)
      .flatMap(([, db]) => db.docs)
      .filter((d) => d.id === id).length,
    1,
  );
  assert.strictEqual(refreshContent(next), next);
});
