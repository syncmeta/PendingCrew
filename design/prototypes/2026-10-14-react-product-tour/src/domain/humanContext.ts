import { refreshTopicCrews } from "./topicCrews";
import content from "../content.json";
import type { Scene, Ledger } from "./types";
import catalog from "../documentLibrary.json";
import type { TourState } from "./types";
export const humanContext = {
  title: "给人的大脑保留上下文",
  paragraphs: [
    "群聊保留讨论的来龙去脉，通讯录让组织与分工清楚，Todo 把需求、进展、卡点和待判断事项留在眼前。",
    "不用把所有事都记在脑子里，也不用每次切换会话都重新理一遍：为什么做、谁在跟进、推进到哪里、下一步需要你判断什么，都有地方可以找到。",
    "PendingCrew 的根本目标是让人与 AI 高效协同、保持清晰思绪，不做“一句话搞定”的产品。",
  ],
};
export const humanContextBody =
  "## " + humanContext.title + "\n\n" + humanContext.paragraphs.join("\n\n");
export function refreshHumanContext(state: TourState): TourState {
  const next = structuredClone(state),
    record = next.cache.website;
  if (record) {
    const authored = [
      "Todo系统是在给人脑做上下文工程",
      "与飞书/企业微信/钉钉的任务跟进相关功能也一样",
    ].map((text, i) => ({ id: "website-todo-" + i, sender: "机长", text }));
    const retained = (record.db.messages["website-todo"] ?? []).filter(
      (m) =>
        !m.id.startsWith("website-todo-") && m.id !== "library-website-todo",
    );
    record.db.messages["website-todo"] = [...authored, ...retained];
    const conv = record.db.conversations.find((c) => c.id === "website-todo");
    if (conv) conv.subtitle = authored[0].text;
    if (
      !record.db.messages["website-main"].some((m) => m.id === "website-main-4")
    )
      record.db.messages["website-main"].push({
        id: "website-main-4",
        sender: "机长",
        text: "可以把 PendingCrew 理解为：把企业微信里的同事换成 Agent 会话。",
        folder: "产品介绍",
      });
    for (const work of [record.db, ...Object.values(record.groups ?? {})])
      for (const d of work.docs) {
        if (
          d.catalogKey === "one-ledger" &&
          !d.body.includes(humanContext.title)
        )
          d.body = humanContextBody + "\n\n" + d.body;
        if (d.catalogKey === "one-ledger")
          d.summary = catalog.find((d) => d.key === "one-ledger")!.summary;
        if (
          d.id === "D1301" &&
          d.title === "Todo介绍说明.md" &&
          !d.body.includes(humanContext.title)
        )
          d.body = humanContextBody + "\n\n" + d.body;
      }
  }
  for (const profile of content.filter((s) =>
    [
      "self-introduction",
      "problems",
      "current-progress",
      "future-five-days",
      "knowledge",
    ].includes(s.id),
  ) as Scene[]) {
    if (!next.cache[profile.id]) {
      next.scenes.push(structuredClone(profile));
      const base = structuredClone(next.cache.website.ui);
      next.cache[profile.id] = {
        db: structuredClone(profile.initialLedger as Ledger),
        groups: structuredClone(profile.initialGroups ?? {}),
        ui: {
          ...base,
          tab: "message",
          kind: "requests",
          conv: profile.id + "-main",
          collapsed: [],
          selectedFolders: {
            message: "all",
            todo: "all",
            document: "all",
            progress: "all",
          },
          drafts: {},
          scrolls: {},
          doc: null,
          query: "",
          mentions: false,
          mode: "human",
          counter: 0,
          mobileChat: false,
        },
      };
    }
  }
  const problems = next.cache.problems;
  const authoredProblems = content.find((s) => s.id === "problems") as Scene;
  if (problems && authoredProblems.initialLedger) {
    const id = "problems-collaboration";
    const authored = authoredProblems.initialLedger.messages[id];
    const visitors = (problems.db.messages[id] ?? []).filter(
      (m) => !m.id.startsWith(id + "-"),
    );
    problems.db.messages[id] = [...structuredClone(authored), ...visitors];
    const conv = problems.db.conversations.find((c) => c.id === id);
    if (conv) conv.subtitle = authored[0].text;
    const work = problems.ui.conv === id ? problems.db : problems.groups?.[id];
    const doc = work?.docs.find((d) => d.id === "Dproblem2");
    if (doc && !doc.body.includes(authored[3].text)) {
      doc.body =
        authored.map((m) => "## " + m.text).join("\n\n") + "\n\n" + doc.body;
      if (doc.title === "协作空间：人、会话与组织.md")
        doc.title = "协作空间：能力、Context与用户.md";
    }
  }
  if (problems && authoredProblems.initialLedger) {
    const id = "problems-remote";
    const authored = authoredProblems.initialLedger.messages[id];
    const visitors = (problems.db.messages[id] ?? []).filter(
      (m) => !m.id.startsWith(id + "-"),
    );
    problems.db.messages[id] = [...structuredClone(authored), ...visitors];
    const conv = problems.db.conversations.find((c) => c.id === id);
    if (conv) conv.subtitle = authored[0].text;
    const work = problems.ui.conv === id ? problems.db : problems.groups?.[id];
    const doc = work?.docs.find((d) => d.id === "Dproblem3");
    const freshDoc = authoredProblems.initialGroups![id].docs[0];
    if (doc) {
      if (
        doc.body ===
        "## 不再围着终端窗口找会话\n\n使用多个 Claude Code、Codex 等会话时，除了写代码，还要找到会话、补上下文、分配工作、追问进度、处理等待确认的事项。统一界面把这些管理动作组织起来。\n\n## 先统一组织和工作上下文\n\n按机组找到项目和负责人，在群里讨论，在 Todo 里跟进，在文档和进展里查看依据与结构。不同 Harness 背后仍是各自的会话，机长负责协调，人的重要判断保持可见。\n\n## 远程管理的产品方向\n\n文档站的前后端解耦与移动端远控设想，是让界面连接运行在其它机器上的后端。例如用手机查看和操作主机上的 Harness，让管理入口不必跟执行机器绑在一起。\n\n## 沿用原文的状态\n\n移动端远控、跨平台和多人 co-drive 在所参考的文档站中属于展望。本 Demo 展示统一界面的组织方式与产品方向，不把这些设想写成已交付的远程能力。"
      )
        doc.body = freshDoc.body;
      if (
        doc.summary ===
        "用统一界面组织散落的 Coding Agent 会话，并把操作界面与执行机器分开的方向讲清楚。"
      )
        doc.summary = freshDoc.summary;
    }
  }

  const currentProgress = next.cache["current-progress"];
  const progressScene = content.find(
    (s) => s.id === "current-progress",
  ) as Scene;
  if (currentProgress && progressScene.initialLedger) {
    const id = "current-progress-main";
    const fresh = progressScene.initialLedger;
    const visitors = (currentProgress.db.messages[id] ?? []).filter(
      (m) => !m.id.startsWith(id + "-"),
    );
    currentProgress.db.messages[id] = [
      ...structuredClone(fresh.messages[id]),
      ...visitors,
    ];
    const conv = currentProgress.db.conversations.find((c) => c.id === id);
    if (conv) conv.subtitle = fresh.messages[id][0].text;
    const work =
      currentProgress.ui.conv === id
        ? currentProgress.db
        : currentProgress.groups?.[id];
    if (work) {
      const doc = work.docs.find((d) => d.id === "Dcurrent-progress-main");
      if (
        doc?.body ===
        "## 讨论起点\n\n截至 2026 年 10 月 21 日，这个交互式产品介绍已经用 React + TypeScript 实现。群聊、Todo、文档/数据和结构化进展都可以操作，接下来把它作为公开的产品讨论入口。\n\n## 需要看清的事情\n\n原生产品仍在推进。仓库最近的主线记录包括 Mac 工作区与四个视图的打磨、iOS 首次连接电脑的引导，以及按工作空间保存未发送草稿。这些是主线开发记录，不能直接等同于所有安装版本都已更新。\n\n## 希望留下的结果\n\n跨平台后端、远程连接与数据工作区也有相应设计和候选验收记录。这里先讲清楚已经做到了哪一步，以及还有哪些体验需要验证，不用一个总完成百分比代替实际进展。"
      )
        Object.assign(doc, structuredClone(fresh.docs[0]));
      const oldTitles = [
        "公开可交互的产品介绍",
        "继续打磨原生协作工作区",
        "验证远程与跨平台使用体验",
      ];
      const ids = new Set(fresh.plans.map((p) => p.id));
      const updated = fresh.plans.map((p) => {
        const old = work.plans.find((q) => q.id === p.id);
        if (old && !oldTitles.includes(old.title)) return old;
        return old
          ? { ...structuredClone(p), note: old.note, owner: old.owner }
          : structuredClone(p);
      });
      work.plans = [...updated, ...work.plans.filter((p) => !ids.has(p.id))];
    }
    const existingScene = next.scenes.find((s) => s.id === "current-progress");
    if (existingScene)
      Object.assign(existingScene, structuredClone(progressScene));
  }

  next.contentRevision = 10;
  return refreshTopicCrews(next);
}
