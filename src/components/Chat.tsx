import { useState } from "react";
import type { AgentStatus, ProviderStatus, Session, Settings, StoredMessage } from "../lib/api";
import { Icon } from "../lib/icons";
import type { TurnState } from "../lib/turns";
import type { SettingsTab } from "./settings/SettingsModal";
import { Composer } from "./Composer";
import { MessageList } from "./MessageList";
import { Pill } from "./ui";
import { WindowControls } from "./WindowControls";

const starters = [
  { icon: "sparkles" as const, text: "Merge two addon VPKs and explain any conflicts" },
  { icon: "terminal" as const, text: "Look at my open Blender scene and summarize it" },
  { icon: "wrench" as const, text: "Help me swap a hero sound with my own MP3" },
];

interface Props {
  session: Session | null;
  messages: StoredMessage[];
  turn: TurnState | undefined;
  settings: Settings | null;
  providers: ProviderStatus[];
  agents: AgentStatus[];
  focusSignal: number;
  error: string | null;
  onDismissError: () => void;
  onSend: (text: string) => void;
  onStop: () => void;
  onPermission: (requestId: string, optionId: string | null) => void;
  onSettingsChange: (s: Settings) => void;
  onOpenSettings: (tab?: SettingsTab) => void;
}

export function Chat(props: Props) {
  const { session, messages, turn, error, onDismissError } = props;
  const [prefill, setPrefill] = useState({ text: "", n: 0 });
  const busy = turn?.busy ?? false;
  const empty = !session || (messages.length === 0 && !busy && !turn?.error);

  const composer = (rows: number) => (
    <Composer
      busy={busy}
      rows={rows}
      settings={props.settings}
      providers={props.providers}
      agents={props.agents}
      focusSignal={props.focusSignal}
      prefill={prefill}
      onSend={props.onSend}
      onStop={props.onStop}
      onSettingsChange={props.onSettingsChange}
      onOpenSettings={props.onOpenSettings}
    />
  );

  return (
    <main className="flex min-w-0 flex-1 flex-col bg-bg">
      <header data-tauri-drag-region className="flex h-[52px] shrink-0 items-center gap-2.5 border-b border-line pl-6 pr-3">
        <h1 data-tauri-drag-region className="min-w-0 truncate text-[15px] font-semibold tracking-[-0.01em]">
          {session?.title ?? "New session"}
        </h1>
        {busy && <Pill color="var(--running)">Working</Pill>}
        <div data-tauri-drag-region className="flex-1 self-stretch" />
        <WindowControls />
      </header>

      {error && (
        <div role="alert" className="fade-in flex shrink-0 items-center gap-2.5 bg-danger/[0.07] px-6 py-2.5">
          <Icon name="alertCircle" size={16} className="text-danger" />
          <span className="selectable min-w-0 flex-1 truncate text-[13.5px] text-danger">{error}</span>
          <button type="button" aria-label="Dismiss" onClick={onDismissError} className="inline-flex size-7 items-center justify-center rounded-md text-muted hover:bg-hover hover:text-text">
            <Icon name="x" size={14} />
          </button>
        </div>
      )}

      {empty ? (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center overflow-y-auto px-8 pb-16">
          <div className="flex w-full max-w-[680px] flex-col gap-8">
            <div className="flex flex-col gap-2.5 text-center">
              <h2 className="text-[32px] font-[650] leading-tight tracking-[-0.022em]">What are we making?</h2>
              <p className="text-[15px] leading-normal text-muted">
                Describe the change you want. Your MCP tools and skills come along.
              </p>
            </div>
            {composer(3)}
            <div className="flex flex-wrap justify-center gap-2">
              {starters.map((s) => (
                <button
                  key={s.text}
                  type="button"
                  onClick={() => setPrefill((p) => ({ text: s.text, n: p.n + 1 }))}
                  className="inline-flex h-9 items-center gap-2 rounded-full bg-fill px-3.5 text-[13.5px] text-text-2 transition-colors hover:bg-active hover:text-text"
                >
                  <Icon name={s.icon} size={14} className="text-accent-ink" />
                  {s.text}
                </button>
              ))}
            </div>
          </div>
        </div>
      ) : (
        <>
          <MessageList key={session.id} messages={messages} turn={turn} onPermission={props.onPermission} />
          <div className="flex shrink-0 justify-center px-8 pb-4">
            <div className="w-full max-w-[720px]">{composer(1)}</div>
          </div>
        </>
      )}
    </main>
  );
}
