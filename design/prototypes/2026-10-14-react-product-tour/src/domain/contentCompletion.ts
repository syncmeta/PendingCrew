import cases from "./contentCases.json";
import site from "./siteDocuments.json";
import { crewLedgers, switchConversation } from "./selectors";
import type { TourState, GroupWork, SceneState, DocumentRecord } from "./types";
const practiceId = "website-development-practice";
function workAt(record: SceneState, id: string): GroupWork | undefined {
  return record.ui.conv === id ? record.db : record.groups?.[id];
}
function pickWork(db: GroupWork): GroupWork {
  const { plans, requests, asks, docs, folders, placements, history } = db;
  return structuredClone({
    plans,
    requests,
    asks,
    docs,
    folders,
    placements,
    history,
  });
}
function emptyWork(): GroupWork {
  return {
    plans: [],
    requests: [],
    asks: [],
    docs: [],
    folders: { message: [], todo: [], document: [], progress: [] },
    placements: {},
    history: [],
  };
}
function mergeWork(target: GroupWork, source: GroupWork) {
  for (const key of ["plans", "requests", "asks", "docs"] as const) {
    const ids = new Set(target[key].map((item) => item.id));
    for (const item of source[key])
      if (!ids.has(item.id)) {
        (target[key] as { id: string }[]).push(structuredClone(item));
        ids.add(item.id);
      }
  }
  for (const scope of ["message", "todo", "document", "progress"] as const)
    target.folders[scope] = [
      ...new Set([...target.folders[scope], ...source.folders[scope]]),
    ];
  Object.assign(target.placements, source.placements);
  for (const event of source.history)
    if (
      !target.history.some(
        (e) => e.text === event.text && e.time === event.time,
      )
    )
      target.history.push(structuredClone(event));
}
export function applyContentCompletion(state: TourState): TourState {
  const title = state.cache.website.db.conversations.find(
    (c) => c.id === practiceId,
  )?.title;
  const captainMoved =
    !state.scenes.some((s) => s.id === "captain") &&
    workAt(state.cache.website, practiceId)?.requests.some(
      (r) => r.id === "N90001",
    );
  const casesPresent = cases.every((item) => {
    const record = Object.values(state.cache).find((r) =>
      r.db.conversations.some((c) => c.id === item.conv),
    );
    return (
      record &&
      workAt(record, item.conv)?.requests.some(
        (r) => r.id === "N-demo-" + item.conv,
      )
    );
  });
  if (
    state.contentCompletionRevision === 1 &&
    title === "实际开发演示" &&
    captainMoved &&
    casesPresent
  )
    return state;
  const next = structuredClone(state),
    intro = next.cache.website,
    target = workAt(intro, practiceId)!;
  const source = next.cache.captain,
    sourceScene = next.scenes.find((s) => s.id === "captain");
  const wasCaptain = next.sceneId === "captain";
  if (!next.captainDemoSeed && sourceScene?.initialLedger)
    next.captainDemoSeed = {
      work: pickWork(sourceScene.initialLedger),
      messages: structuredClone(
        sourceScene.initialLedger.messages["captain-main"] ?? [],
      ),
    };
  const messages = (intro.db.messages[practiceId] ??= []);
  if (source) {
    for (const [, db] of crewLedgers(source)) mergeWork(target, db);
    const ids = new Set(messages.map((m) => m.id));
    for (const list of Object.values(source.db.messages))
      for (const message of list)
        if (!ids.has(message.id)) {
          messages.push(structuredClone(message));
          ids.add(message.id);
        }
    const drafts = Object.values(source.ui.drafts).filter(Boolean);
    const current = intro.ui.drafts[practiceId] ?? "";
    if (drafts.length)
      intro.ui.drafts[practiceId] = [
        current,
        ...drafts.filter((draft) => !current.includes(draft)),
      ]
        .filter(Boolean)
        .join("\n");
    Object.assign(source.db, emptyWork());
    source.groups = {};
    source.db.messages = Object.fromEntries(
      source.db.conversations.map((c) => [c.id, []]),
    );
  }
  // Reset restores authored history, while migration above moves visitor work with stable identities.
  if (
    !target.requests.some((request) => request.id === "N90001") &&
    next.captainDemoSeed
  ) {
    mergeWork(target, next.captainDemoSeed.work);
    const ids = new Set(messages.map((m) => m.id));
    for (const message of next.captainDemoSeed.messages)
      if (!ids.has(message.id)) messages.push(structuredClone(message));
  }
  for (const request of target.requests)
    request.owner ??=
      target.plans.find((plan) => plan.sources.includes(request.id))?.owner ??
      "机长";
  for (const ask of target.asks) ask.owner ??= "人";
  for (const doc of target.docs)
    if (!doc.siteSource && !doc.dataView && doc.author !== "你")
      doc.archived = true;
  const practice = intro.db.conversations.find((c) => c.id === practiceId)!;
  practice.title = "实际开发演示";
  practice.subtitle = "PendingCrew 真实历史任务、开发分工和过程记录。";
  next.scenes = next.scenes.filter((scene) => scene.id !== "captain");
  if (wasCaptain) {
    next.sceneId = "website";
    switchConversation(intro, practiceId);
    Object.assign(intro.ui, {
      tab: source?.ui.tab ?? "message",
      kind: source?.ui.kind ?? "requests",
      query: source?.ui.query ?? "",
      todoTag: source?.ui.todoTag ?? "all",
    });
  }
  for (const item of cases) {
    const scene = next.scenes.find((s) =>
      next.cache[s.id].db.conversations.some((c) => c.id === item.conv),
    );
    if (!scene) continue;
    const record = next.cache[scene.id],
      work = workAt(record, item.conv);
    if (!work) continue;
    const ai = "N-demo-" + item.conv,
      human = "H-demo-" + item.conv,
      root = "W-demo-" + item.conv;
    const agent = scene.agent;
    const tags =
      scene.id === "knowledge"
        ? ["文档/数据"]
        : scene.id === "administration"
          ? ["行政", "跟进"]
          : scene.id === "research"
            ? ["协作", "组织"]
            : scene.id === "future-five-days"
              ? ["产品思考"]
              : ["协作", "大工程"];
    if (!work.requests.some((r) => r.id === ai)) {
      work.requests.push({
        id: ai,
        code: "#场景",
        text: item.ai,
        owner: agent,
        status: "active",
        tags,
        sourceLabel: "主题场景演示",
        source: "https://docs.pendingname.com/pendingcrew/",
      });
      work.asks.push({
        id: human,
        question: item.human,
        owner: "人",
        context: "此事项用于演示本群主题的判断与反馈，不会执行真实操作。",
        recommendation: item.options[0],
        options: item.options,
        status: "open",
        answer: "",
        item: root + "-3",
        tags,
      });
      work.plans.push({
        id: root,
        code: "P·场景",
        title: item.ai,
        parent: null,
        status: "active",
        owner: agent,
        sources: [ai],
        dependsOn: [],
        blockedOn: [],
        note: item.lesson,
        nextCheck: "核对下一步与相关材料",
        folder: "主题场景",
      });
      for (const [index, step] of item.steps.entries())
        work.plans.push({
          id: root + "-" + (index + 1),
          code: "P·" + (index + 1),
          title: step,
          parent: root,
          status: index === 2 ? "blocked" : index === 0 ? "active" : "open",
          owner: agent,
          sources: [ai],
          dependsOn: [],
          blockedOn: index === 2 ? [human] : [],
          note:
            index === 2
              ? "这一部分等待真人判断，其他部分可以继续。"
              : "按本群案例逐步补充结果与证据。",
          nextCheck: index === 2 ? "等待真人反馈" : "下一检查点",
          folder: "主题场景",
        });
      if (!work.folders.progress.includes("主题场景"))
        work.folders.progress.push("主题场景");
      if (item.expandMessages)
        record.db.messages[item.conv].push(
          {
            id: "case-" + item.conv + "-request",
            sender: "你",
            text: item.ai,
            ref: ai,
          },
          {
            id: "case-" + item.conv + "-context",
            sender: agent,
            text: item.lesson,
            ref: root,
          },
          {
            id: "case-" + item.conv + "-decision",
            sender: agent,
            text:
              "这件事可以分成：" +
              item.steps.join("；") +
              "。其中需要你判断的部分已经单独放进真人 Todo。",
            ref: human,
          },
        );
    }
    const urls = item.docs
      .map((key) => site.find((doc) => doc.key === key)?.siteSource)
      .filter(Boolean);
    for (const doc of work.docs)
      if (doc.siteSource) doc.archived = !urls.includes(doc.siteSource);
    for (const key of item.docs) {
      const original = site.find((doc) => doc.key === key);
      if (!original) continue;
      const existing = work.docs.find(
        (doc) => doc.siteSource === original.siteSource,
      );
      if (existing) {
        existing.archived = false;
        continue;
      }
      work.docs.push({
        id: "D-topic-" + item.conv + "-" + key,
        title: original.title,
        body: original.body,
        siteSource: original.siteSource,
        sourceSha256: original.sourceSha256,
        sources: original.sources,
        author: "PendingCrew 文档站",
        folder: "文档站",
        kind: "产品说明",
      });
    }
    if (!work.folders.document.includes("文档站"))
      work.folders.document.unshift("文档站");
    const conversation = record.db.conversations.find(
      (c) => c.id === item.conv,
    )!;
    if (
      conversation.members &&
      !conversation.members.some((member) => member.name === agent)
    )
      conversation.members.unshift({
        name: agent,
        detail: "Codex Session",
        glyph: scene.glyph,
      });
  }
  const memory = workAt(next.cache.knowledge, "knowledge-memory")!;
  if (!memory.docs.some((doc) => doc.id === "D-memory-links-demo"))
    memory.docs.push({
      id: "D-memory-links-demo",
      title: "问题、经验和观察的关联数据",
      author: "机长",
      folder: "数据示例",
      body: "这是一组可筛选的示意记录。内容与任务、决定相联系，并保留事实和推测的区别。",
      dataView: {
        kind: "table",
        primaryKey: "id",
        description: "演示数据：把零碎记录变成可计算、有关系的公共材料。",
        schema:
          "CREATE TABLE memory_records (id TEXT PRIMARY KEY, kind TEXT, text TEXT, relation TEXT, certainty TEXT);",
        columns: [
          { key: "id", label: "编号", type: "TEXT" },
          { key: "kind", label: "类型", type: "TEXT" },
          { key: "text", label: "内容", type: "TEXT" },
          { key: "relation", label: "关联", type: "TEXT" },
          { key: "certainty", label: "性质", type: "TEXT" },
        ],
        rows: [
          {
            id: "M1",
            kind: "经验",
            text: "错误提示应保留 errno 和路径",
            relation: "诊断记录 / 开发复盘",
            certainty: "历史记录",
          },
          {
            id: "M2",
            kind: "问题",
            text: "重开后是否出现自动连接？",
            relation: "远程配对验收",
            certainty: "待验证",
          },
          {
            id: "M3",
            kind: "观察",
            text: "交接需要目标、证据和下一步",
            relation: "机长 / 工作跟进",
            certainty: "实践观察",
          },
        ],
      },
    } as DocumentRecord);
  const views = workAt(next.cache.knowledge, "knowledge-views")!;
  if (!views.docs.some((doc) => doc.id === "D-views-composition-demo"))
    views.docs.push({
      id: "D-views-composition-demo",
      title: "同一份材料，两种工作视图",
      author: "机长",
      folder: "数据示例",
      body: "材料先独立保存，再按读者和用途组合。修改共享内容块后，引用它的视图会一起更新。",
      dataView: {
        kind: "composition",
        description: "示意数据：相同的内容块可以同时进入项目概览和交接材料。",
        blocks: [
          {
            id: "delivery",
            title: "交付阶段",
            body: "当前材料需要区分已验收模块和仍在推进的完整链路。",
            source: "开发记录",
          },
          {
            id: "next",
            title: "下一步",
            body: "下一轮检查重开后的手动连接、业务读写和撤销。",
            source: "大工程计划",
          },
        ],
        views: [
          {
            id: "overview",
            title: "项目概览",
            placements: [
              { id: "overview-delivery", blockId: "delivery" },
              { id: "overview-next", blockId: "next" },
            ],
          },
          {
            id: "handoff",
            title: "交接材料",
            placements: [
              { id: "handoff-next", blockId: "next" },
              { id: "handoff-delivery", blockId: "delivery" },
            ],
          },
        ],
      },
    } as DocumentRecord);
  for (const work of [memory, views])
    if (!work.folders.document.includes("数据示例"))
      work.folders.document.push("数据示例");
  next.contentCompletionRevision = 1;
  return next;
}
