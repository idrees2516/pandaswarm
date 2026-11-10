# Research Notes

How ~35 referenced repositories/papers shaped PandaSwarm. Everything listed as
**verified** was inspected directly (source/docs read); everything else is
marked **design-inspired (unverified)** and used only at the concept level.
No code was copied from any of these projects — PandaSwarm is a from-scratch
implementation.

## Foundation

### pandacode — github.com/aicodeking/pandacode ✅ verified
Tauri 2 desktop coding workspace: Rust core + TypeScript + SQLite, runs Claude
Code / Codex / Kimi / OpenCode / GLM / DeepSeek CLIs side by side, git
worktrees per conversation, MIT **with an attribution clause**.
**Taken:** the product concept this project extends (Rust + Next.js, many-model
agents, workspace-centric), the idea of worktrees per task.
**Added:** swarm orchestration, durable objects, sandboxes, RAG, memory —
everything that makes it "the most agentic orchestration platform".
**License:** attributed here and in README per its clause.

### CoworkOS — arXiv 2611.03178 ✅ verified (paper + repo)
Three-pillar agent coworking; reports Qwen3.6-35B rising 33.19 → 42.95 across
8 benchmarks with planning/execution/reception split.
**Taken:** the tier structure — master (plan + receive), coordinator
(schedule), sub-agents (execute, sync or async), layered workspaces,
semantic memory, resumable monitored jobs. Every one of those is a PandaSwarm
feature.

## Durable Objects

### pi-rs (pi-ai / pi-agent / pi-mcp / pi-durable) ✅ verified
Crate-per-concern decomposition and the `Behaviour { Event, State, Command }`
model with typed registry via serde erasure.
**Taken:** exactly that triad in `pandaswarm-durable`; the crate layout of the
workspace mirrors the philosophy.

### rivet-agents ✅ verified
Persistent actors with alarm wheels.
**Taken:** the 250 ms ticker firing every object's due alarms; single-writer
per object.

### Cloudflare Durable Objects (product reference) ✅ verified (docs)
Semantics of append-only logs + snapshots + KV + alarms, at-least-once alarm
delivery. `pandaswarm-durable` implements these on plain files + Tokio.

### dquickts ✅ verified
Resumable QuickJS heaps — "state you can serialize and continue".
**Taken:** the jobs-as-data model (`pandaswarm-orchestrator/src/jobs.rs`):
running jobs re-enqueue on restart with attempt caps.

## Sandboxing

### sandstorm ✅ verified
gVisor + Firecracker microVMs with hot/frozen/snapshot/detached lifecycles.
**Taken:** lifecycle vocabulary; the notion that a sandbox is a *managed
resource with profiles*, not just a process.

### nullclaw ✅ verified
Sandbox auto-detection chains.
**Taken:** the `bwrap → unshare → plain` fallback chain with real capability
probing (not `--version` probing — PandaSwarm actually runs an isolated
`true` to test each layer).

### CubeSandbox ✅ verified
E2B-compatible API surface, <60 ms starts, eBPF switching.
**Taken:** the "E2B-compatible gateway" plug point
(`pandaswarm-sandbox/src/e2b.rs`), kept configurable because these gateways
differ in the wild.

### nvx ✅ verified
OpenVMM microVM crate.
**Taken (concept):** remote-isolation backend option in the sandbox trait.

### instabox.sh ✅ verified (service + npm package + CLI, live-tested)
Cloud Linux VMs for agents: 4 vCPU / 8 GB free tier, templates
(agent-browser / android / omarchy / ci), `ibk_…` API keys, CLI with
`run --push` one-shots and `exec --json`, fork-in-seconds, computer use.
**Taken:** a first-class sandbox backend (`pandaswarm-sandbox/src/instabox.rs`
and the TS twin) with one-shot + persistent modes. Verified live against the
real API during development (auth, `ls`, `usage`, `templates`, quota errors).

## Retrieval & context

### semble ✅ verified
99 % token-reduction retrieval with incremental re-indexing.
**Taken:** incremental mtime-based re-index (only changed files re-chunked);
the economics argument for local hashed embeddings ("semble-lite") instead of
an embedding API.

### memvid ✅ verified
Smart frames: pack knowledge into retrievable frames.
**Taken:** `frame_pack` / `frame_unpack` (gzip + base64 retrieval frames) in
`pandaswarm-rag`.

### billion-context ✅ verified
Four context tools (compress/decompress/search/status), 95–97 % cache hit.
**Taken (concept):** retrieval results as first-class portable objects rather
than ad-hoc strings.

## Memory

### obelisk ✅ verified
Agent memory on SQLite with FTS5 history.
**Taken:** sessions / messages / tool_calls / artifacts tables + history
search; extended with semantic facts (cosine-deduplicated, salience-decayed).

## Agents & harnesses

### infinity ✅ verified
Agents as pure data — 75 k sleeping agents in a Raspberry Pi's memory.
**Taken:** sub-agents persist as event-log state between turns; nothing
per-turn is kept alive.

### HarnessPro ✅ verified
Recursive harness optimization — failing diagnostics become harness changes.
**Taken:** `TestReport → failure patterns → harness-patch suggestions`
(`pandaswarm-sandbox/src/harness.rs`, `pandaswarm-orchestrator/src/tuner.rs`),
with recurrence escalation.

### cezar ✅ verified (concept)
One task, many coding agents on per-task branches with PRs.
**Taken:** gitops module (branch + worktree + patch export per agent).

### reason-network ✅ verified
P2P agent messaging over iroh with idempotency keys + offline inboxes.
**Taken:** idempotency keys on jobs; at-least-once command delivery in the DO
framework.

## Web & tooling

### lightpanda ✅ verified (docs)
Headless Chromium with a CDP endpoint built for agents.
**Taken:** the CDP client design in
`pandaswarm-search/src/computer.rs` (screenshot → vision-LLM →
click/type/scroll/wait/done).

### firecrawl ✅ verified (API shape, from public docs)
Fetch-any-page-to-markdown.
**Taken:** `fetch_markdown` + `html_to_markdown` (dependency-free).

### MCP (modelcontextprotocol) ✅ verified (spec)
**Taken:** the stdio JSON-RPC client (`initialize → tools/list → tools/call`)
in `pandaswarm-search/src/mcp.rs`.

## Used but not independently verified (design-level only)

hydro-infinity (agent-as-data phrasing), E2B (API conventions),
event-driven-agents (runbook concepts), PydanticAI / OpenAI Agents SDK /
LangGraph (multi-agent patterns), Claude Code / Codex CLIs (agent UX),
mem-agent, agent-sandbox, sandbox-sdk, gVisor/Firecracker (via sandstorm's
usage), Bun/SQLite ecosystem notes, Tavily/Brave/SearXNG (public API shapes),
DuckDuckGo Lite (HTML scraping target), GLM/DeepSeek/OpenRouter (OpenAI-
compatible endpoints). These informed vocabulary and defaults only.

## What is original here

- The `Behaviour`-trait DO framework with field-split envelope dispatch and
  JSONL+snapshot persistence (`pandaswarm-durable`).
- The swarm-as-one-durable-object design: a single coordinator object whose
  `Tick` command executes a full agent turn and folds it back as events.
- The layered-workspace merge semantics (dependency artifacts staged into a
  dependent's workspace before it runs).
- The instabox.sh sandbox provider (one-shot `run --push` + persistent box
  with tar staging outside any git repo).
- The embedded TypeScript engine mirroring the Rust engine's API surface so
  the same UI runs with or without the Rust server.
