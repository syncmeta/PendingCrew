import { refreshDocumentLibrary } from "./documentLibrary";
import { freshScene, initialScenes, initialGroupWork } from "./seed";
import type { TourState } from "./types";
/** Replace obsolete authored introduction, retaining visitor-created records and drafts. */
export function refreshContent(state: TourState): TourState {
  if ((state.contentRevision ?? 0) >= 6) return state;
  if ((state.contentRevision ?? 0) >= 5) return refreshDocumentLibrary(state);
  if ((state.contentRevision ?? 0) >= 4) return migrateGroups(state);
  if ((state.contentRevision ?? 0) >= 3) return unifyProductCopy(state);
  if ((state.contentRevision ?? 0) >= 2) return refreshProductFeatures(state);
  const next = structuredClone(state);
  next.scenes = next.scenes.map((s) =>
    s.id === "website" ? { ...s, ...initialScenes[0] } : s,
  );
  for (const s of next.scenes) {
    const record = next.cache[s.id];
    if (record)
      for (const c of record.db.conversations)
        if (c.id === s.id + "-main") c.title = "概览群";
  }
  const record = next.cache.website;
  if (record) {
    const fresh = freshScene(initialScenes[0]).db;
    const visitorMessages = (record.db.messages["website-main"] ?? []).filter(
      (m) => !m.id.startsWith("website-main-"),
    );
    record.db.messages["website-main"] = [
      ...fresh.messages["website-main"],
      ...visitorMessages,
    ];
    const visitorRequests = record.db.requests.filter((r) => r.id !== "N101");
    const ids = new Set(
      Object.values(next.cache).flatMap((s) => s.db.requests.map((r) => r.id)),
    );
    const requests = fresh.requests.map((r) => {
      if (r.id === "N101" || !visitorRequests.some((old) => old.id === r.id))
        return r;
      const number =
        Math.max(
          ...[...ids].map((id) => Number(id.slice(1))).filter(Number.isFinite),
        ) + 1;
      const id = "N" + number;
      ids.add(id);
      return { ...r, id, code: "#" + number };
    });
    record.db.requests = [...requests, ...visitorRequests];
    record.db.asks = record.db.asks.filter((a) => a.id !== "H101");
    for (const plan of record.db.plans) {
      const seeded = fresh.plans.find((p) => p.id === plan.id);
      if (seeded) plan.title = seeded.title;
      if (plan.blockedOn.includes("H101")) {
        plan.blockedOn = plan.blockedOn.filter((id) => id !== "H101");
        if (plan.status === "blocked" && !plan.blockedOn.length) {
          plan.status = "open";
          plan.nextCheck = "今天 15:00";
        }
      }
    }
    record.ui.kind = "requests";
    record.ui.tab = "message";
    record.ui.conv = "website-main";
    record.ui.query = "";
    record.ui.mentions = false;
    record.ui.mode = "human";
    record.ui.selectedFolders.message = "all";
    record.ui.scrolls["website-main"] = 0;
  }
  next.contentRevision = 2;
  return refreshProductFeatures(next);
}

/** Refresh authored product feature records; keep visitor edits and folder placements. */
function refreshProductFeatures(state: TourState): TourState {
  const next = structuredClone(state);
  for (const scene of initialScenes) {
    const record = next.cache[scene.id];
    if (!record) continue;
    const fresh = freshScene(scene).db;
    const authoredIds = new Set(fresh.docs.slice(0, 2).map((d) => d.id));
    const visitors = record.db.docs.filter((d) => !authoredIds.has(d.id));
    const usedIds = new Set(
      Object.values(next.cache).flatMap((c) => c.db.docs.map((d) => d.id)),
    );
    record.db.docs = [
      ...fresh.docs.map((d) => {
        if (!visitors.some((v) => v.id === d.id)) return d;
        const id =
          "D" +
          (Math.max(
            ...[...usedIds]
              .map((id) => Number(id.slice(1)))
              .filter(Number.isFinite),
          ) +
            1);
        usedIds.add(id);
        return { ...d, id };
      }),
      ...visitors,
    ];
    for (const plan of record.db.plans) {
      const seeded = fresh.plans.find((p) => p.id === plan.id);
      if (!seeded) continue;
      plan.title = seeded.title;
      plan.goal = seeded.goal;
      plan.acceptance = seeded.acceptance;
      if (plan.owner === "向导") plan.owner = "机长";
      if (
        !plan.note ||
        [
          "保留需求原文，按计划持续跟进。",
          "已经整理完成，成果保存在文档里。",
          "沿用上一步的整理结果。",
          "从一个机组、一条清楚的需求和一次有上下文的判断开始。",
          "原始需求、计划和人的事项互相引用，进展读取同一份计划。",
          "可以先推进其他部分，只有这一项在等你。",
        ].includes(plan.note)
      )
        plan.note = seeded.note;
      plan.folder = seeded.folder;
    }
    for (const scope of ["document", "progress"] as const) {
      const stale =
        scene.id === "website"
          ? scope === "progress"
            ? ["阅读进度", "深入了解"]
            : ["产品说明", "基本概念"]
          : scene.id === "trip"
            ? scope === "progress"
              ? ["流程进展", "关联记录"]
              : ["流程说明", "数据关系"]
            : scope === "progress"
              ? ["上手进度", "后续探索"]
              : ["入门文档", "界面说明"];
      record.db.folders[scope] = [
        ...new Set([
          ...fresh.folders[scope],
          ...record.db.folders[scope].filter((f) => !stale.includes(f)),
        ]),
      ];
      if (!record.db.folders[scope].includes(record.ui.selectedFolders[scope]))
        record.ui.selectedFolders[scope] = "all";
    }
  }
  next.contentRevision = 3;
  return unifyProductCopy(next);
}

/** Unify built-in copy, keeping visitor conversations, messages, edits and classifications. */
function unifyProductCopy(state: TourState): TourState {
  const next = structuredClone(state);
  for (const scene of initialScenes) {
    const record = next.cache[scene.id];
    if (!record) continue;
    next.scenes = next.scenes.map((s) =>
      s.id === scene.id ? { ...s, ...scene } : s,
    );
    const fresh = freshScene(scene).db;
    const oldNames = {
      website: {
        message: ["产品介绍", "你的问题"],
        todo: ["认识产品", "阅读路线"],
      },
      trip: {
        message: ["工作方式", "你的问题"],
        todo: ["协作流程", "进一步了解"],
      },
      research: {
        message: ["开始使用", "你的问题"],
        todo: ["上手步骤", "阅读清单"],
      },
    }[scene.id as "website" | "trip" | "research"];
    for (const scope of ["message", "todo"] as const) {
      const rename = (name: string) => {
        const index = oldNames?.[scope].indexOf(name) ?? -1;
        return index < 0 ? name : fresh.folders[scope][index];
      };
      record.db.folders[scope] = [
        ...new Set([
          ...fresh.folders[scope],
          ...record.db.folders[scope].map(rename),
        ]),
      ];
      if (record.ui.selectedFolders[scope] !== "all")
        record.ui.selectedFolders[scope] = rename(
          record.ui.selectedFolders[scope],
        );
      for (const key of Object.keys(record.db.placements))
        if (key.startsWith(scope + ":"))
          record.db.placements[key] = rename(record.db.placements[key]);
      const items =
        scope === "todo"
          ? [...record.db.requests, ...record.db.asks]
          : Object.values(record.db.messages).flat();
      for (const item of items)
        if (item.folder) item.folder = rename(item.folder);
    }
    for (const conversation of record.db.conversations) {
      const seeded = fresh.conversations.find((c) => c.id === conversation.id);
      if (seeded) Object.assign(conversation, seeded);
    }
    for (const [id, messages] of Object.entries(fresh.messages)) {
      const visitors = (record.db.messages[id] ?? []).filter(
        (m) => !m.id.startsWith(id + "-"),
      );
      record.db.messages[id] = [...messages, ...visitors];
    }
    for (const request of record.db.requests) {
      const seeded = fresh.requests.find((r) => r.id === request.id);
      if (seeded && scene.id !== "website") request.text = seeded.text;
    }
    for (const ask of record.db.asks) {
      const seeded = fresh.asks.find((a) => a.id === ask.id);
      if (seeded) {
        ask.question = seeded.question;
        ask.options = seeded.options;
        ask.recommendation = seeded.recommendation;
        ask.context = seeded.context;
      }
    }
    for (const plan of record.db.plans)
      if (plan.owner === "向导") plan.owner = "协作员";
  }
  next.contentRevision = 4;
  return migrateGroups(next);
}

function migrateGroups(state: TourState): TourState {
  const next = structuredClone(state);
  for (const scene of initialScenes) {
    if (!next.cache[scene.id]) {
      next.scenes.push(structuredClone(scene));
      next.cache[scene.id] = freshScene(scene);
      continue;
    }
    next.scenes = next.scenes.map((s) =>
      s.id === scene.id ? { ...s, ...scene } : s,
    );
    const record = next.cache[scene.id],
      fresh = freshScene(scene).db;
    const builtIn = (id: string) => id.startsWith(scene.id + "-");
    record.db.conversations = [
      ...fresh.conversations,
      ...record.db.conversations.filter((c) => !builtIn(c.id)),
    ];
    for (const [id, messages] of Object.entries(fresh.messages)) {
      const oldId = id.endsWith("-todo") ? scene.id + "-topic" : id;
      const visitors = (record.db.messages[oldId] ?? []).filter(
        (m) => !m.id.startsWith(oldId + "-"),
      );
      record.db.messages[id] = [...messages, ...visitors];
    }
    delete record.db.messages[scene.id + "-topic"];
    record.ui.conv = scene.id + "-main";
    record.ui.doc = null;
    record.ui.selectedFolders.message = "all";
    record.ui.query = "";
    record.ui.mentions = false;
    record.groups = initialGroupWork(scene, record.db);
  }
  next.contentRevision = 5;
  return refreshDocumentLibrary(next);
}
