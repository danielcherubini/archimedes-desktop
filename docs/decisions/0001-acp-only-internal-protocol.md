---
status: superseded
date: 2026-09-15
superseded-by: 0009-rpc-replaces-acp.md
superseded-date: 2026-09-24
---

# ACP is the only internal agent protocol

Archimedes Desktop is pi-first for v1, but we decided the Rust core speaks **Agent Client Protocol (ACP) exclusively** — pi connects through the `pi-acp` adapter (which wraps `pi --mode rpc`) rather than the core implementing pi's RPC mode directly.

**Why:** one protocol path for all agents now and in later phases; no dual-protocol code in the core; swapping in a native pi ACP mode (or a custom thin adapter) later is a one-line agent-registry change.

**Considered options:**
- *Direct `pi --mode rpc` for pi + ACP for other agents* — higher fidelity for pi, but two protocol implementations in the core and a permanent fork in the session layer.
- *Implement pi's RPC mode natively* — rejected for the same reason.

**Consequences:** v1 fidelity for pi is bounded by the `pi-acp` adapter's approximation of ACP over RPC. If that proves lossy (e.g. subagent or todo semantics), the remedy is a custom pi ACP adapter, not a second protocol path.
