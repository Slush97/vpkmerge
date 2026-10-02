import type { AgentStatus, Choice, ProviderStatus } from "./api";

export interface ChoiceInfo {
  label: string;
  detail: string | null;
  ready: boolean;
  problem: string | null;
}

export function describeChoice(
  choice: Choice | null | undefined,
  providers: ProviderStatus[],
  agents: AgentStatus[],
  modelNames: Record<string, string> = {},
): ChoiceInfo {
  if (!choice) return { label: "Choose a model", detail: null, ready: false, problem: "No model chosen" };
  if (choice.kind === "agent") {
    const agent = agents.find((a) => a.id === choice.agent);
    const name = agent?.name ?? choice.agent;
    if (agent && !agent.installed) return { label: name, detail: "Agent", ready: false, problem: `${name} is not installed` };
    return { label: name, detail: "Agent", ready: true, problem: null };
  }
  const provider = providers.find((p) => p.id === choice.provider);
  const label = modelNames[`${choice.provider}/${choice.model}`] ?? choice.model;
  const detail = provider?.name ?? choice.provider;
  if (provider && !provider.connected) return { label, detail, ready: false, problem: `${detail} is not connected` };
  return { label, detail, ready: true, problem: null };
}

export function sameChoice(a: Choice | null | undefined, b: Choice): boolean {
  if (!a || a.kind !== b.kind) return false;
  if (a.kind === "agent" && b.kind === "agent") return a.agent === b.agent;
  if (a.kind === "model" && b.kind === "model") return a.provider === b.provider && a.model === b.model;
  return false;
}
