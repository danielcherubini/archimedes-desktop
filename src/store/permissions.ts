import { create } from "zustand";
import type { PermissionOption, PermissionRequest } from "../lib/tauri";

export interface PermissionPromptData {
  /** JSON-RPC id of the agent's request (carried in the event payload). */
  requestId: string;
  toolTitle: string;
  options: PermissionOption[];
}

interface PermissionsState {
  /** Pending prompts, keyed by session id. */
  prompts: Record<string, PermissionPromptData[]>;
  addPrompt: (
    sessionId: string,
    requestId: string,
    request: PermissionRequest | unknown,
  ) => void;
  removePrompt: (sessionId: string, requestId: string) => void;
  /** Dismiss every prompt for a session (session closed). */
  dismissSessionPrompts: (sessionId: string) => void;
}

export const usePermissions = create<PermissionsState>((set) => ({
  prompts: {},

  addPrompt: (sessionId, requestId, request) =>
    set((state) => {
      const req = request as PermissionRequest | undefined;
      const data: PermissionPromptData = {
        requestId,
        toolTitle: req?.toolCall?.title ?? "Permission requested",
        options: req?.options ?? [],
      };
      const existing = (state.prompts[sessionId] ?? []).filter(
        (p) => p.requestId !== requestId,
      );
      return {
        prompts: { ...state.prompts, [sessionId]: [...existing, data] },
      };
    }),

  removePrompt: (sessionId, requestId) =>
    set((state) => {
      const list = (state.prompts[sessionId] ?? []).filter(
        (p) => p.requestId !== requestId,
      );
      return { prompts: { ...state.prompts, [sessionId]: list } };
    }),

  dismissSessionPrompts: (sessionId) =>
    set((state) => {
      if (!state.prompts[sessionId]) return state;
      const prompts = { ...state.prompts };
      delete prompts[sessionId];
      return { prompts };
    }),
}));
