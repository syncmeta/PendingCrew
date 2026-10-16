import { test } from "node:test";
import assert from "node:assert/strict";
import { initialState } from "./seed";
import { loadState, saveState } from "../state/persistence";
function storage() {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (k: string) => values.get(k) ?? null,
      setItem: (k: string, v: string) => values.set(k, v),
      removeItem: (k: string) => values.delete(k),
    },
  });
  return values;
}
test("legacy browser state migrates without overwriting its original key", () => {
  const values = storage();
  const seed = initialState();
  seed.cache.website.ui.drafts["website-main"] = "旧版草稿";
  seed.cache.website.db.plans[2].note = "旧版修改";
  const old = {
    version: 1,
    sceneId: "website",
    cache: Object.fromEntries(
      Object.entries(seed.cache).map(([id, s]) => [id, { db: s.db, ...s.ui }]),
    ),
  };
  const text = JSON.stringify(old);
  values.set("pendingcrew-product-tour-v1", text);
  const restored = loadState("new", true, "website");
  assert.equal(restored.cache.website.ui.drafts["website-main"], "旧版草稿");
  assert.equal(restored.cache.website.db.plans[2].note, "旧版修改");
  assert.equal(values.get("pendingcrew-product-tour-v1"), text);
  saveState("new", restored);
  assert.equal(
    loadState("new", true, "website").cache.website.db.plans[2].note,
    "旧版修改",
  );
});
test("malformed saved content falls back instead of crashing the component", () => {
  const values = storage();
  const seed = initialState();
  const bad = JSON.parse(JSON.stringify(seed));
  bad.cache.website.db.messages["website-main"][0].text = null;
  values.set("broken", JSON.stringify({ version: 2, state: bad }));
  assert.equal(
    loadState("broken", true, "website", false).cache.website.db.messages[
      "website-main"
    ][0].text,
    seed.cache.website.db.messages["website-main"][0].text,
  );
});
test("storage being unavailable does not prevent using the tour", () => {
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    get() {
      throw Error("denied");
    },
  });
  const state = loadState("unavailable", true, "trip");
  assert.equal(state.sceneId, "trip");
  assert.doesNotThrow(() => saveState("unavailable", state));
});
