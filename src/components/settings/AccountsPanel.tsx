import { useState } from "react";
import {
  api,
  errorText,
  type AgentStatus,
  type ProviderId,
  type ProviderStatus,
  type Settings,
} from "../../lib/api";
import { Icon } from "../../lib/icons";
import { btn, Group, groupInputClass, Message, monoGroupInputClass, Pill, Row } from "../ui";

const notes: Record<ProviderId, string> = {
  openai: "Uses your ChatGPT plan through OpenAI's open-source app program, or an API key.",
  xai: "API key from console.x.ai. For your Grok subscription, use the Grok Build agent below.",
  openrouter: "Sign in to mint a key tied to your OpenRouter account, or paste one.",
  deepseek: "API key from platform.deepseek.com.",
  local: "Any OpenAI-compatible server, such as Ollama or LM Studio.",
};

interface Props {
  providers: ProviderStatus[];
  agents: AgentStatus[];
  settings: Settings | null;
  onSettingsChange: (s: Settings) => void;
  onRefresh: () => Promise<void>;
}

export function AccountsPanel({ providers, agents, settings, onSettingsChange, onRefresh }: Props) {
  return (
    <div className="flex flex-col gap-9">
      <div className="flex flex-col gap-6">
        <SectionHeading
          title="Agents"
          text="Coding agents that run on this computer and sign in with their own accounts. Workbench talks to them over ACP."
        />
        {agents.length === 0 ? (
          <p className="text-[14px] text-muted">No agents available.</p>
        ) : (
          agents.map((a) => <AgentGroup key={a.id} agent={a} onRefresh={onRefresh} />)
        )}
      </div>

      <div className="flex flex-col gap-6">
        <SectionHeading
          title="Model providers"
          text="Models Workbench runs itself, with your MCP tools and skills. Keys and tokens stay in the Rust side of the app."
        />
        {providers.map((p) => (
          <ProviderGroup
            key={p.id}
            provider={p}
            settings={settings}
            onSettingsChange={onSettingsChange}
            onRefresh={onRefresh}
          />
        ))}
      </div>
    </div>
  );
}

function SectionHeading({ title, text }: { title: string; text: string }) {
  return (
    <div className="flex flex-col gap-1 px-1">
      <h3 className="text-[16px] font-semibold tracking-[-0.01em]">{title}</h3>
      <p className="text-[13.5px] leading-snug text-muted">{text}</p>
    </div>
  );
}

function useAction(onRefresh: () => Promise<void>) {
  const [pending, setPending] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  const run = async (kind: string, action: () => Promise<unknown>, done?: string) => {
    setPending(kind);
    setError(null);
    setSaved(null);
    try {
      await action();
      await onRefresh();
      if (done) setSaved(done);
    } catch (e) {
      const text = errorText(e);
      if (text !== "cancelled") setError(text);
    } finally {
      setPending(null);
    }
  };

  const feedback = (
    <>
      {error && <Message tone="error">{error}</Message>}
      {saved && !error && (
        <p className="inline-flex items-center gap-1.5 px-1 text-[13px] text-muted">
          <Icon name="check" size={14} className="text-success" />
          {saved}
        </p>
      )}
    </>
  );

  return { pending, run, feedback };
}

function AgentGroup({ agent: a, onRefresh }: { agent: AgentStatus; onRefresh: () => Promise<void> }) {
  const { pending, run, feedback } = useAction(onRefresh);

  return (
    <div className="flex flex-col gap-2">
      <Group
        title={a.name}
        action={
          a.installed ? <Pill color="var(--success)">Installed</Pill> : <Pill color="var(--muted)">Not installed</Pill>
        }
      >
        <Row>
          <span className="inline-flex size-9 shrink-0 items-center justify-center rounded-lg bg-fill text-accent-ink">
            <Icon name="bot" size={18} />
          </span>
          <div className="min-w-0 flex-1">
            <p className="text-[14px] leading-snug text-text">{a.description}</p>
            <p className="mt-0.5 truncate font-mono text-[13px] text-faint" title={a.command}>
              {a.command}
            </p>
          </div>
        </Row>
        {a.installed && (
          <Row className="flex-wrap">
            {a.authMethods.length === 0 ? (
              <span className="text-[13.5px] text-muted">Signs in with its own account on first use.</span>
            ) : (
              <>
                <span className="flex-1 text-[13.5px] text-muted">Sign in</span>
                {a.authMethods.map((m, i) => (
                  <button
                    key={m.id}
                    type="button"
                    title={m.description ?? undefined}
                    disabled={pending !== null}
                    onClick={() => run(m.id, () => api.agentSignIn(a.id, m.id), `Signed in with ${m.name}.`)}
                    className={i === 0 ? btn.smallPrimary : btn.smallSecondary}
                  >
                    {pending === m.id ? <Icon name="loader" size={13} className="spin" /> : null}
                    {m.name}
                  </button>
                ))}
              </>
            )}
          </Row>
        )}
        {a.error && (
          <Row>
            <p className="selectable min-w-0 break-words font-mono text-[13px] leading-relaxed text-danger">{a.error}</p>
          </Row>
        )}
      </Group>
      {feedback}
    </div>
  );
}

function statusPill(p: ProviderStatus) {
  if (p.method === "oauth") return <Pill color="var(--success)">Signed in</Pill>;
  if (p.method === "apiKey") return <Pill color="var(--success)">Key saved</Pill>;
  if (p.id === "local") return <Pill color="var(--muted)">No key needed</Pill>;
  return <Pill color="var(--muted)">Not connected</Pill>;
}

function ProviderGroup({
  provider: p,
  settings,
  onSettingsChange,
  onRefresh,
}: {
  provider: ProviderStatus;
  settings: Settings | null;
  onSettingsChange: (s: Settings) => void;
  onRefresh: () => Promise<void>;
}) {
  const [key, setKey] = useState("");
  const [url, setUrl] = useState(settings?.localBaseUrl ?? "");
  const { pending, run, feedback } = useAction(onRefresh);
  const keyId = `key-${p.id}`;
  const urlId = `url-${p.id}`;

  return (
    <div className="flex flex-col gap-2">
      <Group title={p.name} action={statusPill(p)}>
        <Row>
          <p className="min-w-0 flex-1 text-[13.5px] leading-snug text-muted">
            {notes[p.id]}
            {p.account && <span className="mt-0.5 block font-medium text-text-2">{p.account}</span>}
          </p>
          {p.signInLabel &&
            (pending === "signin" ? (
              <div className="flex shrink-0 items-center gap-2">
                <span className="inline-flex items-center gap-2 text-[13.5px] text-text-2">
                  <Icon name="loader" size={14} className="spin text-running" />
                  Waiting for the browser
                </span>
                <button type="button" onClick={() => void api.cancelSignIn()} className={btn.smallSecondary}>
                  Cancel
                </button>
              </div>
            ) : (
              <button
                type="button"
                disabled={pending !== null}
                onClick={() => run("signin", () => api.signIn(p.id), "Signed in.")}
                className={`${btn.primary} shrink-0`}
              >
                <Icon name="external" size={14} />
                {p.signInLabel}
              </button>
            ))}
        </Row>

        {p.id === "local" && (
          <Row>
            <form
              className="flex min-w-0 flex-1 items-center gap-3"
              onSubmit={(e) => {
                e.preventDefault();
                void run(
                  "url",
                  async () => onSettingsChange(await api.updateSettings({ localBaseUrl: url.trim() })),
                  "Base URL saved.",
                );
              }}
            >
              <label htmlFor={urlId} className="w-[92px] shrink-0 text-[14px] text-text">
                Base URL
              </label>
              <input
                id={urlId}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                placeholder="http://127.0.0.1:11434/v1"
                spellCheck={false}
                className={monoGroupInputClass}
              />
              <button type="submit" disabled={pending !== null || !url.trim()} className={btn.secondary}>
                Save
              </button>
            </form>
          </Row>
        )}

        {p.acceptsApiKey && (
          <Row>
            <form
              className="flex min-w-0 flex-1 items-center gap-3"
              onSubmit={(e) => {
                e.preventDefault();
                void run(
                  "key",
                  async () => {
                    await api.setApiKey(p.id, key);
                    setKey("");
                  },
                  "Key saved.",
                );
              }}
            >
              <label htmlFor={keyId} className="w-[92px] shrink-0 text-[14px] text-text">
                API key
              </label>
              <input
                id={keyId}
                type="password"
                value={key}
                onChange={(e) => setKey(e.target.value)}
                placeholder={
                  p.method === "apiKey" ? "Saved. Paste a new key to replace it." : p.id === "local" ? "Optional" : "Paste a key"
                }
                autoComplete="off"
                spellCheck={false}
                className={groupInputClass}
              />
              <button type="submit" disabled={pending !== null || !key.trim()} className={btn.secondary}>
                Save
              </button>
            </form>
          </Row>
        )}

        {p.method && (
          <Row>
            <span className="flex-1 text-[13.5px] text-muted">
              {p.method === "oauth" ? "Signed in through the browser" : "Using a saved API key"}
            </span>
            <button
              type="button"
              disabled={pending !== null}
              onClick={() => run("signout", () => api.signOut(p.id), "Signed out.")}
              className={btn.danger}
            >
              Sign out
            </button>
          </Row>
        )}
      </Group>
      {feedback}
    </div>
  );
}
