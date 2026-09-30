import { useEffect, useMemo, useRef, useState } from "react";
import type { Session } from "../lib/api";
import type { ChoiceInfo } from "../lib/choice";
import { groupSessions } from "../lib/format";
import { Icon } from "../lib/icons";
import type { TurnState } from "../lib/turns";
import { btn, Dot, Kbd, Mark, SrOnly } from "./ui";

interface SidebarProps {
  sessions: Session[];
  activeId: string | null;
  turns: Record<string, TurnState>;
  collapsed: boolean;
  searchFocusSignal: number;
  choice: ChoiceInfo;
  onToggle: () => void;
  onSearch: () => void;
  onSelect: (id: string) => void;
  onNew: () => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
  onOpenSettings: () => void;
}

export function Sidebar(props: SidebarProps) {
  return props.collapsed ? <Rail {...props} /> : <Expanded {...props} />;
}

const railButton =
  "inline-flex size-9 items-center justify-center rounded-lg text-muted transition-colors hover:bg-hover hover:text-text";

function Rail({ choice, onToggle, onNew, onSearch, onOpenSettings }: SidebarProps) {
  return (
    <nav aria-label="Sessions" className="flex h-full w-[60px] shrink-0 flex-col items-center gap-1 border-r border-line bg-sidebar pb-3">
      <div data-tauri-drag-region className="flex h-[52px] w-full shrink-0 items-center justify-center">
        <Mark size={26} />
      </div>
      <button type="button" aria-label="Expand sidebar" title="Expand sidebar (Ctrl B)" onClick={onToggle} className={railButton}>
        <Icon name="panelLeft" size={18} />
      </button>
      <button type="button" aria-label="New session" title="New session (Ctrl N)" onClick={onNew} className={railButton}>
        <Icon name="compose" size={17} />
      </button>
      <button type="button" aria-label="Search sessions" title="Search (Ctrl K)" onClick={onSearch} className={railButton}>
        <Icon name="search" size={18} />
      </button>
      <div data-tauri-drag-region className="w-full flex-1" />
      <button
        type="button"
        aria-label={`Settings. ${choice.problem ?? choice.label}`}
        title="Settings (Ctrl ,)"
        onClick={onOpenSettings}
        className={`${railButton} relative`}
      >
        <Icon name="settings" size={18} />
        {!choice.ready && (
          <span className="absolute right-1.5 top-1.5 size-2.5 rounded-full border-2 border-sidebar bg-warning" />
        )}
      </button>
    </nav>
  );
}

function Expanded({
  sessions,
  activeId,
  turns,
  searchFocusSignal,
  choice,
  onToggle,
  onSelect,
  onNew,
  onRename,
  onDelete,
  onOpenSettings,
}: SidebarProps) {
  const [query, setQuery] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (searchFocusSignal) searchRef.current?.focus();
  }, [searchFocusSignal]);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    return groupSessions(q ? sessions.filter((s) => s.title.toLowerCase().includes(q)) : sessions);
  }, [sessions, query]);

  return (
    <nav aria-label="Sessions" className="flex h-full w-[264px] shrink-0 flex-col border-r border-line bg-sidebar">
      <div data-tauri-drag-region className="flex h-[52px] shrink-0 items-center gap-2.5 pl-4 pr-2">
        <Mark />
        <span data-tauri-drag-region className="text-[15px] font-semibold tracking-[-0.01em]">
          Workbench
        </span>
        <span data-tauri-drag-region className="flex-1 self-stretch" />
        <button type="button" aria-label="Collapse sidebar" title="Collapse sidebar (Ctrl B)" onClick={onToggle} className={btn.icon}>
          <Icon name="panelLeft" size={17} />
        </button>
      </div>

      <div className="flex shrink-0 flex-col gap-2 px-3 pb-1 pt-1">
        <button
          type="button"
          onClick={onNew}
          className={`group flex h-9 items-center gap-2.5 rounded-lg px-3 text-[14px] font-medium transition-colors ${
            activeId === null ? "bg-active text-text" : "text-text hover:bg-hover"
          }`}
        >
          <Icon name="compose" size={16} className="text-accent-ink" />
          <span>New session</span>
          <span className="flex-1" />
          <Kbd className="opacity-0 transition-opacity group-hover:opacity-100">Ctrl N</Kbd>
        </button>
        <label className="flex h-9 items-center gap-2 rounded-lg bg-fill px-3 text-faint transition-shadow focus-within:shadow-[0_0_0_3px_color-mix(in_srgb,var(--accent)_25%,transparent)]">
          <Icon name="search" size={15} />
          <input
            ref={searchRef}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                setQuery("");
                e.currentTarget.blur();
              }
            }}
            placeholder="Search"
            aria-label="Search sessions"
            className="min-w-0 flex-1 bg-transparent text-[14px] text-text outline-none placeholder:text-faint focus-visible:outline-none"
          />
          {!query && <Kbd>Ctrl K</Kbd>}
        </label>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 pb-3">
        {sessions.length === 0 ? (
          <p className="px-3 pt-5 text-[13.5px] leading-snug text-faint">Your sessions will show up here.</p>
        ) : groups.length === 0 ? (
          <p className="px-3 pt-5 text-[13.5px] text-faint">No sessions match.</p>
        ) : (
          groups.map((g) => (
            <div key={g.label} className="flex flex-col gap-px pt-4">
              <span className="px-3 pb-1.5 text-[13px] font-semibold text-muted">{g.label}</span>
              {g.items.map((s) => (
                <SessionRow
                  key={s.id}
                  session={s}
                  active={s.id === activeId}
                  turn={turns[s.id]}
                  onSelect={onSelect}
                  onRename={onRename}
                  onDelete={onDelete}
                />
              ))}
            </div>
          ))
        )}
      </div>

      <div className="flex shrink-0 flex-col gap-px border-t border-line px-3 pb-3 pt-2">
        <button
          type="button"
          onClick={onOpenSettings}
          className="flex h-9 items-center gap-2.5 rounded-lg px-3 text-left transition-colors hover:bg-hover"
        >
          <Dot color={choice.ready ? "var(--success)" : "var(--warning)"} />
          <span className="min-w-0 flex-1 truncate text-[13.5px] text-text-2">
            {choice.problem ?? choice.label}
          </span>
          {choice.ready && choice.detail && (
            <span className="shrink-0 truncate text-[13px] text-faint">{choice.detail}</span>
          )}
        </button>
        <button
          type="button"
          onClick={onOpenSettings}
          className="group flex h-9 items-center gap-2.5 rounded-lg px-3 text-[14px] font-medium text-text transition-colors hover:bg-hover"
        >
          <Icon name="settings" size={16} className="text-muted" />
          <span>Settings</span>
          <span className="flex-1" />
          <Kbd className="opacity-0 transition-opacity group-hover:opacity-100">Ctrl ,</Kbd>
        </button>
      </div>
    </nav>
  );
}

interface RowProps {
  session: Session;
  active: boolean;
  turn: TurnState | undefined;
  onSelect: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onDelete: (id: string) => void;
}

function SessionRow({ session, active, turn, onSelect, onRename, onDelete }: RowProps) {
  const [mode, setMode] = useState<"view" | "rename" | "confirm">("view");
  const [draft, setDraft] = useState(session.title);
  const committed = useRef(false);

  const startRename = () => {
    committed.current = false;
    setDraft(session.title);
    setMode("rename");
  };

  const commit = () => {
    if (committed.current) return;
    committed.current = true;
    setMode("view");
    const title = draft.trim();
    if (title && title !== session.title) onRename(session.id, title);
  };

  if (mode === "rename") {
    return (
      <input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") commit();
          if (e.key === "Escape") {
            committed.current = true;
            setMode("view");
          }
        }}
        onBlur={commit}
        aria-label="Session title"
        className="h-9 rounded-lg bg-surface px-3 text-[14px] text-text shadow-[0_0_0_2px_color-mix(in_srgb,var(--accent)_55%,transparent)] outline-none focus-visible:outline-none"
      />
    );
  }

  if (mode === "confirm") {
    return (
      <div className="fade-in flex h-9 items-center gap-1 rounded-lg bg-danger/10 pl-3 pr-1 text-[13.5px]">
        <span className="min-w-0 flex-1 truncate font-medium text-danger">Delete this session?</span>
        <button
          type="button"
          autoFocus
          onClick={() => onDelete(session.id)}
          className="rounded-md px-2 py-1 font-semibold text-danger hover:bg-danger/15"
        >
          Delete
        </button>
        <button type="button" onClick={() => setMode("view")} className="rounded-md px-2 py-1 text-muted hover:bg-hover">
          Cancel
        </button>
      </div>
    );
  }

  const busy = turn?.busy ?? false;
  const failed = !busy && Boolean(turn?.error);

  return (
    <div className={`group flex h-9 items-center rounded-lg transition-colors ${active ? "bg-active" : "hover:bg-hover"}`}>
      <button
        type="button"
        onClick={() => onSelect(session.id)}
        aria-current={active ? "page" : undefined}
        className={`flex h-full min-w-0 flex-1 items-center gap-2 rounded-lg pl-3 pr-2 text-left text-[14px] ${
          active ? "font-medium text-text" : "text-text-2"
        }`}
      >
        <span className="min-w-0 flex-1 truncate">{session.title}</span>
        {busy && (
          <>
            <Icon
              name="loader"
              size={13}
              className="spin text-running group-focus-within:hidden group-hover:hidden"
            />
            <SrOnly>Working</SrOnly>
          </>
        )}
        {failed && (
          <>
            <Dot color="var(--danger)" className="group-focus-within:hidden group-hover:hidden" />
            <SrOnly>Last turn failed</SrOnly>
          </>
        )}
      </button>
      <div className="hidden shrink-0 items-center pr-1 group-focus-within:flex group-hover:flex">
        <button type="button" aria-label="Rename session" title="Rename" onClick={startRename} className={btn.smallIcon}>
          <Icon name="pencil" size={14} />
        </button>
        <button
          type="button"
          aria-label="Delete session"
          title="Delete"
          onClick={() => setMode("confirm")}
          className={`${btn.smallIcon} hover:text-danger`}
        >
          <Icon name="trash" size={14} />
        </button>
      </div>
    </div>
  );
}
