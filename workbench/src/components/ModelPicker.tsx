import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  api,
  errorText,
  type AgentStatus,
  type Choice,
  type ModelInfo,
  type ProviderId,
  type ProviderStatus,
  type Settings,
} from "../lib/api";
import { describeChoice, sameChoice } from "../lib/choice";
import { Icon } from "../lib/icons";
import type { SettingsTab } from "./settings/SettingsModal";
import { btn, inputClass, labelClass, monoInputClass } from "./ui";

type ListState = { status: "loading" } | { status: "ok"; models: ModelInfo[] } | { status: "error"; error: string };

const CAP = 60;

interface Props {
  settings: Settings | null;
  providers: ProviderStatus[];
  agents: AgentStatus[];
  onChange: (settings: Settings) => void;
  onOpenSettings: (tab?: SettingsTab) => void;
}

export function ModelPicker({ settings, providers, agents, onChange, onOpenSettings }: Props) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [lists, setLists] = useState<Partial<Record<ProviderId, ListState>>>({});
  const [manualProvider, setManualProvider] = useState<ProviderId | "">("");
  const [manualId, setManualId] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [place, setPlace] = useState<{ below: boolean; maxHeight: number }>({ below: false, maxHeight: 520 });
  const root = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);

  const connected = useMemo(() => providers.filter((p) => p.connected), [providers]);
  const current = settings?.model ?? null;

  useEffect(() => {
    if (!open) return;
    search.current?.focus();
    for (const p of connected) {
      const state = lists[p.id];
      if (state && state.status !== "error") continue;
      setLists((l) => ({ ...l, [p.id]: { status: "loading" } }));
      api
        .listModels(p.id)
        .then((models) => setLists((l) => ({ ...l, [p.id]: { status: "ok", models } })))
        .catch((e) => setLists((l) => ({ ...l, [p.id]: { status: "error", error: errorText(e) } })));
    }
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  useEffect(() => {
    if (manualProvider || !connected.length) return;
    setManualProvider(current?.kind === "model" ? current.provider : connected[0].id);
  }, [connected, current, manualProvider]);

  const choose = (choice: Choice) => {
    setError(null);
    api
      .updateSettings({ model: choice })
      .then((s) => {
        onChange(s);
        setOpen(false);
        setQuery("");
      })
      .catch((e) => setError(errorText(e)));
  };

  const modelNames = useMemo(() => {
    const names: Record<string, string> = {};
    for (const [provider, state] of Object.entries(lists)) {
      if (state?.status === "ok") for (const m of state.models) names[`${provider}/${m.id}`] = m.name;
    }
    return names;
  }, [lists]);

  const info = describeChoice(current, providers, agents, modelNames);
  const q = query.trim().toLowerCase();
  const shownAgents = agents.filter((a) => !q || a.name.toLowerCase().includes(q));

  return (
    <div
      ref={root}
      className="relative"
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        onClick={() => {
          if (!open && root.current) {
            const r = root.current.getBoundingClientRect();
            const above = r.top - 64;
            const below = window.innerHeight - r.bottom - 16;
            const openBelow = above < 380 && below > above;
            setPlace({ below: openBelow, maxHeight: Math.max(240, Math.min(520, openBelow ? below : above)) });
          }
          setOpen((o) => !o);
        }}
        aria-expanded={open}
        aria-haspopup="dialog"
        className={`inline-flex h-8 max-w-[300px] items-center gap-1.5 rounded-lg px-2.5 text-[13.5px] font-medium transition-colors hover:bg-hover ${
          current ? "text-text-2" : "text-warning"
        }`}
      >
        <Icon name={current?.kind === "agent" ? "bot" : "cpu"} size={15} className={current ? "text-muted" : ""} />
        <span className="truncate">{info.label}</span>
        <Icon name={open ? "chevUp" : "chevDown"} size={14} className="text-faint" />
      </button>

      {open && (
        <div
          role="dialog"
          aria-label="Choose a model or agent"
          style={{ maxHeight: place.maxHeight }}
          className={`pop-in absolute left-0 z-40 flex w-[420px] flex-col overflow-hidden rounded-2xl bg-elevated shadow-pop ${
            place.below ? "top-full mt-2" : "bottom-full mb-2"
          }`}
        >
          <div className="shrink-0 p-2">
            <label className="flex h-9 items-center gap-2 rounded-lg bg-fill px-3 text-faint">
              <Icon name="search" size={15} />
              <input
                ref={search}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search models and agents"
                aria-label="Search models and agents"
                className="min-w-0 flex-1 bg-transparent text-[14px] text-text outline-none placeholder:text-faint focus-visible:outline-none"
              />
            </label>
          </div>

          <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
            {shownAgents.length > 0 && (
              <Section title="Agents">
                {shownAgents.map((a) => {
                  const choice: Choice = { kind: "agent", agent: a.id };
                  return (
                    <Option
                      key={a.id}
                      title={a.name}
                      subtitle={a.installed ? a.description : `Not installed (${a.command})`}
                      icon="bot"
                      selected={sameChoice(current, choice)}
                      disabled={!a.installed}
                      onClick={() => choose(choice)}
                    />
                  );
                })}
              </Section>
            )}

            {connected.map((p) => {
              const state = lists[p.id];
              const models =
                state?.status === "ok"
                  ? state.models.filter((m) => !q || m.id.toLowerCase().includes(q) || m.name.toLowerCase().includes(q))
                  : [];
              const shown = models.slice(0, q ? 200 : CAP);
              if (q && state?.status === "ok" && models.length === 0) return null;
              return (
                <Section
                  key={p.id}
                  title={p.name}
                  busy={state?.status === "loading"}
                >
                  {state?.status === "error" && (
                    <p className="px-3 pb-2 text-[13px] leading-snug text-faint" title={state.error}>
                      {listError(p.id, state.error)}
                    </p>
                  )}
                  {state?.status === "ok" && models.length === 0 && (
                    <p className="px-3 pb-2 text-[13px] text-faint">No models listed.</p>
                  )}
                  {shown.map((m) => {
                    const choice: Choice = { kind: "model", provider: p.id, model: m.id };
                    return (
                      <Option
                        key={m.id}
                        title={m.name}
                        subtitle={m.name !== m.id ? m.id : null}
                        mono
                        selected={sameChoice(current, choice)}
                        onClick={() => choose(choice)}
                      />
                    );
                  })}
                  {models.length > shown.length && (
                    <p className="px-3 py-1.5 text-[13px] text-faint">
                      Showing {shown.length} of {models.length}. Search to narrow.
                    </p>
                  )}
                </Section>
              );
            })}
          </div>

          <form
            className="flex shrink-0 flex-col gap-2 border-t border-line bg-group/60 p-3"
            onSubmit={(e) => {
              e.preventDefault();
              if (manualProvider && manualId.trim())
                choose({ kind: "model", provider: manualProvider, model: manualId.trim() });
            }}
          >
            <span className={labelClass}>Use a model ID directly</span>
            <div className="grid grid-cols-[128px_minmax(0,1fr)_auto] gap-2">
              <div className="relative">
                <select
                  aria-label="Provider"
                  value={manualProvider}
                  onChange={(e) => setManualProvider(e.target.value as ProviderId)}
                  className={`${inputClass} appearance-none truncate pr-7`}
                >
                  {connected.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
                </select>
                <Icon name="chevDown" size={14} className="pointer-events-none absolute right-2.5 top-[11px] text-faint" />
              </div>
              <input
                value={manualId}
                onChange={(e) => setManualId(e.target.value)}
                placeholder="model id"
                aria-label="Model ID"
                spellCheck={false}
                className={monoInputClass}
              />
              <button type="submit" disabled={!manualId.trim() || !manualProvider} className={btn.secondary}>
                Use
              </button>
            </div>
            {error && <p className="text-[13px] text-danger">{error}</p>}
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                onOpenSettings("accounts");
              }}
              className="self-start text-[13px] font-medium text-accent-ink hover:underline hover:underline-offset-2"
            >
              Connect accounts and agents
            </button>
          </form>
        </div>
      )}
    </div>
  );
}

function Section({ title, busy, children }: { title: string; busy?: boolean; children: ReactNode }) {
  return (
    <section className="pt-2">
      <div className={`flex items-center gap-2 px-3 pb-1 pt-1 ${labelClass}`}>
        <span>{title}</span>
        {busy && <Icon name="loader" size={12} className="spin text-running" />}
      </div>
      {children}
    </section>
  );
}

function Option({
  title,
  subtitle,
  icon,
  mono,
  selected,
  disabled,
  onClick,
}: {
  title: string;
  subtitle: string | null;
  icon?: "bot";
  mono?: boolean;
  selected: boolean;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      onClick={onClick}
      aria-pressed={selected}
      className={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors hover:bg-hover disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:bg-transparent ${
        selected ? "bg-active" : ""
      }`}
    >
      {icon && (
        <span className="inline-flex size-8 shrink-0 items-center justify-center rounded-lg bg-fill text-accent-ink">
          <Icon name={icon} size={16} />
        </span>
      )}
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[14px] font-medium text-text">{title}</span>
        {subtitle && (
          <span className={`block truncate text-[13px] text-faint ${mono ? "font-mono" : ""}`}>{subtitle}</span>
        )}
      </span>
      {selected && <Icon name="check" size={16} strokeWidth={2.5} className="text-accent-ink" />}
    </button>
  );
}

function listError(provider: ProviderId, error: string): string {
  if (/connection refused|error sending request|timed out|dns error/i.test(error)) {
    return provider === "local"
      ? "Nothing is answering at the local endpoint. Start your model server or change its address in Settings."
      : "Couldn't reach the provider. Check your connection.";
  }
  if (/401|403|unauthorized|invalid api key/i.test(error)) return "The saved key or sign-in was rejected. Reconnect it in Settings.";
  return error.length > 160 ? `${error.slice(0, 160)}...` : error;
}
