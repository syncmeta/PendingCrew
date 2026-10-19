import {
  installDocumentLibrary,
  refreshDocumentLibrary,
} from "./documentLibrary";
import content from "../content.json";
import fixtures from "../fixtures.json";
import type { Ledger, Scene, SceneState, TourState, GroupWork } from "./types";
export const initialScenes: Scene[] = content;
export function freshScene(
  scene: Scene,
  desktop = true,
  data?: Ledger,
): SceneState {
  const db = structuredClone(
    data ?? scene.initialLedger ?? fixtures[scene.id as keyof typeof fixtures],
  ) as Ledger;
  const result: SceneState = {
    groups: initialGroupWork(scene, db),
    db,
    ui: {
      tab: "message",
      kind: "requests",
      conv: scene.id + "-main",
      sidebar: "hierarchy",
      collapsed: [],
      selectedFolders: {
        message: "all",
        todo: "all",
        document: "all",
        progress: "all",
      },
      inspector: desktop,
      todoClosed: false,
      membersClosed: false,
      drafts: {},
      scrolls: {},
      doc: null,
      mentions: false,
      query: "",
      mobileChat: false,
      machineClosed: false,
      mode: "human",
      askScope: "crew",
      counter: 0,
    },
  };
  installDocumentLibrary(scene, result);
  return result;
}
export function initialState(
  desktop = true,
  initialScene = "website",
): TourState {
  return refreshDocumentLibrary({
    contentRevision: 7,
    sceneId: initialScenes.some((s) => s.id === initialScene)
      ? initialScene
      : "website",
    scenes: structuredClone(initialScenes),
    cache: Object.fromEntries(
      initialScenes.map((s) => [s.id, freshScene(s, desktop)]),
    ),
    notice: null,
  });
}

export function initialGroupWork(
  scene: Scene,
  db: Ledger,
): Record<string, GroupWork> {
  const result: Record<string, GroupWork> = {};
  for (const [index, conv] of db.conversations.entries()) {
    if (conv.id === scene.id + "-main") continue;
    const offset =
      (initialScenes.findIndex((s) => s.id === scene.id) + 1) * 1000 +
      index * 100;
    const work = structuredClone({
      plans: db.plans,
      requests: db.requests,
      asks: [],
      docs: db.docs,
      folders: db.folders,
      placements: {},
      history: [],
    }) as GroupWork;
    const remap = (id: string) =>
      /^[WNHD]\d+$/.test(id) ? id[0] + (offset + Number(id.slice(1))) : id;
    work.requests = work.requests.slice(0, 1).map((r) => ({
      ...r,
      id: remap(r.id),
      code: "#" + remap(r.id).slice(1),
      text: conv.title + "：" + conv.subtitle,
    }));
    work.plans = work.plans.map((p, i) => ({
      ...p,
      id: remap(p.id),
      parent: p.parent ? remap(p.parent) : null,
      sources: work.requests.map((r) => r.id),
      dependsOn: p.dependsOn.map(remap),
      blockedOn: [],
      status: p.status === "blocked" ? "open" : p.status,
      title: i === 0 ? conv.title + "的工作结构" : p.title,
      owner: "机长",
      evidence: p.evidence ? remap(p.evidence) : undefined,
    }));
    work.docs = work.docs.slice(0, 1).map((d) => ({
      ...d,
      id: remap(d.id),
      title: conv.title + "说明.md",
      author: "机长",
      body:
        conv.subtitle +
        "\n\n" +
        (db.messages[conv.id]?.[0]?.text ?? "围绕当前群的工作持续协作。"),
    }));
    result[conv.id] = work;
  }
  return result;
}
