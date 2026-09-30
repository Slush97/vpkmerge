import { useState } from "react";
import type { Part } from "../lib/api";
import { argsSummary, prettyJson, prettyToolName } from "../lib/format";
import { Icon, type IconName } from "../lib/icons";

type ToolCall = Extract<Part, { type: "toolCall" }>;
export type ToolResult = Extract<Part, { type: "toolResult" }>;
type State = "running" | "succeeded" | "failed" | "missing";

const meta: Record<State, { icon: IconName; color: string; label: string }> = {
  running: { icon: "loader", color: "text-running", label: "Running" },
  succeeded: { icon: "checkCircle", color: "text-success", label: "Done" },
  failed: { icon: "xCircle", color: "text-danger", label: "Failed" },
  missing: { icon: "alertCircle", color: "text-faint", label: "No result" },
};

export function ToolCard({ call, result, pending }: { call: ToolCall; result: ToolResult | undefined; pending: boolean }) {
  const [open, setOpen] = useState(false);
  const state: State = result ? (result.isError ? "failed" : "succeeded") : pending ? "running" : "missing";
  const m = meta[state];

  return (
    <div className="overflow-hidden rounded-xl bg-surface shadow-card">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="flex h-11 w-full items-center gap-3 px-3.5 text-left transition-colors hover:bg-hover"
      >
        <Icon name={m.icon} size={16} className={`${m.color} ${state === "running" ? "spin" : ""}`} />
        <span className="shrink-0 font-mono text-[13px] font-medium text-text">{prettyToolName(call.name)}</span>
        <span className="min-w-0 flex-1 truncate text-[13px] text-faint">{argsSummary(call.arguments)}</span>
        <span className={`shrink-0 text-[13px] font-medium ${m.color}`}>{m.label}</span>
        <Icon name={open ? "chevUp" : "chevDown"} size={15} className="text-faint" />
      </button>
      {open && (
        <div className="fade-in flex flex-col gap-3 border-t border-line px-3.5 py-3.5">
          <Block label="Arguments" text={prettyJson(call.arguments)} />
          {result && <Block label={result.isError ? "Error" : "Output"} text={result.output || "(empty)"} />}
        </div>
      )}
    </div>
  );
}

function Block({ label, text }: { label: string; text: string }) {
  return (
    <div>
      <div className="mb-1.5 text-[13px] font-semibold text-muted">{label}</div>
      <pre className="selectable max-h-80 overflow-auto whitespace-pre-wrap break-words rounded-lg bg-code px-3.5 py-2.5 font-mono text-[13px] leading-relaxed text-text-2">
        {text}
      </pre>
    </div>
  );
}
