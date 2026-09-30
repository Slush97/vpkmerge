import { useCallback, useEffect, useState } from "react";
import { api, errorText, type McpServerStatus } from "../../lib/api";
import { Icon } from "../../lib/icons";
import { btn, Dot, Group, Message, Row } from "../ui";

const stateColor: Record<McpServerStatus["state"], string> = {
  connected: "var(--success)",
  connecting: "var(--running)",
  failed: "var(--danger)",
  disabled: "var(--faint)",
};

const stateLabel: Record<McpServerStatus["state"], string> = {
  connected: "Connected",
  connecting: "Connecting",
  failed: "Failed",
  disabled: "Disabled",
};

const example = { command: "uvx", args: ["blender-mcp"], env: { DISABLE_TELEMETRY: "true" } };

export function McpPanel() {
  const [servers, setServers] = useState<McpServerStatus[]>([]);
  const [text, setText] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);

  const poll = useCallback(() => {
    api.mcpStatus().then(setServers).catch(() => {});
  }, []);

  useEffect(() => {
    poll();
    const id = window.setInterval(poll, 2000);
    return () => window.clearInterval(id);
  }, [poll]);

  useEffect(() => {
    api
      .mcpConfig()
      .then((t) => {
        setText(t);
        setLoaded(true);
      })
      .catch((e) => setMessage({ ok: false, text: errorText(e) }));
  }, []);

  const save = async () => {
    setSaving(true);
    setMessage(null);
    try {
      await api.saveMcpConfig(text);
      setMessage({ ok: true, text: "Saved. Servers restarted." });
      poll();
    } catch (e) {
      setMessage({ ok: false, text: errorText(e) });
    } finally {
      setSaving(false);
    }
  };

  const insertExample = () => {
    let next: Record<string, unknown> = { mcpServers: { blender: example } };
    try {
      const parsed: unknown = JSON.parse(text || "{}");
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
        const obj = parsed as Record<string, unknown>;
        const current =
          obj.mcpServers && typeof obj.mcpServers === "object" ? (obj.mcpServers as Record<string, unknown>) : {};
        next = { ...obj, mcpServers: { ...current, blender: current.blender ?? example } };
      }
    } catch {
      setMessage({ ok: false, text: "The current text is not valid JSON, so the example replaced it." });
    }
    setText(JSON.stringify(next, null, 2));
  };

  return (
    <div className="flex flex-col gap-7">
      <Group title="Servers" description="Tools from these servers are offered to provider models and to agents.">
        {servers.length === 0 ? (
          <Row>
            <span className="text-[14px] text-muted">No servers yet. Add one to mcp.json below.</span>
          </Row>
        ) : (
          servers.map((s) => <ServerRow key={s.name} server={s} onReconnect={poll} />)
        )}
      </Group>

      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          void save();
        }}
      >
        <div className="flex items-end gap-2 px-1">
          <label htmlFor="mcp-json" className="flex-1 text-[13px] font-semibold text-muted">
            mcp.json
          </label>
          <button type="button" onClick={insertExample} className={btn.smallGhost}>
            <Icon name="plus" size={13} />
            Blender example
          </button>
        </div>
        <textarea
          id="mcp-json"
          value={text}
          onChange={(e) => setText(e.target.value)}
          spellCheck={false}
          rows={11}
          disabled={!loaded}
          className="selectable min-h-[200px] w-full resize-y rounded-xl bg-code px-4 py-3 font-mono text-[13px] leading-relaxed text-text shadow-card outline-none transition-shadow focus:shadow-[var(--card-shadow),0_0_0_3px_color-mix(in_srgb,var(--accent)_25%,transparent)] focus-visible:outline-none"
        />
        <p className="px-1 text-[13px] leading-snug text-faint">
          Same shape other MCP hosts use. Each server takes <code className="font-mono">command</code> and{" "}
          <code className="font-mono">args</code> (stdio) or <code className="font-mono">url</code> and{" "}
          <code className="font-mono">headers</code> (HTTP), plus optional <code className="font-mono">env</code> and{" "}
          <code className="font-mono">disabled</code>.
        </p>
        <div className="flex items-center gap-3 pt-1">
          <button type="submit" disabled={saving || !loaded} className={btn.primary}>
            {saving ? "Saving" : "Save and restart"}
          </button>
          {message && <Message tone={message.ok ? "ok" : "error"}>{message.text}</Message>}
        </div>
      </form>
    </div>
  );
}

function ServerRow({ server: s, onReconnect }: { server: McpServerStatus; onReconnect: () => void }) {
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const reconnect = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.reconnectMcp(s.name);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
      onReconnect();
    }
  };

  return (
    <div className="border-t border-line first:border-t-0">
      <div className="flex min-h-12 items-center gap-3 px-4 py-2.5">
        <Dot color={stateColor[s.state]} />
        <span className="font-mono text-[14px] font-medium text-text">{s.name}</span>
        <span className="rounded-md bg-fill px-1.5 py-0.5 text-[12px] font-medium text-muted">{s.transport}</span>
        <span className="text-[13px] text-muted">{stateLabel[s.state]}</span>
        <span className="flex-1" />
        {s.state === "connected" && (
          <button
            type="button"
            onClick={() => setOpen((o) => !o)}
            aria-expanded={open}
            className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-[13px] text-muted hover:bg-hover hover:text-text"
          >
            {s.tools.length} {s.tools.length === 1 ? "tool" : "tools"}
            <Icon name={open ? "chevUp" : "chevDown"} size={14} />
          </button>
        )}
        {s.state !== "disabled" && (
          <button
            type="button"
            disabled={busy || s.state === "connecting"}
            onClick={() => void reconnect()}
            className={btn.smallSecondary}
          >
            <Icon name="refresh" size={13} className={busy ? "spin" : ""} />
            Reconnect
          </button>
        )}
      </div>
      {(s.error || error) && (
        <p className="selectable break-words px-4 pb-3 font-mono text-[12.5px] leading-relaxed text-danger">
          {error ?? s.error}
        </p>
      )}
      {open && s.tools.length > 0 && (
        <ul className="fade-in flex flex-col gap-2.5 px-4 pb-3.5 pl-9">
          {s.tools.map((t) => (
            <li key={t.name}>
              <div className="font-mono text-[13px] text-text-2">{t.name}</div>
              {t.description && <div className="text-[13px] leading-snug text-faint">{t.description}</div>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
