import type { Classified, Ledger, Request, SceneState } from "./types";
import { crewLedgers } from "./selectors";
export const todoTags = ["紧急", "小工程", "大工程", "前端", "后端"];
export function tagsOf(item: Classified): string[] { return item.tags ?? []; }
export function requestStatus(db: Ledger, item: Request) {
  if (item.status) return item.status;
  const plans = db.plans.filter((p) => p.sources.includes(item.id));
  return plans.length && plans.every((p) => p.status === "done") ? "done" : "open";
}
export function todoCounts(scene: SceneState) {
  const requests = new Map<string, {db: Ledger; item: Request}>();
  const asks = new Map<string, Ledger["asks"][number]>();
  for (const [conv, db] of crewLedgers(scene)) {
    if (!scene.db.conversations.some((c) => c.id === conv)) continue;
    for (const item of db.requests) requests.set(item.id, {db, item});
    for (const item of db.asks) asks.set(item.id, item);
  }
  return {red: [...requests.values()].filter(({db,item}) => !["done", "dropped"].includes(requestStatus(db,item))).length,
    yellow: [...asks.values()].filter((item) => ["open", "later"].includes(item.status)).length};
}
