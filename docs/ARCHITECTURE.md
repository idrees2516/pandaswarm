# Architecture

## The two engines

PandaSwarm ships **one product, two engines, one API**:

| | Rust engine | Embedded engine |
|---|---|---|
| where | `crates/*`, `pandaswarm-server`, `pandaswarm-cli` | `src/lib/engine/*` (Next.js app) |
| state | Durable Objects (JSONL logs + snapshots) + SQLite | Prisma/SQLite |
| LLM | OpenAI-compatible / Anthropic / Mock | zai SDK / OpenAI-compatible / Mock |
| sandbox | bwrap → unshare → plain · instabox CLI · E2B-compat | plain (timeout+tempdir) · instabox CLI |
| RAG | in-process BM25 + hashed vectors + RRF | same algorithms, ported |
| memory | SQLite facts with salience decay | same, via Prisma |

Both expose the REST surface documented in the README, so the frontend works
against either. The embedded engine exists so the app is fully functional
without a Rust toolchain; the Rust engine is the production core.

## Durable Objects (`pandaswarm-durable`)

```rust
#[async_trait]
pub trait Behaviour: Send + Sync + 'static {
    type State;    // Serialize + DeserializeOwned + Default
    type Event;    // the only way state changes
    type Command;  // may trigger async I/O

    fn initial_events(state: &Self::State) -> Vec<Self::Event>;
    fn reduce(state: &mut Self::State, event: &Self::Event);          // pure
    async fn handle(&self, state, ctx: &mut Ctx, command) -> Vec<Event>; // I/O
    fn pending_alarms(state: &Self::State, now_ms: u64) -> Vec<Self::Command>;
}
```

* **Host**: `DurableRuntime` keeps `HashMap<class, Arc<dyn AnyObject>>`;
  `ObjectAdapter<B>` is the serde-erasure bridge (commands arrive as JSON,
  events return as JSON — logs, snapshots, API and frontend share the format).
* **Persistence**: `<class>.jsonl` (one event per line) +
  `<class>.snap.json` every 200 events; recovery = snapshot + replay.
* **KV** lives in the envelope (`Ctx::kv_put/kv_get`), snapshotted with state.
* **Alarms** live *inside* state; the 250 ms ticker asks each object for due
  commands and dispatches them through `handle`. An alarm is cancelled by
  folding an event; a crash before that simply re-delivers (at-least-once).
* **Single writer**: one Tokio mutex per object; `handle` may hold it across
  awaits (one command at a time, like a real DO).

## The swarm object (`pandaswarm-orchestrator`)

State (pure data): task, plan, `Vec<AgentRuntime>`, milestone ledger, event
ring (last 400), test reports, next-tick deadline, job list.

Commands: `Start{task}` → `Tick` → … → `TaskFinished`.

A `Tick`:
1. finds the next runnable agent (pending + dependencies done, or running with
   turns left);
2. **merges dependency artifacts** into its workspace (layered workspaces);
3. executes one turn: LLM call with machine-readable `<task-hints>`, tool
   dispatch (`write_file`, `read_file`, `sandbox_run`, `rag_search`,
   `web_search`), all async;
4. folds the outcome as `TurnCompleted{events, done, failed}` — the reducer
   advances statuses, artifacts, reports and milestones deterministically;
5. schedules the next `Tick` (alarm pattern) until everything is terminal,
   then commits the session workspace to git.

Crash at any point: the log replays, the next alarm resumes.

## Sandboxes (`pandaswarm-sandbox`)

```rust
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    fn backend(&self) -> SandboxBackend;
    async fn exec(&self, profile: &SandboxProfile,
                  files: &[(String, String)], cmd: &str) -> Result<ExecResult>;
    async fn cleanup(&self, profile: &SandboxProfile) -> Result<()>;
}
```

* `SandboxProfile`: `fs_ro` binds, `fs_rw` workdir, `net_enabled`,
  `timeout_secs`, env allowlist, backend.
* **local chain** (capability-probed at startup — a real isolated `true`, not
  `--version`): bwrap (ro-root bind, `/workspace` rw, `--unshare-net`) →
  unshare (`--net --pid --fork --mount-proc`) → plain (`timeout` + tempdir).
  Staging happens in the system temp dir so workspaces stay clean.
* **instabox.sh**: one-shot (`instabox run --template ci --push <dir> --
  bash -lc cmd`, box auto-deleted) or persistent (create named
  `pandaswarm-<task>` → push tar bundle → `exec --json` → `rm --yes`).
  Auth via `INSTABOX_API_KEY` (`ibk_…`).
* **E2B-compatible**: configurable create/exec/delete endpoints for
  CubeSandbox-style gateways (explicitly experimental).
* **harness**: `parse_test_report` (pytest/TAP/jest/go/generic counters,
  failure-signature extraction) and `suggest_harness_patches`
  (HarnessPro-style, recurring patterns escalate priority).

## Retrieval (`pandaswarm-rag`)

Incremental (mtime+size) re-index over code roots; line-window chunking with
overlap; every chunk keeps `path:line_start-line_end` provenance; search =
BM25(50) ∪ cosine(50) fused with RRF (k=60); `frame_pack` gzips the result set
into a base64 frame for cheap context shipping. Vectors are deterministic
hashed embeddings (two hash locations per token, ±1 signs, L2-normalised) —
zero model downloads, honest about being "semble-lite".

## Memory (`pandaswarm-memory`)

SQLite: sessions/messages/tool_calls/artifacts (episodic) + facts (semantic):
write-time cosine dedupe at ≥ 0.92 (merge = salience bump), recall-time
exponential decay (`salience·e^(−0.05·days)`), hit counters on both paths.

## Frontend + embedded engine

Next.js 16 App Router, single `/` route with six views (Overview, Swarm
Sessions, Sandbox, RAG, Memory, Settings). Sessions poll
`/api/sessions/:id?after=<seq>` every 2 s while running. The embedded engine
(`src/lib/engine`) mirrors the Rust crates file-for-file in spirit:
`text.ts` (BM25/embeddings/RRF), `llm.ts` (providers + JSON tool protocol),
`planner.ts`, `runner.ts` (coordinator loop with per-agent conversation
memory), `sandbox.ts` (local + instabox + harness), `rag.ts`, `memory.ts`.

Agent↔LLM protocol (provider-agnostic): the system prompt defines a fenced
```json {"tool_calls":[{"name","arguments"}]}``` block; builders that reply
plain text on their first turn get one insistent retry; every turn appends
tool results to the agent's conversation memory.

## Security notes

* Sandbox env is allowlisted (PATH/HOME/LANG + explicit list), network off by
  default, `timeout` enforced inside and outside the jail.
* Agent file writes are path-sanitized (no `..`, no leading `/`).
* Secrets (`INSTABOX_API_KEY`, LLM keys) live in env/server-side settings,
  never in the client bundle; `.env*` is gitignored.
* The `plain` backend is honestly labelled "no kernel isolation" everywhere
  it appears.
