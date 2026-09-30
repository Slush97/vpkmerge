import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { AgentStatus, ProviderStatus, Settings } from "../lib/api";
import { Icon } from "../lib/icons";
import type { SettingsTab } from "./settings/SettingsModal";
import { ModelPicker } from "./ModelPicker";

interface Props {
  busy: boolean;
  settings: Settings | null;
  providers: ProviderStatus[];
  agents: AgentStatus[];
  focusSignal: number;
  prefill: { text: string; n: number };
  rows?: number;
  onSend: (text: string) => void;
  onStop: () => void;
  onSettingsChange: (s: Settings) => void;
  onOpenSettings: (tab?: SettingsTab) => void;
}

export function Composer({
  busy,
  settings,
  providers,
  agents,
  focusSignal,
  prefill,
  rows = 1,
  onSend,
  onStop,
  onSettingsChange,
  onOpenSettings,
}: Props) {
  const [text, setText] = useState("");
  const area = useRef<HTMLTextAreaElement>(null);
  const hasModel = Boolean(settings?.model);
  const canSend = text.trim().length > 0 && !busy && hasModel;

  useEffect(() => {
    area.current?.focus();
  }, [focusSignal]);

  useEffect(() => {
    if (!prefill.n) return;
    setText(prefill.text);
    area.current?.focus();
  }, [prefill]);

  useLayoutEffect(() => {
    const el = area.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 260)}px`;
  }, [text]);

  const submit = () => {
    if (!canSend) return;
    onSend(text.trim());
    setText("");
  };

  return (
    <div className="w-full rounded-2xl bg-surface px-4 pb-3 pt-3.5 shadow-pop transition-shadow focus-within:shadow-[var(--pop-shadow),0_0_0_3px_color-mix(in_srgb,var(--accent)_22%,transparent)]">
      <textarea
        ref={area}
        rows={rows}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
            e.preventDefault();
            submit();
          }
        }}
        placeholder={hasModel ? "Message Workbench" : "Choose a model below to start"}
        aria-label="Message"
        className="block min-h-6 w-full resize-none bg-transparent px-0.5 text-[15px] leading-[1.55] text-text outline-none placeholder:text-faint focus-visible:outline-none"
      />
      <div className="mt-3 flex items-center gap-2">
        <ModelPicker
          settings={settings}
          providers={providers}
          agents={agents}
          onChange={onSettingsChange}
          onOpenSettings={onOpenSettings}
        />
        <span className="flex-1" />
        {!busy && hasModel && text.trim() && (
          <span className="hidden whitespace-nowrap text-[12.5px] text-faint sm:inline">Enter to send</span>
        )}
        {busy ? (
          <button
            type="button"
            aria-label="Stop"
            title="Stop"
            onClick={onStop}
            className="inline-flex size-8 items-center justify-center rounded-full bg-text text-bg transition-opacity hover:opacity-85"
          >
            <Icon name="stop" size={14} strokeWidth={2.5} />
          </button>
        ) : (
          <button
            type="button"
            aria-label="Send"
            title="Send"
            disabled={!canSend}
            onClick={submit}
            className="inline-flex size-8 items-center justify-center rounded-full bg-accent-fill text-on-accent transition-[background-color,opacity] hover:bg-[color-mix(in_srgb,var(--accent-fill)_88%,black)] disabled:cursor-not-allowed disabled:bg-fill disabled:text-faint"
          >
            <Icon name="send" size={16} strokeWidth={2.4} />
          </button>
        )}
      </div>
    </div>
  );
}
