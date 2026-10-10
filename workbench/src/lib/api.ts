// Typed wrappers over the Tauri commands in src-tauri/src/lib.rs.
// Field names mirror the Rust structs (serde camelCase).
import { Channel, convertFileSrc, invoke } from "@tauri-apps/api/core";

export type ProviderId = "openai" | "xai" | "openrouter" | "deepseek" | "local";
export type Role = "user" | "assistant" | "tool";

export type Part =
  | { type: "text"; text: string }
  | { type: "reasoning"; text: string }
  | { type: "toolCall"; id: string; name: string; arguments: string }
  | { type: "toolResult"; callId: string; name: string; output: string; isError: boolean };

export interface Session {
  id: string;
  title: string;
  createdAt: number;
  updatedAt: number;
}

export interface StoredMessage {
  id: number;
  sessionId: string;
  role: Role;
  parts: Part[];
  model: string | null;
  createdAt: number;
}

export type AgentEvent =
  | { type: "messageSaved"; message: StoredMessage }
  | { type: "textDelta"; text: string }
  | { type: "reasoningDelta"; text: string }
  | { type: "toolStarted"; callId: string; name: string }
  | { type: "sessionRenamed"; sessionId: string; title: string }
  | { type: "notice"; message: string }
  /** A tool needs the user's approval before it runs. Answer with respondPermission. */
  | { type: "permissionRequested"; requestId: string; title: string; options: PermissionOption[] }
  | { type: "permissionResolved"; requestId: string }
  | { type: "finished" }
  | { type: "cancelled" }
  | { type: "failed"; message: string };

/** External coding agents driven over ACP, each with its own sign-in. */
export type AgentId = "grok" | "codex";

/** What answers the next message: a provider model run by our harness, or an ACP agent. */
export type Choice =
  | { kind: "model"; provider: ProviderId; model: string }
  | { kind: "agent"; agent: AgentId };

export type Theme = "system" | "light" | "dark";

/** What tools may do without asking. One setting for every model and agent. */
export type PermissionLevel = "readOnly" | "ask" | "autoEdit" | "fullAccess";

export interface Settings {
  model: Choice | null;
  localBaseUrl: string;
  skillDirs: string[];
  disabledSkills: string[];
  maxToolRounds: number;
  /** Extra instructions for provider models, appended to the built-in ones. */
  systemPrompt: string;
  openaiHostId: string | null;
  /** Working folder ACP agents run in. */
  agentCwd: string;
  permissionLevel: PermissionLevel;
  theme: Theme;
  /** Accent color as #rrggbb. */
  accent: string;
}

export interface SettingsPatch {
  model?: Choice;
  localBaseUrl?: string;
  maxToolRounds?: number;
  systemPrompt?: string;
  agentCwd?: string;
  permissionLevel?: PermissionLevel;
  theme?: Theme;
  accent?: string;
}

export interface AgentStatus {
  id: AgentId;
  name: string;
  description: string;
  /** The agent's command was found on this machine. */
  installed: boolean;
  command: string;
  /** Filled once the agent process has been started (first use or sign-in). */
  authMethods: { id: string; name: string; description: string | null }[];
  error: string | null;
}

export interface PermissionOption {
  optionId: string;
  name: string;
  kind: "allow_once" | "allow_always" | "reject_once" | "reject_always";
}

export interface ProviderStatus {
  id: ProviderId;
  name: string;
  /** Browser sign-in button label; null when the provider only takes a key. */
  signInLabel: string | null;
  acceptsApiKey: boolean;
  connected: boolean;
  method: "oauth" | "apiKey" | null;
  account: string | null;
}

export interface ModelInfo {
  id: string;
  name: string;
}

export interface Skill {
  name: string;
  description: string;
  dir: string;
  enabled: boolean;
}

export interface McpServerStatus {
  name: string;
  /** Ships with the app; not listed in mcp.json. */
  builtin: boolean;
  transport: "stdio" | "http";
  state: "disabled" | "connecting" | "connected" | "failed";
  error: string | null;
  tools: { name: string; description: string }[];
}

export interface AppInfo {
  version: string;
  dataDir: string;
  secretStorage: "keychain" | "file";
}

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),

  listSessions: () => invoke<Session[]>("list_sessions"),
  createSession: () => invoke<Session>("create_session"),
  renameSession: (id: string, title: string) => invoke<void>("rename_session", { id, title }),
  deleteSession: (id: string) => invoke<void>("delete_session", { id }),
  getMessages: (sessionId: string) => invoke<StoredMessage[]>("get_messages", { sessionId }),

  /** Resolves when the turn is over; progress and the outcome arrive as events. */
  sendMessage: (sessionId: string, text: string, onEvent: (e: AgentEvent) => void) => {
    const channel = new Channel<AgentEvent>();
    channel.onmessage = onEvent;
    return invoke<void>("send_message", { sessionId, text, onEvent: channel });
  },
  cancelTurn: (sessionId: string) => invoke<void>("cancel_turn", { sessionId }),

  getSettings: () => invoke<Settings>("get_settings"),
  updateSettings: (patch: SettingsPatch) => invoke<Settings>("update_settings", { patch }),

  listProviders: () => invoke<ProviderStatus[]>("list_providers"),
  /** Opens the system browser and resolves once the sign-in completes. */
  signIn: (provider: ProviderId) => invoke<void>("sign_in", { provider }),
  cancelSignIn: () => invoke<void>("cancel_sign_in"),
  setApiKey: (provider: ProviderId, key: string) => invoke<void>("set_api_key", { provider, key }),
  signOut: (provider: ProviderId) => invoke<void>("sign_out", { provider }),
  listModels: (provider: ProviderId) => invoke<ModelInfo[]>("list_models", { provider }),

  listAgents: () => invoke<AgentStatus[]>("list_agents"),
  /** Runs the agent's own sign-in (it may open the browser). */
  agentSignIn: (agent: AgentId, methodId: string) => invoke<void>("agent_sign_in", { agent, methodId }),
  /** optionId null means dismiss (cancelled). */
  respondPermission: (requestId: string, optionId: string | null) =>
    invoke<void>("respond_permission", { requestId, optionId }),

  listSkills: () => invoke<Skill[]>("list_skills"),
  skillDirs: () => invoke<string[]>("skill_dirs"),
  setSkillEnabled: (name: string, enabled: boolean) => invoke<void>("set_skill_enabled", { name, enabled }),
  addSkillDir: (path: string) => invoke<void>("add_skill_dir", { path }),
  removeSkillDir: (path: string) => invoke<void>("remove_skill_dir", { path }),

  mcpStatus: () => invoke<McpServerStatus[]>("mcp_status"),
  mcpConfig: () => invoke<string>("mcp_config"),
  saveMcpConfig: (text: string) => invoke<void>("save_mcp_config", { text }),
  reconnectMcp: (name: string) => invoke<void>("reconnect_mcp", { name }),

  openDataDir: () => invoke<void>("open_data_dir"),
  /** http(s) only; opens in the system browser. */
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  /** Where to fetch a .glb a tool result points at: the app's `preview` scheme, not IPC. */
  previewUrl: (path: string) => convertFileSrc(path, "preview"),
  /** Animations a hero preview can play, from the built-in vpkmerge server. */
  previewAnimations: (hero: string, vpk: string | null) =>
    invoke<{ codename: string; animations: string[] }>("preview_animations", { hero, vpk }),
  /** One animation as a skeleton-only .glb, to play on a preview model. */
  previewAnimation: (hero: string, vpk: string | null, animation: string) =>
    invoke<ArrayBuffer>("preview_animation", { hero, vpk, animation }),
};

/** Tauri rejects with the Rust error string. */
export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
