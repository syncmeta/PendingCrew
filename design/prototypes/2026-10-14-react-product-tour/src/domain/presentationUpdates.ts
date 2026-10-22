import { applyContentCompletion } from "./contentCompletion";
import { applyNativeDemo } from "./nativeDemo";
import { applyDevelopmentExamples } from "./developmentExamples";
import { applyWorkspaceRefresh } from "./workspaceRefresh";
import catalog from "../documentLibrary.json";
import contentData from "../content.json";
import type { Scene } from "./types";
const content = contentData as Scene[];
import type { TourState } from "./types";
export const initialContentUpdatedAt = "2026-10-21T03:32:46.018Z";
const authoredUpdatedAt = "2026-10-21T03:48:43.670Z";
const palantirMessages = [
  "Palantir用企业数据做成一套企业操作系统",
  "我也想弄一个给个人/家庭/小团队用的人机协作系统",
  "数据之间可计算、有关系",
  "一定程度上也类似于把企业的ERP给小团队使用",
];
const slicingUpdatedAt = "2026-10-21T03:56:14.260Z";
const slicingMessages = [
  "文档只是作为数据库的一个视图",
  "人机协作过程中一切的零碎数据和经验都可以存到数据库中，合适时再写视图。其实这就和人类写作的原理和过程一样。",
];
const databaseUpdatedAt = "2026-10-21T03:57:42.170Z";
const databaseMessages = [
  "每个人、每个团队都会产生很多数据",
  "以前没有如此强大的Agent的时候，整理数据的成本高于收益，大家都不弄",
  "就像移动互联网普及后很多原本不会用互联网解决的东西都在线上解决了",
];
const memoryUpdatedAt = "2026-10-21T03:59:54.581Z";
const memoryMessages = [
  "其实就是关系型数据库的特征",
  "各种东西，文档啊，牢骚啊，经验啊，问题啊，或者什么零碎的东西，见闻，都放到数据库里面",
  "有一点数据湖的意味，但是没有那么湖",
  "Agent让原本不会被计算的东西、整理成本太高的但是又确实能带来价值的东西，拿来结构化处理，让它们可计算",
];
const viewsUpdatedAt = "2026-10-21T04:01:17.549Z";
const viewsMessages = [
  "和人类写作过程一样",
  "人类写作也是在不断的实践中积累经验、思考，然后找个时间整理出来，形成的文字在一定程度上就是作者大脑中数据的一个视图",
];
const futureUpdatedAt = "2026-10-21T04:07:12.955Z";
const futureMessages: Record<string, string[]> = {
  "main": [
    "我虽然认为它有价值，但是这样模式的类似产品实在是太多了。我自己也经常陷入价值何在的思考。",
    "我给它的价值定位是优化人机协作的摩擦",
    "我不知道这是否能形成独特价值",
    "或许价值不在这里？"
  ],
  "needs": [
    "我还没有给很多人试过，不清楚真实用户需求",
    "什么东西是用户会觉得自己想要的？很大很重要的问题"
  ],
  "purpose": [
    "做着做着就很迷茫了。"
  ],
  "users": [
    "我目前觉得是为创造者设计的，像Adobe的定位，是创意软件。",
    "但是这似乎有点窄。",
    "也许它更适合其它场景"
  ]
};
const taskflowUpdatedAt = "2026-10-21T04:17:25.109Z";
const taskflowMessages = [
  "一个人、一个团队要面对的事情、想做的事情是源源不断且不断变化的",
  "主流Agent产品大多在想象用户有很多事情等待处理，需要一个帮手",
  "实际上一般人的任务没有那么明确，不是一件件可以写出来交代出来的事情",
  "给Agent交代的任务往往只是一个大目标的其中一个实现小环节",
  "持续性Agent如Dot也只是在持续性上面下功夫，就像分了一个长期陪伴的朋友而不是一个专业人员，可是Dot又想让Agent成为用户的专业人员",
  "专业人员和专业团队需要理解更广泛的背景信息，并且是互相了解，不是像多数Personal AI产品那样把聊天记录、电脑文件拿过来分析，那是刑侦的做事方法"
];
const frictionId = "website-main-friction";
const frictionText = "主要想优化的是人机协作的摩擦。";
export function applyPresentationUpdates(state: TourState): TourState {
  state = applyNativeDemo(applyDevelopmentExamples(applyWorkspaceRefresh(state)));
  const website = state.cache.website, knowledge = state.cache.knowledge;
  const needFriction = website && !website.db.messages["website-main"]?.some((m) => m.id === frictionId && m.text === frictionText);
  const authored = knowledge?.db.messages["knowledge-main"]?.filter((m) => m.id.startsWith("knowledge-main-")) ?? [];
  const needPalantir = knowledge && (authored.length !== palantirMessages.length || authored.some((m, i) => m.text !== palantirMessages[i]));
  const slicing = knowledge?.db.messages["knowledge-slicing"]?.filter((m) => m.id.startsWith("knowledge-slicing-")) ?? [];
  const needSlicing = knowledge && (slicing.length !== slicingMessages.length || slicing.some((m, i) => m.text !== slicingMessages[i]));
  const database = knowledge?.db.messages["knowledge-database"]?.filter((m) => m.id.startsWith("knowledge-database-")) ?? [];
  const needDatabase = knowledge && (database.length !== databaseMessages.length || database.some((m, i) => m.text !== databaseMessages[i]));
  const memory = knowledge?.db.messages["knowledge-memory"]?.filter((m) => m.id.startsWith("knowledge-memory-")) ?? [];
  const needMemory = knowledge && (memory.length !== memoryMessages.length || memory.some((m, i) => m.text !== memoryMessages[i]));
  const views = knowledge?.db.messages["knowledge-views"]?.filter((m) => m.id.startsWith("knowledge-views-")) ?? [];
  const needViews = knowledge && (views.length !== viewsMessages.length || views.some((m, i) => m.text !== viewsMessages[i]));
  const future = state.cache["future-five-days"];
  const needFuture = future && Object.entries(futureMessages).some(([suffix, texts]) => {
    const id = "future-five-days-" + suffix;
    const messages = (future.db.messages[id] ?? []).filter((m) => m.id.startsWith(id + "-"));
    return messages.length !== texts.length || messages.some((m, i) => m.text !== texts[i]);
  });
  const taskflow = state.cache.problems?.db.messages["problems-main"]?.filter((m) => m.id.startsWith("problems-main-")) ?? [];
  const needTaskflow = state.cache.problems && (taskflow.length !== taskflowMessages.length || taskflow.some((m, i) => m.text !== taskflowMessages[i]));
  const needNavigation = state.scenes.some((scene) => scene.id === "self-introduction" || (scene.id === "website" && (scene.name !== "概览和演示" || scene.crew !== "概览和演示")) || (scene.id === "future-five-days" && scene.crew !== "它的未来")) || state.sceneId === "self-introduction";
  const missingTime = Object.values(state.cache).some((c) => !c.updatedAt);
  if (!needFriction && !needPalantir && !needSlicing && !needDatabase && !needMemory && !needViews && !needFuture && !needTaskflow && !missingTime && !needNavigation) return applyContentCompletion(state);
  const next = structuredClone(state);
  for (const record of Object.values(next.cache)) record.updatedAt ??= initialContentUpdatedAt;
  if (needFriction) {
    const record = next.cache.website;
    record.db.messages["website-main"] = record.db.messages["website-main"].filter((m) => m.id !== frictionId);
    record.db.messages["website-main"].push({id: frictionId, sender: "机长", text: frictionText, folder: "产品介绍"});
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(authoredUpdatedAt))).toISOString();
  }
  if (needPalantir) {
    const record = next.cache.knowledge;
    const visitors = (record.db.messages["knowledge-main"] ?? []).filter((m) => !m.id.startsWith("knowledge-main-"));
    record.db.messages["knowledge-main"] = [...palantirMessages.map((text, i) => ({id: "knowledge-main-" + i, sender: "资料设计", text, folder: "信息组织"})), ...visitors];
    const conv = record.db.conversations.find((c) => c.id === "knowledge-main");
    if (conv) conv.subtitle = palantirMessages[0];
    const work = record.ui.conv === "knowledge-main" ? record.db : record.groups?.["knowledge-main"];
    const doc = work?.docs.find((d) => d.id === "Dknowledge-main");
    const sourceBody = catalog.find((d) => d.key === "palantir-model")?.body;
    if (doc && doc.body === sourceBody) {
      doc.body = palantirMessages.map((text, i) => (i + 1) + ". " + text).join("\n\n") + "\n\n" + doc.body;
      doc.summary = palantirMessages[0];
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(authoredUpdatedAt))).toISOString();
  }
  if (needSlicing) {
    const record = next.cache.knowledge;
    const visitors = (record.db.messages["knowledge-slicing"] ?? []).filter((m) => !m.id.startsWith("knowledge-slicing-"));
    record.db.messages["knowledge-slicing"] = [...slicingMessages.map((text, i) => ({id: "knowledge-slicing-" + i, sender: "资料设计", text, folder: "信息组织"})), ...visitors];
    const conv = record.db.conversations.find((c) => c.id === "knowledge-slicing");
    if (conv) conv.subtitle = slicingMessages[0];
    const work = record.ui.conv === "knowledge-slicing" ? record.db : record.groups?.["knowledge-slicing"];
    const doc = work?.docs.find((d) => d.id === "Dknowledge-slicing");
    if (doc && doc.body === catalog.find((d) => d.key === "semantic-slicing")?.body) {
      doc.body = slicingMessages.map((text, i) => (i + 1) + ". " + text).join("\n\n");
      doc.summary = slicingMessages[0];
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(slicingUpdatedAt))).toISOString();
  }
  if (needDatabase) {
    const record = next.cache.knowledge;
    const visitors = (record.db.messages["knowledge-database"] ?? []).filter((m) => !m.id.startsWith("knowledge-database-"));
    record.db.messages["knowledge-database"] = [...databaseMessages.map((text, i) => ({id: "knowledge-database-" + i, sender: "资料设计", text, folder: "信息组织"})), ...visitors];
    const conv = record.db.conversations.find((c) => c.id === "knowledge-database");
    if (conv) conv.subtitle = databaseMessages[0];
    const work = record.ui.conv === "knowledge-database" ? record.db : record.groups?.["knowledge-database"];
    const doc = work?.docs.find((d) => d.id === "Dknowledge-database");
    if (doc && doc.body === catalog.find((d) => d.key === "sqlite-direction")?.body) {
      doc.body = databaseMessages.map((text, i) => (i + 1) + ". " + text).join("\n\n");
      doc.summary = databaseMessages[0];
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(databaseUpdatedAt))).toISOString();
  }
  if (needMemory) {
    const record = next.cache.knowledge;
    const visitors = (record.db.messages["knowledge-memory"] ?? []).filter((m) => !m.id.startsWith("knowledge-memory-"));
    record.db.messages["knowledge-memory"] = [...memoryMessages.map((text, i) => ({id: "knowledge-memory-" + i, sender: "资料设计", text, folder: "信息组织"})), ...visitors];
    const conv = record.db.conversations.find((c) => c.id === "knowledge-memory");
    if (conv) conv.subtitle = memoryMessages[0];
    const work = record.ui.conv === "knowledge-memory" ? record.db : record.groups?.["knowledge-memory"];
    const doc = work?.docs.find((d) => d.id === "Dknowledge-memory");
    const sourceDoc = content.find((s) => s.id === "knowledge")?.initialGroups?.["knowledge-memory"]?.docs[0];
    if (doc && doc.body === sourceDoc?.body) {
      doc.body = memoryMessages.map((text, i) => (i + 1) + ". " + text).join("\n\n");
      doc.summary = memoryMessages[0];
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(memoryUpdatedAt))).toISOString();
  }
  if (needViews) {
    const record = next.cache.knowledge;
    const visitors = (record.db.messages["knowledge-views"] ?? []).filter((m) => !m.id.startsWith("knowledge-views-"));
    record.db.messages["knowledge-views"] = [...viewsMessages.map((text, i) => ({id: "knowledge-views-" + i, sender: "资料设计", text, folder: "信息组织"})), ...visitors];
    const conv = record.db.conversations.find((c) => c.id === "knowledge-views");
    if (conv) conv.subtitle = viewsMessages[0];
    const work = record.ui.conv === "knowledge-views" ? record.db : record.groups?.["knowledge-views"];
    const doc = work?.docs.find((d) => d.id === "Dknowledge-views");
    if (doc && doc.body === catalog.find((d) => d.key === "custom-tables")?.body) {
      doc.body = viewsMessages.map((text, i) => (i + 1) + ". " + text).join("\n\n");
      doc.summary = viewsMessages[0];
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(viewsUpdatedAt))).toISOString();
  }
  if (needFuture) {
    const record = next.cache["future-five-days"];
    const sourceScene = content.find((s) => s.id === "future-five-days");
    for (const [suffix, texts] of Object.entries(futureMessages)) {
      const id = "future-five-days-" + suffix;
      const conv = record.db.conversations.find((c) => c.id === id);
      const sender = record.db.messages[id]?.find((m) => m.id.startsWith(id + "-"))?.sender ?? "产品讨论";
      const visitors = (record.db.messages[id] ?? []).filter((m) => !m.id.startsWith(id + "-"));
      record.db.messages[id] = [...texts.map((text, i) => ({id: id + "-" + i, sender, text, folder: "产品讨论"})), ...visitors];
      if (conv) conv.subtitle = texts[0];
      const work = record.ui.conv === id ? record.db : record.groups?.[id];
      const sourceWork = suffix === "main" ? sourceScene?.initialLedger : sourceScene?.initialGroups?.[id];
      const doc = work?.docs.find((d) => d.id === "D" + id);
      if (doc && doc.body === sourceWork?.docs[0]?.body) {
        doc.body = texts.map((text, i) => (i + 1) + ". " + text).join("\n\n");
        doc.summary = texts[0];
      }
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(futureUpdatedAt))).toISOString();
  }
  if (needTaskflow) {
    const record = next.cache.problems;
    const visitors = (record.db.messages["problems-main"] ?? []).filter((m) => !m.id.startsWith("problems-main-"));
    record.db.messages["problems-main"] = [...taskflowMessages.map((text, i) => ({id: "problems-main-" + i, sender: "工作跟进", text, folder: "产品问题"})), ...visitors];
    const conv = record.db.conversations.find((c) => c.id === "problems-main");
    if (conv) conv.subtitle = taskflowMessages[0];
    const work = record.ui.conv === "problems-main" ? record.db : record.groups?.["problems-main"];
    const doc = work?.docs.find((d) => d.id === "Dproblem1");
    const sourceDoc = content.find((s) => s.id === "problems")?.initialLedger?.docs[0];
    if (doc && doc.body === sourceDoc?.body) {
      doc.body = taskflowMessages.map((text, i) => (i + 1) + ". " + text).join("\n\n");
      doc.summary = taskflowMessages[0];
    }
    record.updatedAt = new Date(Math.max(Date.parse(record.updatedAt!), Date.parse(taskflowUpdatedAt))).toISOString();
  }
  next.scenes = next.scenes.filter((scene) => scene.id !== "self-introduction");
  const overviewScene = next.scenes.find((scene) => scene.id === "website");
  if (overviewScene) { overviewScene.name = "概览和演示"; overviewScene.crew = "概览和演示"; }
  const futureScene = next.scenes.find((scene) => scene.id === "future-five-days");
  if (futureScene) futureScene.crew = "它的未来";
  if (next.sceneId === "self-introduction") next.sceneId = "website";
  next.presentationRevision = 8;
  return applyContentCompletion(next);
}
export function formatUpdateTime(iso: string, now = Date.now()) {
  const date = new Date(iso);
  const options = {timeZone: "Asia/Taipei", hour12: false} as const;
  if (!Number.isFinite(date.getTime())) return {short: "—", full: "时间未知"};
  const full = new Intl.DateTimeFormat("zh-CN", {...options, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit"}).format(date);
  const minutes = Math.max(0, Math.floor((now - date.getTime()) / 60000));
  const day = (value: Date) => new Intl.DateTimeFormat("en-CA", {...options, year:"numeric", month:"2-digit", day:"2-digit"}).format(value);
  const short = minutes < 1 ? "刚刚" : minutes < 60 ? minutes + "分钟前" : day(date) === day(new Date(now)) ? full.split(" ").at(-1)! : full.replace(/^\d{4}\//, "");
  return {short, full};
}
