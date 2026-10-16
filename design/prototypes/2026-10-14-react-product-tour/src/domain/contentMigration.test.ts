import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { refreshContent } from "./contentMigration";
test("authored overview refresh retains visitor content and avoids Todo ID collisions", () => {
  const before = initialState();
  delete before.contentRevision;
  before.cache.website.db.messages["website-main"] = [
    { id: "website-main-0", sender: "你", text: "obsolete guide" },
    { id: "message-website-1", sender: "你", text: "访客追加的内容" },
  ];
  before.cache.website.db.requests = [
    { id: "N101", code: "#101", text: "obsolete guide" },
    { id: "N102", code: "#102", text: "访客原文" },
  ];
  before.cache.website.ui.drafts["website-main"] = "保留草稿";
  before.cache.website.ui.mentions = true;
  const after = refreshContent(before),
    db = after.cache.website.db;
  assert.equal(
    before.cache.website.db.messages["website-main"][0].text,
    "obsolete guide",
  );
  assert.equal(
    db.messages["website-main"].filter((m) => m.sender === "机长").length,
    3,
  );
  assert(db.messages["website-main"].some((m) => m.text === "访客追加的内容"));
  assert.equal(after.cache.website.ui.drafts["website-main"], "保留草稿");
  assert.equal(db.requests.find((r) => r.id === "N102")?.text, "访客原文");
  assert(
    db.requests.some(
      (r) => r.text === "你可以持续往群里喂Todo，我来处理这海量而琐碎的需求",
    ),
  );
  assert.equal(new Set(db.requests.map((r) => r.id)).size, db.requests.length);
  assert.equal(after.cache.website.ui.mentions, false);
});

test("product feature refresh preserves visitor documents, edits and stable IDs", () => {
  const before = initialState();
  before.contentRevision = 2;
  const db = before.cache.website.db;
  db.docs = db.docs.slice(0, 2);
  db.docs.push({
    id: "D103",
    title: "我的文档",
    body: "不能覆盖",
    author: "你",
    folder: "我的分类",
  });
  db.folders.document.push("我的分类");
  db.plans[2].note = "我自己写的阅读备注";
  db.plans[2].owner = "你";
  const after = refreshContent(before),
    updated = after.cache.website.db;
  assert.equal(after.contentRevision, 5);
  assert(updated.docs.some((d) => d.id === "D103" && d.body === "不能覆盖"));
  assert(
    updated.docs.some(
      (d) => d.title === "分类由工作决定.md" && d.id !== "D103",
    ),
  );
  const ids = Object.values(after.cache).flatMap((c) =>
    c.db.docs.map((d) => d.id),
  );
  assert.equal(new Set(ids).size, ids.length);
  assert.equal(updated.plans[2].note, "我自己写的阅读备注");
  assert.equal(updated.plans[2].owner, "你");
  assert(updated.folders.document.includes("我的分类"));
  assert.equal(refreshContent(after), after);
});

test("uniform product copy refresh preserves visitor messages, drafts and ask answers", () => {
  const before = initialState();
  before.contentRevision = 3;
  before.scenes[2].crew = "开始使用";
  const d = before.cache.research;
  d.db.messages["research-main"] = [
    { id: "research-main-0", sender: "向导", text: "开始使用" },
    { id: "message-research-1", sender: "你", text: "我的问题" },
  ];
  d.ui.drafts["research-main"] = "未发送的草稿";
  d.db.asks[0].status = "answered";
  d.db.asks[0].answer = "我的答复";
  d.db.folders.message = ["开始使用", "你的问题", "我的文件夹"];
  d.db.placements["message:message-research-1"] = "我的文件夹";
  const after = refreshContent(before),
    db = after.cache.research.db;
  assert.equal(after.scenes[2].crew, "组织与沟通");
  assert(db.messages["research-main"].some((m) => m.text === "我的问题"));
  assert(!db.messages["research-main"].some((m) => m.sender === "向导"));
  assert.equal(after.cache.research.ui.drafts["research-main"], "未发送的草稿");
  assert.equal(db.asks[0].status, "answered");
  assert.equal(db.asks[0].answer, "我的答复");
  assert.equal(db.placements["message:message-research-1"], "我的文件夹");
  assert(db.folders.message.includes("我的文件夹"));
});
