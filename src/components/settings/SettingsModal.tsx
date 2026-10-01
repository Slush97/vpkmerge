import { useEffect, useRef, useState } from "react";
import type { AgentStatus, ProviderStatus, Settings } from "../../lib/api";
import { Icon, type IconName } from "../../lib/icons";
import { btn, displayClass } from "../ui";
import { AboutPanel } from "./AboutPanel";
import { AccountsPanel } from "./AccountsPanel";
import { AppearancePanel } from "./AppearancePanel";
import { McpPanel } from "./McpPanel";
import { ModelPanel } from "./ModelPanel";
import { SkillsPanel } from "./SkillsPanel";

export type SettingsTab = "accounts" | "appearance" | "model" | "mcp" | "skills" | "about";

const tabs: { id: SettingsTab; label: string; icon: IconName }[] = [
  { id: "accounts", label: "Accounts", icon: "user" },
  { id: "appearance", label: "Appearance", icon: "sun" },
  { id: "model", label: "Model and agents", icon: "cpu" },
  { id: "mcp", label: "MCP servers", icon: "plug" },
  { id: "skills", label: "Skills", icon: "book" },
  { id: "about", label: "About", icon: "info" },
];

interface Props {
  initialTab: SettingsTab;
  settings: Settings | null;
  providers: ProviderStatus[];
  agents: AgentStatus[];
  onClose: () => void;
  onSettingsChange: (s: Settings) => void;
  onRefresh: () => Promise<void>;
}

export function SettingsModal({ initialTab, settings, providers, agents, onClose, onSettingsChange, onRefresh }: Props) {
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const dialog = useRef<HTMLElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !e.defaultPrevented) onClose();
    };
    window.addEventListener("keydown", onKey);
    dialog.current?.focus();
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const current = tabs.find((t) => t.id === tab) ?? tabs[0];

  return (
    <div
      className="fade-in fixed inset-0 z-50 flex items-center justify-center bg-[var(--scrim)] p-8"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <section
        ref={dialog}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-title"
        className="pop-in deco-frame h-[min(680px,100%)] w-[min(960px,100%)] p-px outline-none [--frame-fill:var(--elevated)] [--notch:7px] focus-visible:outline-none"
      >
        <div className="deco-clip flex h-full [--notch:7px]">
          <nav
            aria-label="Settings sections"
            className="grain flex w-[224px] shrink-0 flex-col gap-0.5 border-r-[3px] border-double border-line-strong bg-sidebar px-3 py-5"
          >
            <span className="px-3 pb-4 font-display text-[15px] font-normal uppercase tracking-[0.22em]">Settings</span>
            {tabs.map((t) => {
              const on = t.id === tab;
              return (
                <button
                  key={t.id}
                  type="button"
                  onClick={() => setTab(t.id)}
                  aria-current={on ? "page" : undefined}
                  className={`flex h-9 items-center gap-2.5 rounded-lg px-3 text-left text-[14px] transition-colors ${
                    on
                      ? "bg-active font-medium text-text shadow-[inset_2px_0_0_var(--accent)]"
                      : "text-text-2 hover:bg-hover"
                  }`}
                >
                  <Icon name={t.icon} size={16} className={on ? "text-accent-ink" : "text-muted"} />
                  <span>{t.label}</span>
                </button>
              );
            })}
          </nav>
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex shrink-0 items-center gap-4 pb-2 pl-8 pr-4 pt-5">
              <h2 id="settings-title" className={`flex-1 text-[24px] ${displayClass}`}>
                {current.label}
              </h2>
              <button type="button" aria-label="Close settings" onClick={onClose} className={btn.icon}>
                <Icon name="x" size={17} />
              </button>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto px-8 pb-8 pt-3">
              {tab === "accounts" && (
                <AccountsPanel
                  providers={providers}
                  agents={agents}
                  settings={settings}
                  onSettingsChange={onSettingsChange}
                  onRefresh={onRefresh}
                />
              )}
              {tab === "appearance" && <AppearancePanel settings={settings} onSettingsChange={onSettingsChange} />}
              {tab === "model" && (
                <ModelPanel
                  settings={settings}
                  providers={providers}
                  agents={agents}
                  onSettingsChange={onSettingsChange}
                />
              )}
              {tab === "mcp" && <McpPanel />}
              {tab === "skills" && <SkillsPanel />}
              {tab === "about" && <AboutPanel />}
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
