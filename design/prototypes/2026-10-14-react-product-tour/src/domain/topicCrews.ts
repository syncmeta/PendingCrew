import { applyPresentationUpdates } from "./presentationUpdates";
import topicData from "../topicCrews.json";
const topics: Record<
  string,
  { id: string; title: string; host: string; documents: string[] }[]
> = topicData;
import catalog from "../documentLibrary.json";
import content from "../content.json";
import type { GroupWork, Scene, SceneState, TourState } from "./types";
import fixtures from "../fixtures.json";

export function presentTopicCrew(scene: Scene, record: SceneState) {
  const entries = topics[scene.id as keyof typeof topics];
  if (!entries) return;
  const workAt = (conv: string): GroupWork | undefined =>
    conv === record.ui.conv ? record.db : record.groups?.[conv];
  // Move the original document, keeping its identity, edits and references.
  for (const item of catalog.filter((d) => d.scene === scene.id)) {
    const target = workAt(scene.id + "-" + item.conversation);
    if (!target) continue;
    for (const work of [record.db, ...Object.values(record.groups ?? {})]) {
      if (work === target) continue;
      const index = work.docs.findIndex((d) => d.catalogKey === item.key);
      if (index < 0) continue;
      const [doc] = work.docs.splice(index, 1);
      if (!target.docs.some((d) => d.catalogKey === item.key))
        target.docs.push(doc);
      const placement = work.placements["document:" + doc.id];
      if (placement) {
        target.placements["document:" + doc.id] = placement;
        if (!target.folders.document.includes(placement))
          target.folders.document.push(placement);
        delete work.placements["document:" + doc.id];
      }
    }
  }
  for (const { id: suffix, title, host, documents: keys } of entries) {
    const id = scene.id + "-" + suffix;
    const conv = record.db.conversations.find((c) => c.id === id);
    const work = workAt(id);
    if (!conv || !work) continue;
    const articles = catalog.filter((d) => keys.includes(d.key));
    conv.title = title;
    conv.kind = "group";
    conv.subtitle = articles[0]?.summary ?? scene.brief;
    conv.members = [
      {
        name: host,
        detail: "产品资料 · " + title,
        glyph: scene.glyph,
      },
      { name: "人", detail: "人类", glyph: "👤" },
    ];
    const authored = articles.length
      ? articles.map((item) => ({
          id: "topic-" + item.key,
          sender: host,
          text:
            "**" +
            item.title.replace(/\.md$/, "") +
            "**\n\n" +
            item.kind +
            "\n\n" +
            item.body.replace(/^## (.+)$/gm, "**$1**"),
          ref: work.docs.find((d) => d.catalogKey === item.key)?.id,
          folder: item.folder,
        }))
      : [
          {
            id: "topic-" + scene.id + "-overview",
            sender: host,
            text:
              scene.intro +
              "\n\n" +
              entries
                .slice(1)
                .map(
                  ({ title: name, documents: docKeys }) =>
                    "**" +
                    name +
                    "**：" +
                    catalog.find((d) => docKeys.includes(d.key))?.summary,
                )
                .join("\n\n"),
            ref: work.docs[0]?.id,
            folder: scene.folderNames.message[0],
          },
        ];
    const retained = (record.db.messages[id] ?? []).filter(
      (m) =>
        !m.id.startsWith(id + "-") &&
        !m.id.startsWith("topic-") &&
        m.id !== "library-" + id,
    );
    record.db.messages[id] = [...authored, ...retained];
    for (const d of work.docs) if (d.author === "机长") d.author = host;
    for (const p of work.plans) if (p.owner === "机长") p.owner = host;
    // Topic plans express the source document's structure, rather than a copied Demo task tree.
    if (suffix !== "main" && articles[0]) {
      const item = articles[0];
      const legacyTitles = new Set([
        "机长的工作结构",
        "Todo介绍的工作结构",
        "演示策划的工作结构",
        "Demo开发的工作结构",
        title + "的工作结构",
        ...fixtures[scene.id as keyof typeof fixtures].plans.map(
          (p) => p.title,
        ),
      ]);
      const headings = [...item.body.matchAll(/^## (.+)$/gm)].map((m) => m[1]);
      const labels = [
        title,
        headings[0],
        headings[1],
        item.summary,
        headings[2] ?? "来源与相关资料",
      ];
      work.plans.forEach((p, index) => {
        if (legacyTitles.has(p.title)) {
          const original =
            fixtures[scene.id as keyof typeof fixtures].plans[index];
          const defaultStatus =
            original?.status === "blocked" ? "open" : original?.status;
          if (p.status === defaultStatus) {
            p.status = item.kind === "已敲定设计" ? "done" : "open";
            p.nextCheck = "随设计更新";
          }
          p.title = labels[index] ?? p.title;
          p.goal = item.summary;
          p.acceptance = "对应文档中的设计范围与边界";
        }
        if (index === 0 && !p.evidence)
          p.evidence = work.docs.find((d) => d.catalogKey === item.key)?.id;
      });
      for (const doc of work.docs) {
        if (
          [
            "机长说明.md",
            "Todo介绍说明.md",
            "演示策划说明.md",
            "Demo开发说明.md",
          ].includes(doc.title)
        ) {
          doc.title = title + "说明.md";
          const oldSubtitle: Record<string, string> = {
            agent: "围绕目标、任务和判断持续协作。",
            todo: "原始需求、计划和人的事项。",
            planning: "用 PendingCrew 自己介绍自己。",
            development: "React + TypeScript，可交互、可嵌入。",
          };
          const oldMessages = fixtures[scene.id as keyof typeof fixtures]
            .messages as Record<string, { text: string }[]>;
          const oldBody =
            oldSubtitle[suffix] + "\n\n" + oldMessages[id]?.[0]?.text;
          if (doc.body === oldBody)
            doc.body = item.summary + "\n\n" + item.body;
          doc.summary = item.summary;
        }
      }
      for (const request of work.requests)
        if (
          request.text.startsWith("机长：") ||
          request.text.startsWith("Todo介绍：") ||
          request.text.startsWith("演示策划：") ||
          request.text.startsWith("Demo开发：")
        )
          request.text = item.summary;
      const folder = item.folder;
      for (const scope of ["message", "todo", "document", "progress"] as const)
        if (!work.folders[scope].includes(folder))
          work.folders[scope].push(folder);
      for (const p of work.plans) p.folder = folder;
    }
  }
}

export function refreshTopicCrews(state: TourState): TourState {
  for (const authored of content) {
    if (!(authored.id in topics)) continue;
    const scene = state.scenes.find((s) => s.id === authored.id);
    const record = state.cache[authored.id];
    if (!scene || !record) continue;
    Object.assign(scene, structuredClone(authored));
    if (scene.id === "administration") {
      const main =
        record.ui.conv === "administration-main"
          ? record.db
          : record.groups?.["administration-main"];
      if (main) {
        const oldTitles = fixtures.research.plans.map((p) => p.title);
        main.plans.forEach((p, index) => {
          if (p.title === oldTitles[index])
            p.title = fixtures.administration.plans[index]?.title ?? p.title;
        });
        const doc = main.docs.find((d) => d.id === "D401");
        if (
          doc?.title === "机组：组织持续协作.md" &&
          doc.body === fixtures.research.docs[0].body
        ) {
          doc.title = fixtures.administration.docs[0].title;
          doc.body = fixtures.administration.docs[0].body;
        }
      }
    }
    presentTopicCrew(scene, record);
  }
  state.contentRevision = 16;
  return applyPresentationUpdates(state);
}
