import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type AgentStatus, type ProviderStatus, type Settings } from "../../lib/api";
import { describeChoice } from "../../lib/choice";
import { Icon } from "../../lib/icons";
import { btn, Group, groupInputClass, Message, monoGroupInputClass, Row } from "../ui";

interface Props {
  settings: Settings | null;
  providers: ProviderStatus[];
  agents: AgentStatus[];
  onSettingsChange: (s: Settings) => void;
}

export function ModelPanel({ settings, providers, agents, onSettingsChange }: Props) {
  const [rounds, setRounds] = useState(String(settings?.maxToolRounds ?? 24));
  const [cwd, setCwd] = useState(settings?.agentCwd ?? "");
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const choice = settings?.model;
  const info = describeChoice(choice, providers, agents);

  const save = async (patch: Parameters<typeof api.updateSettings>[0], done: string) => {
    setMessage(null);
    try {
      const s = await api.updateSettings(patch);
      onSettingsChange(s);
      setRounds(String(s.maxToolRounds));
      setCwd(s.agentCwd);
      setMessage({ ok: true, text: done });
    } catch (e) {
      setMessage({ ok: false, text: errorText(e) });
    }
  };

  const saveRounds = () => {
    const n = Number.parseInt(rounds, 10);
    if (!Number.isFinite(n) || n < 1) {
      setMessage({ ok: false, text: "Enter a whole number of at least 1." });
      return;
    }
    void save({ maxToolRounds: n }, "Saved.");
  };

  const browse = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Agent working folder", defaultPath: cwd || undefined });
    if (typeof picked === "string") {
      setCwd(picked);
      void save({ agentCwd: picked }, "Working folder saved.");
    }
  };

  return (
    <div className="flex flex-col gap-7">
      <Group title="Answering messages">
        <Row>
          <span className="inline-flex size-9 shrink-0 items-center justify-center rounded-lg bg-fill text-accent-ink">
            <Icon name={choice?.kind === "agent" ? "bot" : "cpu"} size={18} />
          </span>
          <div className="min-w-0 flex-1">
            {choice ? (
              <>
                <div className="truncate text-[14px] font-medium text-text">{info.label}</div>
                <div className="truncate text-[13px] text-muted">
                  {choice.kind === "agent" ? "Agent over ACP" : info.detail}
                  {choice.kind === "model" && <span className="font-mono text-faint"> {choice.model}</span>}
                </div>
              </>
            ) : (
              <div className="text-[14px] text-muted">Nothing chosen yet. Use the menu under the message box.</div>
            )}
          </div>
          {info.problem && choice && <span className="shrink-0 text-[13px] font-medium text-warning">{info.problem}</span>}
        </Row>
      </Group>

      <Group
        title="Agent working folder"
        description="Grok Build and Codex read and edit files here. Pick the project you are working on."
      >
        <Row>
          <form
            className="flex min-w-0 flex-1 items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              if (cwd.trim()) void save({ agentCwd: cwd.trim() }, "Working folder saved.");
            }}
          >
            <input
              aria-label="Agent working folder"
              value={cwd}
              onChange={(e) => setCwd(e.target.value)}
              spellCheck={false}
              className={monoGroupInputClass}
            />
            <button type="button" onClick={() => void browse()} className={btn.secondary}>
              <Icon name="folder" size={14} />
              Browse
            </button>
            <button type="submit" disabled={!cwd.trim() || cwd === settings?.agentCwd} className={btn.secondary}>
              Save
            </button>
          </form>
        </Row>
      </Group>

      <Group
        title="Built-in harness"
        description="Applies to provider models, which Workbench runs itself with your MCP tools and skills."
      >
        <Row>
          <form
            className="flex min-w-0 flex-1 items-center gap-3"
            onSubmit={(e) => {
              e.preventDefault();
              saveRounds();
            }}
          >
            <label htmlFor="max-rounds" className="flex-1 text-[14px] text-text">
              Max tool rounds per message
            </label>
            <input
              id="max-rounds"
              type="number"
              min={1}
              max={200}
              value={rounds}
              onChange={(e) => setRounds(e.target.value)}
              className={`${groupInputClass} max-w-[96px] text-right`}
            />
            <button type="submit" className={btn.secondary}>
              Save
            </button>
          </form>
        </Row>
      </Group>

      {message && <Message tone={message.ok ? "ok" : "error"}>{message.text}</Message>}
    </div>
  );
}
