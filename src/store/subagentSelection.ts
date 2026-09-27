import { create } from "zustand";

interface SubagentSelectionState {
  /** The subagent session id open in the dedicated modal (`null` = closed). */
  selectedSessionId: string | null;
  /** Open (or close, with `null`) the dedicated modal for a subagent. */
  select: (sessionId: string | null) => void;
}

export const useSubagentSelection = create<SubagentSelectionState>((set) => ({
  selectedSessionId: null,
  select: (sessionId) => set({ selectedSessionId: sessionId }),
}));
