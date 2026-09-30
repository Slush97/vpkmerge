import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { PermissionOption, StoredMessage } from "../lib/api";
import { Icon } from "../lib/icons";
import type { PermissionRequest, TurnState } from "../lib/turns";
import { Markdown } from "./Markdown";
import { ToolCard, type ToolResult } from "./ToolCard";
import { btn } from "./ui";

interface Props {
  messages: StoredMessage[];
  turn: TurnState | undefined;
  onPermission: (requestId: string, optionId: string | null) => void;
}

export function MessageList({ messages, turn, onPermission }: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const stick = useRef(true);
  const busy = turn?.busy ?? false;

  const results = useMemo(() => {
    const map = new Map<string, ToolResult>();
    for (const m of messages) {
      for (const p of m.parts) if (p.type === "toolResult") map.set(p.callId, p);
    }
    return map;
  }, [messages]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  });

  return (
    <div
      ref={scroller}
      onScroll={(e) => {
        const el = e.currentTarget;
        stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
      }}
      className="min-h-0 flex-1 overflow-y-auto px-8"
    >
      <div className="mx-auto flex w-full max-w-[720px] flex-col gap-7 pb-8 pt-8">
        {messages.map((m) => {
          if (m.role === "user") return <UserMessage key={m.id} message={m} />;
          if (m.role === "assistant")
            return <AssistantMessage key={m.id} message={m} results={results} pending={busy} />;
          return null;
        })}
        {turn && busy && <LiveDraft turn={turn} />}
        {turn?.permissions.map((p) => (
          <PermissionCard key={p.requestId} request={p} onRespond={onPermission} />
        ))}
        {turn?.notices.map((n, i) => (
          <div key={i} className="flex items-center gap-2 text-[13.5px] text-muted">
            <Icon name="alertCircle" size={15} className="text-warning" />
            <span>{n}</span>
          </div>
        ))}
        {turn?.error && !busy && (
          <div role="alert" className="fade-in flex gap-3 rounded-xl bg-danger/[0.08] px-4 py-3.5">
            <Icon name="xCircle" size={17} className="mt-px text-danger" />
            <div className="min-w-0">
              <div className="text-[14px] font-semibold text-danger">That didn't go through</div>
              <div className="selectable mt-1 break-words font-mono text-[13px] leading-relaxed text-text-2">
                {turn.error}
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}

function UserMessage({ message }: { message: StoredMessage }) {
  const text = message.parts
    .map((p) => (p.type === "text" ? p.text : ""))
    .filter(Boolean)
    .join("\n\n");
  return (
    <div className="flex justify-end">
      <div className="selectable max-w-[560px] whitespace-pre-wrap break-words rounded-[20px] bg-fill px-4 py-2.5 text-[16px] leading-[1.55] text-text">
        {text}
      </div>
    </div>
  );
}

function AssistantMessage({
  message,
  results,
  pending,
}: {
  message: StoredMessage;
  results: Map<string, ToolResult>;
  pending: boolean;
}) {
  return (
    <div className="flex flex-col gap-3.5">
      {message.parts.map((p, i) => {
        switch (p.type) {
          case "text":
            return <Markdown key={i} text={p.text} />;
          case "reasoning":
            return <Thinking key={i} text={p.text} live={false} />;
          case "toolCall":
            return <ToolCard key={p.id || i} call={p} result={results.get(p.id)} pending={pending} />;
          default:
            return null;
        }
      })}
    </div>
  );
}

function LiveDraft({ turn }: { turn: TurnState }) {
  const toolsRunning = Object.keys(turn.running).length > 0;
  return (
    <div className="flex flex-col gap-3.5">
      {turn.reasoning && <Thinking text={turn.reasoning} live={!turn.draft} />}
      {turn.draft ? (
        <Markdown text={turn.draft} />
      ) : (
        !toolsRunning &&
        !turn.reasoning &&
        turn.permissions.length === 0 && (
          <div className="flex items-center gap-2 text-[13.5px] text-muted" aria-live="polite">
            <span className="flex gap-1" aria-hidden="true">
              <span className="size-1.5 animate-pulse rounded-full bg-faint" />
              <span className="size-1.5 animate-pulse rounded-full bg-faint [animation-delay:150ms]" />
              <span className="size-1.5 animate-pulse rounded-full bg-faint [animation-delay:300ms]" />
            </span>
            <span>Thinking</span>
          </div>
        )
      )}
    </div>
  );
}

function Thinking({ text, live }: { text: string; live: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div>
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="inline-flex h-7 items-center gap-1.5 rounded-md px-1.5 -ml-1.5 text-[13.5px] font-medium text-muted transition-colors hover:bg-hover hover:text-text"
      >
        <Icon name={live ? "loader" : "brain"} size={14} className={live ? "spin text-running" : ""} />
        <span>{live ? "Thinking" : "Thought process"}</span>
        <Icon name={open ? "chevUp" : "chevDown"} size={14} className="text-faint" />
      </button>
      {open && (
        <div className="selectable fade-in mt-2 whitespace-pre-wrap break-words border-l-2 border-line-strong pl-4 text-[14px] leading-relaxed text-muted">
          {text}
        </div>
      )}
    </div>
  );
}

function optionClass(kind: PermissionOption["kind"]): string {
  switch (kind) {
    case "allow_once":
      return btn.primary;
    case "allow_always":
      return btn.secondary;
    case "reject_once":
      return btn.ghost;
    case "reject_always":
      return btn.danger;
  }
}

const order: Record<PermissionOption["kind"], number> = {
  allow_once: 0,
  allow_always: 1,
  reject_once: 2,
  reject_always: 3,
};

function PermissionCard({
  request,
  onRespond,
}: {
  request: PermissionRequest;
  onRespond: (requestId: string, optionId: string | null) => void;
}) {
  const options = [...request.options].sort((a, b) => order[a.kind] - order[b.kind]);
  return (
    <div
      role="group"
      aria-label="Permission request"
      className="pop-in flex flex-col gap-3 rounded-2xl bg-surface p-4 shadow-pop"
    >
      <div className="flex items-start gap-3">
        <span className="inline-flex size-8 shrink-0 items-center justify-center rounded-lg bg-accent/15 text-accent-ink">
          <Icon name="shield" size={16} />
        </span>
        <div className="min-w-0 flex-1">
          <div className="text-[14px] font-semibold text-text">Allow this action?</div>
          <div className="selectable mt-0.5 break-words font-mono text-[13px] leading-relaxed text-text-2">
            {request.title}
          </div>
        </div>
      </div>
      <div className="flex flex-wrap gap-2 pl-11">
        {options.map((o) => (
          <button key={o.optionId} type="button" onClick={() => onRespond(request.requestId, o.optionId)} className={optionClass(o.kind)}>
            {o.name}
          </button>
        ))}
      </div>
    </div>
  );
}
