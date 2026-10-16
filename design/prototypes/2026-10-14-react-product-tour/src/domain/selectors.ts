import type { Classified, Ledger, Scope, SceneState, TourState } from "./types";
export const current = (state: TourState) => state.cache[state.sceneId];
export const currentScene = (state: TourState) =>
  state.scenes.find((s) => s.id === state.sceneId)!;
export function folderOf(db: Ledger, item: Classified, scope: Scope): string {
  if (scope === "todo" && item.id.startsWith("W")) {
    const plan = db.plans.find((p) => p.id === item.id);
    return (
      db.placements["todo:" + item.id] ??
      db.requests.find((r) => plan?.sources.includes(r.id))?.folder ??
      "未分组"
    );
  }
  return db.placements[scope + ":" + item.id] ?? item.folder ?? "未分组";
}
export function filtered<T extends Classified>(
  scene: SceneState,
  items: T[],
  scope: Scope,
): T[] {
  return items.filter(
    (item) =>
      (scene.ui.selectedFolders[scope] === "all" ||
        folderOf(scene.db, item, scope) === scene.ui.selectedFolders[scope]) &&
      (!scene.ui.query ||
        JSON.stringify(item)
          .toLowerCase()
          .includes(scene.ui.query.toLowerCase())),
  );
}
export function findReference(state: TourState, id: string) {
  for (const scene of state.scenes) {
    for (const [convId, db] of crewLedgers(state.cache[scene.id])) {
      const plan = db.plans.find((p) => p.id === id);
      if (plan)
        return {
          kind: "plan" as const,
          sceneId: scene.id,
          convId,
          record: plan,
        };
      const ask = db.asks.find((p) => p.id === id);
      if (ask)
        return { kind: "ask" as const, sceneId: scene.id, convId, record: ask };
      const doc = db.docs.find((p) => p.id === id);
      if (doc)
        return {
          kind: "document" as const,
          sceneId: scene.id,
          convId,
          record: doc,
        };
      const request = db.requests.find((p) => p.id === id);
      if (request)
        return {
          kind: "request" as const,
          sceneId: scene.id,
          convId,
          record: request,
        };
    }
  }
  return null;
}
export function referenceLabel(state: TourState, id: string): string {
  const ref = findReference(state, id);
  if (!ref) return id;
  switch (ref.kind) {
    case "plan":
      return ref.record.code + " · " + ref.record.title;
    case "ask":
      return ref.record.id + " · " + ref.record.question;
    case "document":
      return ref.record.title;
    case "request":
      return ref.record.code + " 原始需求";
  }
}

export function crewLedgers(scene: SceneState): [string, Ledger][] {
  return [
    [scene.ui.conv, scene.db],
    ...Object.entries(scene.groups ?? {}).map(
      ([id, work]) => [id, { ...scene.db, ...work }] as [string, Ledger],
    ),
  ];
}
export function switchConversation(scene: SceneState, id: string) {
  if (scene.ui.conv === id) return;
  if (!scene.db.conversations.some((c) => c.id === id)) return;
  const target = scene.groups?.[id];
  if (target) {
    const { plans, requests, asks, docs, folders, placements, history } =
      scene.db;
    scene.groups![scene.ui.conv] = {
      plans,
      requests,
      asks,
      docs,
      folders,
      placements,
      history,
    };
    Object.assign(scene.db, target);
    delete scene.groups![id];
  }
  scene.ui = {
    ...scene.ui,
    conv: id,
    mobileChat: true,
    doc: null,
    query: "",
    collapsed: [],
    selectedFolders: {
      message: "all",
      todo: "all",
      document: "all",
      progress: "all",
    },
  };
}
