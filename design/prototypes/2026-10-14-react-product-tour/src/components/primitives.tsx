import type { ButtonHTMLAttributes, CSSProperties, ReactNode } from "react";
import { useTour } from "../state/TourContext";
import { referenceLabel } from "../domain/selectors";
import { statusLabels, type Status } from "../domain/types";
export function Button({
  action,
  className = "text-button",
  children,
  ...props
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  action?: string;
  children?: ReactNode;
}) {
  return (
    <button type="button" className={className} data-action={action} {...props}>
      {children}
    </button>
  );
}
export function Icon({ name }: { name: string }) {
  const content: Record<string, ReactNode> = {
    search: (
      <>
        <circle cx="7" cy="7" r="4.8" />
        <path d="m10.7 10.7 3.5 3.5" />
      </>
    ),
    send: <path d="M8 13V3M3.5 7.5 8 3l4.5 4.5" />,
    checklist: (
      <>
        <path d="m1 4 1.5 1.5L5 2.5M7 4h7M1 9h3M7 9h7M1 13h3M7 13h7" />
      </>
    ),
    plus: <path d="M8 2v12M2 8h12" />,
    refresh: <path d="M13 5a5.5 5.5 0 1 0 0 6M13 1v4H9" />,
    panel: (
      <>
        <rect x="1.5" y="2" width="13" height="12" rx="2" />
        <path d="M10 2v12" />
      </>
    ),
    tag: <><path d="M2 2h6l6 6-6 6-6-6z" /><circle cx="5" cy="5" r="1" /></>,
    folder: <path d="M1.5 4V13h13V5H8L6 3H2z" />,
    file: <path d="M3 1.5h7l3 3V14H3zM10 2v3h3M5 8h6M5 11h6" />,
    menu: <path d="M2 4h12M2 8h12M2 12h12" />,
    arrow: <path d="m9 3-5 5 5 5" />,
    info: (
      <>
        <circle cx="8" cy="8" r="6.5" />
        <path d="M8 7v5M8 4v.5" />
      </>
    ),
    more: (
      <>
        <circle cx="3" cy="8" r=".5" />
        <circle cx="8" cy="8" r=".5" />
        <circle cx="13" cy="8" r=".5" />
      </>
    ),
  };
  return (
    <svg className="icon" viewBox="0 0 16 16" aria-hidden="true">
      {content[name] ?? content.info}
    </svg>
  );
}
export function StatusBadge({ status }: { status: Status }) {
  return <span className={"badge " + status}>{statusLabels[status]}</span>;
}
export function Reference({
  id,
  className = "link-pill",
}: {
  id: string;
  className?: string;
}) {
  const { state, reference } = useTour();
  return (
    <Button
      action="reference"
      data-id={id}
      className={className}
      onClick={() => reference(id)}
    >
      {referenceLabel(state, id)}
    </Button>
  );
}
const glyphs = [
  "🦊",
  "🐼",
  "🐯",
  "🦁",
  "🐸",
  "🐧",
  "🐳",
  "🐙",
  "🦉",
  "🦄",
  "🐝",
  "🦋",
  "🐢",
  "🐬",
  "🦒",
  "🦔",
  "🌵",
  "🌻",
  "🌸",
  "🌙",
  "⭐",
  "🔥",
  "🍄",
  "🍀",
  "🍓",
  "🍑",
  "🍋",
  "🍇",
  "🥑",
  "🌽",
  "🥨",
  "🍪",
  "🎈",
  "🎨",
  "🎭",
  "🎪",
  "🎲",
  "🧩",
  "🪁",
  "🪐",
  "🚀",
  "⛵",
  "🏕",
  "🪴",
  "🪨",
  "💎",
  "🧊",
  "🦤",
];
export function fnv(seed: string) {
  let hash = 1469598103934665603n;
  for (const c of seed)
    hash = BigInt.asUintN(
      64,
      (hash ^ BigInt(c.codePointAt(0)!)) * 1099511628211n,
    );
  return hash;
}
export function groupGlyphs(seed: string) {
  let hash = fnv(seed);
  const picked: string[] = [];
  while (picked.length < 3) {
    const g = glyphs[Number(hash % 48n)];
    if (!picked.includes(g)) picked.push(g);
    hash = BigInt.asUintN(
      64,
      hash * 6364136223846793005n + 1442695040888963407n,
    );
  }
  return picked;
}
export function Avatar({
  seed,
  group = false,
  glyph = "🦉",
}: {
  seed: string;
  group?: boolean;
  glyph?: string;
}) {
  return group ? (
    <span className="avatar group" aria-hidden="true">
      {groupGlyphs(seed).map((g, i) => (
        <span
          key={g}
          style={
            {
              "--dx": [0, -5.3, 5.3][i] + "px",
              "--dy": [-6.1, 3.1, 3.1][i] + "px",
            } as CSSProperties
          }
        >
          {g}
        </span>
      ))}
    </span>
  ) : (
    <span className="avatar single" aria-hidden="true">
      {glyph}
    </span>
  );
}
export function crewColor(id: string) {
  const hue = Number(fnv(id) % 360n),
    v = 0.96,
    s = 0.72,
    c = v * s,
    x = c * (1 - Math.abs(((hue / 60) % 2) - 1)),
    m = v - c;
  const rgb =
    hue < 60
      ? [c, x, 0]
      : hue < 120
        ? [x, c, 0]
        : hue < 180
          ? [0, c, x]
          : hue < 240
            ? [0, x, c]
            : hue < 300
              ? [x, 0, c]
              : [c, 0, x];
  return (
    "#" +
    rgb
      .map((n) =>
        Math.round((n + m) * 255)
          .toString(16)
          .padStart(2, "0"),
      )
      .join("")
  );
}
export function crewChain(id: string) {
  const parents: Record<string, string> = {
    website: "crew-product",
    trip: "crew-product",
    research: "trip",
    administration: "crew-product",
    "self-introduction": "crew-product",
  };
  const chain = [id],
    seen = new Set(chain);
  let parent =
    parents[id] ?? (id.startsWith("custom-") ? "crew-product" : undefined);
  while (parent && !seen.has(parent) && chain.length < 8) {
    chain.push(parent);
    seen.add(parent);
    parent = parents[parent];
  }
  return chain;
}
export function Lineage({ id }: { id: string }) {
  return (
    <span
      className="lineage"
      data-crew-color={id}
      style={{
        background: crewColor(id),
      }}
    />
  );
}
