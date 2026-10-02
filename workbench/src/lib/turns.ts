import type { PermissionOption } from "./api";

export interface PermissionRequest {
  requestId: string;
  title: string;
  options: PermissionOption[];
}

export interface TurnState {
  busy: boolean;
  draft: string;
  reasoning: string;
  running: Record<string, string>;
  permissions: PermissionRequest[];
  notices: string[];
  error: string | null;
}

export function newTurn(): TurnState {
  return { busy: false, draft: "", reasoning: "", running: {}, permissions: [], notices: [], error: null };
}
