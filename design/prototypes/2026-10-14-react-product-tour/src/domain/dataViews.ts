import type { DataView } from "./types";
const record = (v: unknown): v is Record<string, unknown> =>
  v !== null && typeof v === "object" && !Array.isArray(v);
const textFields = (v: unknown, keys: string[]) =>
  record(v) && keys.every((key) => typeof v[key] === "string");
export function validDataView(value: unknown): value is DataView {
  if (!record(value) || typeof value.description !== "string") return false;
  if (value.kind === "table")
    return (
      typeof value.primaryKey === "string" &&
      typeof value.schema === "string" &&
      Array.isArray(value.columns) &&
      value.columns.every((c) => textFields(c, ["key", "label", "type"])) &&
      Array.isArray(value.rows) &&
      value.rows.every(
        (r) =>
          record(r) &&
          Object.values(r).every(
            (v) =>
              v === null ||
              typeof v === "string" ||
              (Array.isArray(v) && v.every((x) => typeof x === "string")),
          ),
      )
    );
  if (
    value.kind !== "composition" ||
    !Array.isArray(value.blocks) ||
    !Array.isArray(value.views) ||
    !value.blocks.every((b) => textFields(b, ["id", "title", "body", "source"]))
  )
    return false;
  const ids = new Set(
    value.blocks.map((b) => (b as Record<string, unknown>).id),
  );
  if (ids.size !== value.blocks.length) return false;
  const placements = new Set<string>();
  return value.views.every(
    (v) =>
      textFields(v, ["id", "title"]) &&
      record(v) &&
      Array.isArray(v.placements) &&
      v.placements.every((p) => {
        if (
          !record(p) ||
          !textFields(p, ["id", "blockId"]) ||
          !ids.has(p.blockId) ||
          placements.has(String(p.id))
        )
          return false;
        placements.add(String(p.id));
        return true;
      }),
  );
}
