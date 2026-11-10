<div align="center">

# 🐼 PandaSwarm

**The most agentic coding-agent orchestration platform.**

Spawn agent swarms for coding tasks — planned by a master agent, coordinated by
self-hosted Rust Durable Objects, executed by sub-agents whose code is validated
in layered sandboxes before it counts.

`Rust` · `Next.js 16` · `SQLite` · `instabox.sh` · `hybrid RAG` · `Durable Objects`

</div>

---

## What it does

Give PandaSwarm a task:

```
implement fibonacci and palindrome utilities with tests
```

and it:

1. **Plans** — the master agent (tier 1) decomposes the task into milestones (LLM-driven, with a deterministic heuristic fallback so it never blocks);
2. **Spawns a swarm** — the coordinator (tier 2) instantiates coder / tester / reviewer / integrator sub-agents (tier 3) with a dependency graph and layered workspaces;
3. **Writes real code** — coders call `write_file`, producing artifacts you can open in the UI;
4. **Validates in sandboxes** — every run executes under a `SandboxProfile` (local `bwrap → unshare → plain` chain, or **instabox.sh** cloud Linux VMs), producing a structured `TestReport`;
5. **Learns from failures** — the HarnessPro-style tuner converts recurring failure patterns into harness-patch suggestions;
6. **Remembers** — semantic facts with salience decay + hybrid RAG (BM25 + hashed vectors + RRF fusion) feed every master-agent turn.

Everything is an **event**: kill the process anywhere and the durable-object log
replays, the alarm wheel re-arms, and the swarm resumes exactly where it stopped.

## Architecture (three-tier cowork)

```text
┌─────────────────────────────────────────────────────────────────────┐
│ Tier 1 · Master agent          talks to the human, owns the plan,    │
│ (chat + task intake)           receives results (memory + RAG ctx)  │
└───────────────┬─────────────────────────────────────────────────────┘
                │ Start{task}
┌───────────────▼─────────────────────────────────────────────────────┐
│ Tier 2 · Coordinator = SwarmObject (a Durable Object)               │
│   append-only JSONL event log · snapshots · KV · 250ms alarm ticks   │
│   dependency scheduling · layered-workspace merges · job recovery   │
└───────────────┬─────────────────────────────────────────────────────┘
                │ Tick → one agent turn
┌───────────────▼─────────────────────────────────────────────────────┐
│ Tier 3 · Sub-agents (pure data between turns)                       │
│   coder → write_file · sandbox_run        reviewer → read · sign-off│
│   tester → pytest in sandbox → TestReport  integrator → merge + git │
└─────────────────────────────────────────────────────────────────────┘
```

## Repository layout

```
crates/
  pandaswarm-core          ids, SwarmEvent, Plan/AgentSpec, config, BM25 + hashed embeddings
  pandaswarm-durable       ⭐ self-hosted Durable Objects (Event/State/Command, logs, snapshots, KV, alarms)
  pandaswarm-llm           OpenAI-compatible + Anthropic + deterministic Mock providers
  pandaswarm-memory        obelisk-style SQLite memory: semantic facts, salience decay, FTS history
  pandaswarm-rag           hybrid retrieval (BM25 + vectors + RRF), incremental re-index, gzip frame-packs
  pandaswarm-search        ripgrep tool, web search (Tavily/Brave/SearXNG/DDG), fetch→markdown,
                           CDP computer-use client, MCP stdio client
  pandaswarm-sandbox       ⭐ bwrap → unshare → plain chain + instabox.sh + E2B-compatible gateways,
                           validation harness (TestReport → harness-patch suggestions)
  pandaswarm-orchestrator  ⭐ three-tier cowork, swarm DO, gitops (worktrees/branches), resumable jobs, tuner
  pandaswarm-server        axum REST + WebSocket telemetry
  pandaswarm-cli           pandaswarm serve | task | chat | demo | rag | memory | sandbox | doctor
src/                       Next.js 16 frontend + embedded TypeScript engine (same API surface)
prisma/                    SQLite schema for the embedded engine
docs/                      RESEARCH.md (35-repo study) · ARCHITECTURE.md
```

## Quickstart

### The Next.js app (embedded engine — zero Rust required)

```bash
cp .env.example .env     # SQLite path (+ optional keys)
bun install
bun run db:push          # SQLite schema
bun run dev              # http://localhost:3000
```

The app ships an **embedded TypeScript engine** implementing the full loop
(planner → swarm → sandbox → harness) backed by Prisma/SQLite. The default LLM
provider is the built-in `zai` SDK; switch to `mock` (offline, deterministic)
or any OpenAI-compatible endpoint in **Settings**.

### The Rust engine

```bash
cargo build --release
export PANDASWARM_LLM_PROVIDER=mock          # or openai/anthropic/glm/deepseek/ollama
export PANDASWARM_LLM_API_KEY=sk-...
./target/release/pandaswarm demo             # offline end-to-end demo
./target/release/pandaswarm task "implement fibonacci utilities with tests"
./target/release/pandaswarm serve            # REST + WS on 127.0.0.1:8081
```

### Sandboxes

| backend | what you get | how to enable |
|---|---|---|
| `bwrap` | read-only root bind, writable `/workspace`, no network | install `bubblewrap` |
| `unshare` | `--net --pid --fork --mount-proc` | Linux with userns/root |
| `plain` | `timeout` + tempdir (dev machines) | always available |
| `instabox` | full Linux cloud VM (4 vCPU / 8 GB), templates like `ci` | `INSTABOX_API_KEY=ibk_…` ([instabox.sh](https://instabox.sh), CLI: `npx instabox@latest`) |
| `e2b_compat` | any E2B-shaped REST gateway (CubeSandbox, sandstorm, …) | `sandbox.e2b.base_url` + key |

`auto` (default) picks the strongest available local layer at startup.

### instabox.sh one-shots

```bash
export INSTABOX_API_KEY=ibk_...   # from instabox.sh
pandaswarm sandbox run --backend instabox --cmd "python3 main.py" --file main.py=@main.py
```

One-shot mode (`instabox run --template ci --push <dir>`) creates a box, uploads
the staged files, runs the command and deletes the box — ideal for validation.
Persistent mode keeps a `pandaswarm-<task>` box across runs.

## API (both engines expose the same surface)

```
GET  /api/health                     GET  /api/sessions
POST /api/sessions      {prompt}     GET  /api/sessions/:id ?after=<seq>
POST /api/sessions/:id/chat          POST /api/sessions/:id/cancel
POST /api/sandbox/run   {files, cmd, backend, timeout_secs}
GET  /api/sandbox/backends           GET  /api/rag/search?q=…
POST /api/rag/index                  GET  /api/memory/facts  · POST (remember) · GET ?q= (recall)
GET  /api/computer-use/status        WS   /ws  (live telemetry)
```

## The research behind it

PandaSwarm distills ideas from ~35 repositories and papers — pandacode (the
workspace concept this extends), CoworkOS (three-tier cowork), pi-durable /
rivet-agents (DO semantics), sandstorm / nullclaw / CubeSandbox (layered
sandboxing), semble (hybrid retrieval economics), obelisk (memory), HarnessPro
(harness self-improvement), lightpanda (CDP computer use), reason-network
(idempotent jobs), memvid (frame packs), billion-context (context tools) and
more. The full study — what was verified, what was adapted, what is original —
lives in [`docs/RESEARCH.md`](docs/RESEARCH.md), and the system design in
[`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

## Status

- ✅ 10 Rust crates compile clean, 21 unit tests green, offline demo passes end-to-end
- ✅ Embedded TS engine runs the same loop in the Next.js app (browser-verified)
- ✅ instabox.sh integration live (auth + exec); free-plan slot limits apply
- 🔭 next: worktree-per-agent PR flow in the UI, streaming tokens, multi-repo workspaces

## License & attribution

MIT — see [`LICENSE`](LICENSE).

**PandaSwarm extends the ideas of [pandacode](https://github.com/aicodeking/pandacode)**
(aicodeking, MIT): a Rust+Next.js agent workspace. This project is an
independent implementation that keeps pandacode's spirit (local-first,
Rust core, many-model) and adds swarm orchestration, durable objects,
sandboxes and retrieval on top. See [`docs/RESEARCH.md`](docs/RESEARCH.md) for
the complete credit list.
