import { useRef, type PointerEvent } from "react";
export function ResizeHandle({
  name,
}: {
  name: "sidebar" | "conversation" | "todo-height";
}) {
  const start = useRef<{
    x: number;
    y: number;
    initial: number;
    height: number;
    root: HTMLElement;
  } | null>(null);
  const vertical = name === "todo-height";
  function down(event: PointerEvent<HTMLDivElement>) {
    event.preventDefault();
    const root = event.currentTarget.closest<HTMLElement>(".pc-tour")!;
    start.current = {
      x: event.clientX,
      y: event.clientY,
      initial: parseFloat(getComputedStyle(root).getPropertyValue("--" + name)),
      height:
        root.querySelector(".inspector")?.getBoundingClientRect().height ?? 600,
      root,
    };
    event.currentTarget.setPointerCapture(event.pointerId);
  }
  function move(event: PointerEvent<HTMLDivElement>) {
    const s = start.current;
    if (!s) return;
    const value = vertical
      ? Math.max(
          15,
          Math.min(75, s.initial + ((event.clientY - s.y) / s.height) * 100),
        )
      : Math.max(
          name === "sidebar" ? 190 : 165,
          Math.min(
            name === "sidebar" ? 330 : 300,
            s.initial + event.clientX - s.x,
          ),
        );
    s.root.style.setProperty("--" + name, value + (vertical ? "%" : "px"));
  }
  return (
    <div
      className={vertical ? "resize-y" : "resize-x"}
      data-resize={name}
      aria-hidden="true"
      onPointerDown={down}
      onPointerMove={move}
      onPointerUp={() => (start.current = null)}
      onPointerCancel={() => (start.current = null)}
      onLostPointerCapture={() => (start.current = null)}
    />
  );
}
