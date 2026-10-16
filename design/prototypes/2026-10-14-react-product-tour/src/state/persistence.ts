import { refreshContent } from "../domain/contentMigration";
import { freshScene, initialState } from "../domain/seed";
import type {
  Ledger,
  Scene,
  SceneState,
  TourState,
  UiState,
} from "../domain/types";
const record = (v: unknown): v is Record<string, unknown> =>
  typeof v === "object" && v !== null && !Array.isArray(v);
const rows = (v: unknown): v is (Record<string, unknown> & { id: string })[] =>
  Array.isArray(v) && v.every((x) => record(x) && typeof x.id === "string");
const strings = (v: unknown): v is string[] =>
  Array.isArray(v) && v.every((x) => typeof x === "string");
const stringMap = (v: unknown) =>
  record(v) && Object.values(v).every((x) => typeof x === "string");
const scopes = ["message", "todo", "document", "progress"];
function validLedger(v: unknown): v is Ledger {
  if (
    !record(v) ||
    !rows(v.plans) ||
    !rows(v.requests) ||
    !rows(v.docs) ||
    !rows(v.asks) ||
    !rows(v.conversations) ||
    !record(v.messages) ||
    !stringMap(v.placements) ||
    !record(v.folders) ||
    !Array.isArray(v.history)
  )
    return false;
  return (
    v.plans.length > 0 &&
    v.plans.every(
      (p) =>
        ["code", "title", "owner", "note", "nextCheck"].every(
          (k) => typeof p[k] === "string",
        ) &&
        ["open", "active", "blocked", "done", "dropped"].includes(
          String(p.status),
        ) &&
        (p.parent === null || typeof p.parent === "string") &&
        strings(p.sources) &&
        strings(p.dependsOn) &&
        strings(p.blockedOn),
    ) &&
    v.requests.every(
      (r) => typeof r.text === "string" && typeof r.code === "string",
    ) &&
    v.docs.every((d) =>
      ["title", "body", "author"].every((k) => typeof d[k] === "string"),
    ) &&
    v.asks.every(
      (a) =>
        ["question", "recommendation", "item", "context", "answer"].every(
          (k) => typeof a[k] === "string",
        ) &&
        strings(a.options) &&
        ["open", "later", "answered", "withdrawn"].includes(String(a.status)),
    ) &&
    v.conversations.length > 0 &&
    v.conversations.every(
      (c) =>
        typeof c.title === "string" &&
        ["group", "dm"].includes(String(c.kind)) &&
        Array.isArray((v.messages as Record<string, unknown>)[c.id]),
    ) &&
    Object.values(v.messages).every(
      (msgs) =>
        rows(msgs) &&
        msgs.every(
          (m) => typeof m.text === "string" && typeof m.sender === "string",
        ),
    ) &&
    scopes.every((k) => strings((v.folders as Record<string, unknown>)[k])) &&
    v.history.every(
      (h) =>
        record(h) && typeof h.text === "string" && typeof h.time === "string",
    )
  );
}
function validScene(v: unknown): v is Scene {
  return (
    record(v) &&
    [
      "id",
      "name",
      "crew",
      "root",
      "brief",
      "project",
      "request",
      "question",
      "recommendation",
      "leaf",
      "docTitle",
      "docBody",
      "agent",
      "glyph",
      "note",
      "intro",
    ].every((k) => typeof v[k] === "string") &&
    strings(v.steps) &&
    strings(v.options) &&
    record(v.folderNames) &&
    scopes.every((k) =>
      strings((v.folderNames as Record<string, unknown>)[k]),
    ) &&
    (!v.initialLedger || validLedger(v.initialLedger))
  );
}
function validUi(v: unknown): v is UiState {
  return (
    record(v) &&
    scopes.includes(String(v.tab)) &&
    ["requests", "plans", "asks"].includes(String(v.kind)) &&
    ["hierarchy", "timeline"].includes(String(v.sidebar)) &&
    ["human", "all"].includes(String(v.mode)) &&
    ["crew", "all"].includes(String(v.askScope)) &&
    typeof v.conv === "string" &&
    typeof v.query === "string" &&
    (v.doc === null || typeof v.doc === "string") &&
    typeof v.counter === "number" &&
    strings(v.collapsed) &&
    stringMap(v.drafts) &&
    record(v.selectedFolders) &&
    scopes.every(
      (k) =>
        typeof (v.selectedFolders as Record<string, unknown>)[k] === "string",
    ) &&
    [
      "inspector",
      "todoClosed",
      "membersClosed",
      "mentions",
      "mobileChat",
      "machineClosed",
    ].every((k) => typeof v[k] === "boolean")
  );
}
export function loadState(
  key: string,
  desktop: boolean,
  initialScene: string,
  migrateLegacy = true,
): TourState {
  const base = initialState(desktop, initialScene);
  try {
    const raw = JSON.parse(localStorage.getItem(key) ?? "null") as unknown;
    if (record(raw) && raw.version === 2 && record(raw.state)) {
      const value = raw.state;
      if (
        typeof value.sceneId === "string" &&
        Array.isArray(value.scenes) &&
        record(value.cache) &&
        value.scenes.every(
          (s) =>
            validScene(s) &&
            Boolean((value.cache as Record<string, unknown>)[s.id]),
        ) &&
        Object.values(value.cache).every((s) => {
          if (!record(s) || !validLedger(s.db) || !validUi(s.ui)) return false;
          if (
            s.groups &&
            (!record(s.groups) ||
              !Object.values(s.groups).every(
                (work) =>
                  record(work) &&
                  [
                    "plans",
                    "requests",
                    "asks",
                    "docs",
                    "folders",
                    "placements",
                    "history",
                  ].every((k) => k in work) &&
                  validLedger({ ...(s.db as Ledger), ...work }),
              ))
          )
            return false;
          const ui = s.ui;
          return s.db.conversations.some((c) => c.id === ui.conv);
        }) &&
        value.cache[value.sceneId]
      ) {
        const state = value as unknown as TourState;
        for (const s of Object.values(state.cache)) {
          s.ui.scrolls ??= {};
          s.ui.inspector = desktop && s.ui.inspector;
        }
        state.notice = null;
        return refreshContent(state);
      }
    }
    if (migrateLegacy) {
      const old = JSON.parse(
        localStorage.getItem("pendingcrew-product-tour-v1") ?? "null",
      ) as unknown;
      if (record(old) && old.version === 1 && record(old.cache)) {
        base.contentRevision = 1;
        if (Array.isArray(old.extraScenarios))
          for (const item of old.extraScenarios) {
            if (
              !validScene(item) ||
              !item.id.startsWith("custom-") ||
              base.cache[item.id]
            )
              continue;
            const cached = old.cache[item.id];
            if (!record(cached) || !validLedger(cached.db)) continue;
            const scene = {
              ...item,
              initialLedger: structuredClone(cached.db),
            };
            base.scenes.push(scene);
            base.cache[scene.id] = freshScene(scene, desktop);
          }
        for (const [id, value] of Object.entries(old.cache)) {
          if (!base.cache[id] || !record(value) || !validLedger(value.db))
            continue;
          const { db, ...ui } = value;
          const candidate = {
            ...base.cache[id].ui,
            ...ui,
            scrolls: {},
            inspector: desktop && Boolean(value.inspector),
          };
          if (
            validUi(candidate) &&
            db.conversations.some((c) => c.id === candidate.conv)
          )
            base.cache[id] = { db, ui: candidate } as SceneState;
        }
        if (typeof old.sceneId === "string" && base.cache[old.sceneId])
          base.sceneId = old.sceneId;
      }
    }
  } catch {
    /* Storage unavailable or invalid: the interactive content still works. */
  }
  return refreshContent(base);
}
export function saveState(key: string, state: TourState): void {
  try {
    localStorage.setItem(
      key,
      JSON.stringify({ version: 2, state: { ...state, notice: null } }),
    );
  } catch {
    /* No storage is required to interact. */
  }
}
