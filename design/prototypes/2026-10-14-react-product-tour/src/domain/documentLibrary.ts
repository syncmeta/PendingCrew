import catalog from "../documentLibrary.json";
import type {
  DocumentRecord,
  DataView,
  GroupWork,
  Scene,
  SceneState,
  TourState,
} from "./types";

export const library = catalog;
const workEntries = (record: SceneState): [string, GroupWork][] => [
  [record.ui.conv, record.db],
  ...Object.entries(record.groups ?? {}),
];
/** Add authored material without replacing visitor records, drafts, edits or classifications. */
export function installDocumentLibrary(
  scene: Scene,
  record: SceneState,
  usedIds?: Set<string>,
  resolved?: Map<string, string>,
) {
  const used =
    usedIds ??
    new Set(
      workEntries(record).flatMap(([, work]) => work.docs.map((d) => d.id)),
    );
  const ids = resolved ?? new Map<string, string>();
  for (const [index, item] of catalog.entries()) {
    if (item.scene !== scene.id) continue;
    const conv = scene.id + "-" + item.conversation;
    const work = conv === record.ui.conv ? record.db : record.groups?.[conv];
    if (!work) continue;
    const existing = work.docs.find((d) => d.catalogKey === item.key);
    if (existing) {
      ids.set(item.key, existing.id);
      continue;
    }
    let number = 800001 + index,
      id = "D" + number;
    while (used.has(id)) {
      number++;
      id = "D" + number;
    }
    used.add(id);
    ids.set(item.key, id);
    work.docs.push({
      id,
      catalogKey: item.key,
      title: item.title,
      body: item.body,
      author: "机长",
      folder: item.folder,
      summary: item.summary,
      kind: item.kind as DocumentRecord["kind"],
      sources: item.sources,
      dataView:
        "dataView" in item
          ? (structuredClone(item.dataView) as DataView)
          : undefined,
    });
    if (!work.folders.document.includes(item.folder))
      work.folders.document.push(item.folder);
  }
  // Resolve related records only after every catalog entry has a stable identity.
  for (const [, work] of workEntries(record))
    for (const d of work.docs) {
      const authored = catalog.find((item) => item.key === d.catalogKey);
      if (authored)
        d.related = authored.related
          .map((key) => ids.get(key))
          .filter((id): id is string => Boolean(id));
    }
  for (const [conv, work] of workEntries(record)) {
    const first = work.docs.find((d) => d.catalogKey);
    if (!first) continue;
    if (work.plans[0] && !work.plans[0].evidence)
      work.plans[0].evidence = first.id;
    // Preserve the exact introductory conversation; topical groups expose their own reading entry.
    if (conv === scene.id + "-main") continue;
    const messageId = "library-" + conv;
    if (!record.db.messages[conv]?.some((m) => m.id === messageId))
      record.db.messages[conv]?.push({
        id: messageId,
        sender: "机长",
        text: "本群的说明、设计和相关资料放在文档里，可以分类查看，也可以引用回讨论。",
        ref: first.id,
      });
  }
  return ids;
}
export function refreshDocumentLibrary(state: TourState): TourState {
  const next = structuredClone(state);
  const used = new Set(
    Object.values(next.cache).flatMap((record) =>
      workEntries(record).flatMap(([, work]) => work.docs.map((d) => d.id)),
    ),
  );
  const resolved = new Map<string, string>();
  // Allocate globally, then resolve cross-group and cross-crew references in a second pass.
  for (const scene of next.scenes)
    installDocumentLibrary(scene, next.cache[scene.id], used, resolved);
  for (const scene of next.scenes)
    installDocumentLibrary(scene, next.cache[scene.id], used, resolved);
  next.contentRevision = 7;
  return next;
}
