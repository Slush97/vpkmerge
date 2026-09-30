import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  api,
  errorText,
  type AgentEvent,
  type AgentStatus,
  type ProviderStatus,
  type Session,
  type Settings,
  type StoredMessage,
} from "./lib/api";
import { describeChoice } from "./lib/choice";
import { applyTheme, DEFAULT_ACCENT, watchSystemTheme } from "./lib/theme";
import { newTurn, type TurnState } from "./lib/turns";
import { Chat } from "./components/Chat";
import { Sidebar } from "./components/Sidebar";
import { SettingsModal, type SettingsTab } from "./components/settings/SettingsModal";

export default function App() {
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Record<string, StoredMessage[]>>({});
  const [turns, setTurns] = useState<Record<string, TurnState>>({});
  const [settings, setSettings] = useState<Settings | null>(null);
  const [providers, setProviders] = useState<ProviderStatus[]>([]);
  const [agents, setAgents] = useState<AgentStatus[]>([]);
  const [collapsed, setCollapsed] = useState(false);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [composerFocus, setComposerFocus] = useState(0);
  const [searchFocus, setSearchFocus] = useState(0);
  const turnsRef = useRef(turns);
  turnsRef.current = turns;

  const theme = settings?.theme ?? "system";
  const accent = settings?.accent ?? DEFAULT_ACCENT;
  useEffect(() => {
    applyTheme(theme, accent);
    if (theme !== "system") return;
    return watchSystemTheme(() => applyTheme(theme, accent));
  }, [theme, accent]);

  const refreshSessions = useCallback(async () => {
    try {
      setSessions(await api.listSessions());
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  const refreshAccounts = useCallback(async () => {
    try {
      const [s, p, a] = await Promise.all([
        api.getSettings(),
        api.listProviders(),
        api.listAgents().catch(() => [] as AgentStatus[]),
      ]);
      setSettings(s);
      setProviders(p);
      setAgents(a);
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  const loadMessages = useCallback(async (sid: string) => {
    try {
      const list = await api.getMessages(sid);
      setMessages((m) => ({ ...m, [sid]: list }));
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void refreshSessions();
    void refreshAccounts();
  }, [refreshSessions, refreshAccounts]);

  useEffect(() => {
    if (activeId && !turnsRef.current[activeId]?.busy) void loadMessages(activeId);
  }, [activeId, loadMessages]);

  const updateTurn = useCallback((sid: string, f: (t: TurnState) => TurnState) => {
    setTurns((all) => ({ ...all, [sid]: f(all[sid] ?? newTurn()) }));
  }, []);

  const onEvent = useCallback(
    (sid: string, e: AgentEvent) => {
      switch (e.type) {
        case "messageSaved": {
          const message = e.message;
          setMessages((m) => {
            const list = m[sid] ?? [];
            return list.some((x) => x.id === message.id) ? m : { ...m, [sid]: [...list, message] };
          });
          if (message.role === "assistant") {
            updateTurn(sid, (t) => ({ ...t, draft: "", reasoning: "" }));
          } else if (message.role === "tool") {
            const done = message.parts.flatMap((p) => (p.type === "toolResult" ? [p.callId] : []));
            updateTurn(sid, (t) => {
              const running = { ...t.running };
              for (const id of done) delete running[id];
              return { ...t, running };
            });
          }
          break;
        }
        case "textDelta": {
          const text = e.text;
          updateTurn(sid, (t) => ({ ...t, draft: t.draft + text }));
          break;
        }
        case "reasoningDelta": {
          const text = e.text;
          updateTurn(sid, (t) => ({ ...t, reasoning: t.reasoning + text }));
          break;
        }
        case "toolStarted": {
          const { callId, name } = e;
          updateTurn(sid, (t) => ({ ...t, running: { ...t.running, [callId]: name } }));
          break;
        }
        case "permissionRequested": {
          const request = { requestId: e.requestId, title: e.title, options: e.options };
          updateTurn(sid, (t) => ({
            ...t,
            permissions: [...t.permissions.filter((p) => p.requestId !== request.requestId), request],
          }));
          break;
        }
        case "permissionResolved": {
          const id = e.requestId;
          updateTurn(sid, (t) => ({ ...t, permissions: t.permissions.filter((p) => p.requestId !== id) }));
          break;
        }
        case "sessionRenamed": {
          const { sessionId, title } = e;
          setSessions((ss) => ss.map((s) => (s.id === sessionId ? { ...s, title } : s)));
          break;
        }
        case "notice": {
          const message = e.message;
          updateTurn(sid, (t) => ({ ...t, notices: [...t.notices, message] }));
          break;
        }
        case "finished":
        case "cancelled":
        case "failed": {
          const kind = e.type;
          const failure = e.type === "failed" ? e.message : null;
          updateTurn(sid, (t) => ({
            ...t,
            busy: false,
            draft: "",
            reasoning: "",
            running: {},
            permissions: [],
            error: failure,
            notices: kind === "cancelled" ? [...t.notices, "Stopped."] : t.notices,
          }));
          void loadMessages(sid);
          void refreshSessions();
          break;
        }
      }
    },
    [updateTurn, loadMessages, refreshSessions],
  );

  const send = useCallback(
    async (text: string) => {
      let sid = activeId;
      if (!sid) {
        try {
          const session = await api.createSession();
          setSessions((ss) => [session, ...ss]);
          setMessages((m) => ({ ...m, [session.id]: [] }));
          setActiveId(session.id);
          sid = session.id;
        } catch (e) {
          setError(errorText(e));
          return;
        }
      }
      const id = sid;
      setTurns((all) => ({ ...all, [id]: { ...newTurn(), busy: true } }));
      api
        .sendMessage(id, text, (e) => onEvent(id, e))
        .catch((e) => onEvent(id, { type: "failed", message: errorText(e) }));
    },
    [activeId, onEvent],
  );

  const respondPermission = useCallback(
    (requestId: string, optionId: string | null) => {
      const sid = activeId;
      if (sid) updateTurn(sid, (t) => ({ ...t, permissions: t.permissions.filter((p) => p.requestId !== requestId) }));
      api.respondPermission(requestId, optionId).catch((e) => setError(errorText(e)));
    },
    [activeId, updateTurn],
  );

  const stop = useCallback(() => {
    if (activeId) void api.cancelTurn(activeId);
  }, [activeId]);

  const newSession = useCallback(() => {
    setActiveId(null);
    setComposerFocus((n) => n + 1);
  }, []);

  const focusSearch = useCallback(() => {
    setCollapsed(false);
    setSearchFocus((n) => n + 1);
  }, []);

  const rename = useCallback(async (id: string, title: string) => {
    try {
      await api.renameSession(id, title);
      setSessions((ss) => ss.map((s) => (s.id === id ? { ...s, title } : s)));
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  const remove = useCallback(
    async (id: string) => {
      try {
        await api.deleteSession(id);
        setSessions((ss) => ss.filter((s) => s.id !== id));
        setMessages((m) => {
          const next = { ...m };
          delete next[id];
          return next;
        });
        setTurns((t) => {
          const next = { ...t };
          delete next[id];
          return next;
        });
        if (activeId === id) setActiveId(null);
      } catch (e) {
        setError(errorText(e));
      }
    },
    [activeId],
  );

  const openSettings = useCallback((tab: SettingsTab = "accounts") => setSettingsTab(tab), []);

  const closeSettings = useCallback(() => {
    setSettingsTab(null);
    void refreshAccounts();
  }, [refreshAccounts]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const key = e.key.toLowerCase();
      if (key === "b") setCollapsed((c) => !c);
      else if (key === "n") newSession();
      else if (key === "k") focusSearch();
      else if (key === ",") openSettings();
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [newSession, focusSearch, openSettings]);

  const choice = useMemo(() => describeChoice(settings?.model, providers, agents), [settings, providers, agents]);
  const active = sessions.find((s) => s.id === activeId) ?? null;

  return (
    <div className="flex h-full bg-bg text-text">
      <Sidebar
        sessions={sessions}
        activeId={activeId}
        turns={turns}
        collapsed={collapsed}
        searchFocusSignal={searchFocus}
        choice={choice}
        onToggle={() => setCollapsed((c) => !c)}
        onSearch={focusSearch}
        onSelect={setActiveId}
        onNew={newSession}
        onRename={(id, title) => void rename(id, title)}
        onDelete={(id) => void remove(id)}
        onOpenSettings={() => openSettings()}
      />
      <Chat
        session={active}
        messages={activeId ? (messages[activeId] ?? []) : []}
        turn={activeId ? turns[activeId] : undefined}
        settings={settings}
        providers={providers}
        agents={agents}
        focusSignal={composerFocus}
        error={error}
        onDismissError={() => setError(null)}
        onSend={(text) => void send(text)}
        onStop={stop}
        onPermission={respondPermission}
        onSettingsChange={setSettings}
        onOpenSettings={openSettings}
      />
      {settingsTab && (
        <SettingsModal
          initialTab={settingsTab}
          settings={settings}
          providers={providers}
          agents={agents}
          onClose={closeSettings}
          onSettingsChange={setSettings}
          onRefresh={refreshAccounts}
        />
      )}
    </div>
  );
}
