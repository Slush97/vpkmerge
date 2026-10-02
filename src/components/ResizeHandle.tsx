import { useEffect, useState, type KeyboardEvent, type PointerEvent } from "react";

/** A panel width the user sets by dragging, remembered across launches. */
export function useStoredWidth(key: string, initial: number, min: number, max: number) {
  const clamp = (w: number) => Math.round(Math.min(max, Math.max(min, w)));
  const [width, setWidth] = useState(() => {
    const saved = Number(localStorage.getItem(key));
    return saved ? clamp(saved) : initial;
  });
  useEffect(() => localStorage.setItem(key, String(width)), [key, width]);
  return { width, setWidth: (w: number) => setWidth(clamp(w)), reset: () => setWidth(initial) };
}

/**
 * Drag handle on a panel's inner edge. `side` is the edge it sits on: dragging
 * away from the panel widens it. Arrow keys nudge, double-click resets.
 */
export function ResizeHandle({
  side,
  label,
  width,
  onResize,
  onReset,
}: {
  side: "left" | "right";
  label: string;
  width: number;
  onResize: (width: number) => void;
  onReset: () => void;
}) {
  const grow = side === "right" ? 1 : -1;

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    const startX = e.clientX;
    const startWidth = width;
    handle.setPointerCapture(e.pointerId);
    document.body.style.cursor = "col-resize";
    const move = (ev: globalThis.PointerEvent) => onResize(startWidth + grow * (ev.clientX - startX));
    const up = () => {
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      document.body.style.cursor = "";
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const step = e.key === "ArrowRight" ? 16 : e.key === "ArrowLeft" ? -16 : 0;
    if (!step) return;
    e.preventDefault();
    onResize(width + grow * step);
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label={label}
      aria-valuenow={width}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      onDoubleClick={onReset}
      className={`group absolute inset-y-0 z-10 w-2 cursor-col-resize outline-none ${side === "right" ? "-right-1" : "-left-1"}`}
    >
      <span className="absolute inset-y-0 left-1/2 w-0.5 -translate-x-1/2 transition-colors group-hover:bg-accent group-focus-visible:bg-accent group-active:bg-accent" />
    </div>
  );
}
