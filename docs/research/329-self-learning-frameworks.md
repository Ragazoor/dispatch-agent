# Self-learning frameworks for dispatch (task #329, epic #27)

Research note. Deliverable for task #329; input for refining epic #27 ("Improve over time"). Surveys frameworks dispatch could adopt or take inspiration from to help agents learn a user's setup over time, then proposes a refined epic goal and a list of follow-up work packages.

## 1. Problem framing

Today, every dispatched agent in this repo starts cold:

- It reads `CLAUDE.md`, the workspace `CLAUDE.md`, and the user's global rules.
- It reads any plan file attached to the task and the Allium specs.
- It has no access to "what previous agents learned the hard way" on the same repo, the same epic, or even an earlier slice of the same task.

Dispatch already has several surfaces that could carry learned context (skills, hooks, tips, plan files, Allium specs, the user-level auto-memory at `~/.claude/projects/<project>/memory/`), but nothing wires them together as an explicit *learning loop*. Agents do not write back into these surfaces in any structured way; humans do.

Epic #27 ("I want dispatch to help agents to improve over time and learn a user setup") is the umbrella for closing that gap. This task is the up-front research that picks a direction.

## 2. Frameworks surveyed

For each entry: what it is, how it accumulates context over time, and an honest "fit for dispatch" verdict.

### 2.1 mozilla-ai / cq

**What.** An open standard and reference implementation for *shared agent learning*. Agents query a "commons" before tackling a task; if they discover a novel pitfall/workaround, they propose it back. Designed explicitly for coding agents (first-class plugins for Claude Code and OpenCode). Active development started early March 2026; still early-stage. ([repo](https://github.com/mozilla-ai/cq), [architecture doc](https://github.com/mozilla-ai/cq/blob/main/docs/architecture.md))

**How it works.** Three runtime tiers:
- Agent process loads a `SKILL.md`, hooks, and slash commands.
- A local MCP server (FastMCP, Python) holds a private SQLite store at `~/.local/share/cq/local.db`. Exposes six MCP tools: `query`, `propose`, `confirm`, `flag`, `reflect`, `status`.
- A Docker-hosted FastAPI "remote API" holds organisation-shared knowledge in Postgres + pgvector.

Knowledge is captured as structured "knowledge units" with a JSON schema: insight (summary + actionable guidance), context (language/framework/env/patterns), evidence (confidence, confirmation count, organisational diversity), provenance (contributor DID + audit trail), lifecycle (kind = pitfall / workaround / tool-recommendation / tool-gap-signal, staleness policy, relationships).

The flow on every task: agent calls `query` with the current context → local store searched → remote API queried if local miss → results returned → agent acts. On success agent calls `propose` to register the new learning; guardrails filter, the unit lands locally; a human-review gate promotes it to the remote tier.

**Fit for dispatch.** Surprisingly strong conceptual fit. cq is built around exactly the assumptions dispatch makes: Claude Code plugin model, MCP for tool plumbing, structured per-task knowledge. The shape of a "knowledge unit" is what dispatch would otherwise have to invent. Caveats: very young (months old, prototype-quality), Python service + Docker deployment are heavier than dispatch wants to bundle, and the remote-tier organisational features overshoot a single-developer TUI use case. Recommended primarily as a reference architecture and possibly as an *opt-in MCP server peer* alongside dispatch — not as a hard dependency.

### 2.2 Anthropic Claude Agent SDK — Memory tool

**What.** A first-party Claude tool exposing a `/memories` directory the model can read/write across sessions. Backed by an abstract class (`BetaAbstractMemoryTool` in Python, `betaMemoryTool` in TypeScript) so the host app picks the actual store (file, DB, encrypted blob, cloud). Used in Claude Managed Agents and the Agent SDK long-running-workflow story. ([API docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool))

**How it works.** Files in `/memories` survive across context compaction and across sessions. The recommended pattern for long projects is to *bootstrap* memory deliberately at the start of a project (write a progress log, a feature checklist, a startup script reference) rather than letting the agent ad-hoc accumulate scratch notes. Anthropic's "long-running harnesses" guidance pairs the memory tool with compaction: compaction keeps the window manageable; memory is what survives compaction.

**Fit for dispatch.** Already adjacent — dispatch's TUI launches Claude Code, and Claude Code can be given memory tool access. The cleanest "first version" of agent learning is probably: dispatch provisions a `/memories/` directory under each *worktree* (or, better, under each *repo* inside a dispatch-managed location), and the agent uses the standard memory tool against it. No new MCP plumbing required. The hard problem becomes *what schema* to use inside `/memories` — which is exactly what cq's knowledge-unit shape solves.

### 2.3 Cline "Memory Bank"

**What.** A documentation methodology, not a feature. A set of prescribed markdown files in a `memory-bank/` directory that the agent is instructed (via custom-instructions) to read at the start of *every* task and update at the end. Files: `projectbrief.md`, `productContext.md`, `activeContext.md`, `systemPatterns.md`, `techContext.md`, `progress.md`. Pure prompt engineering on top of the agent's existing file tools. ([docs](https://docs.cline.bot/features/memory-bank), [methodology blog](https://cline.bot/blog/memory-bank-how-to-make-cline-an-ai-agent-that-never-forgets))

**How it works.** No code, no MCP server, no vector DB. The "system" is: the convention that those files exist, plus a custom-instructions block that drilled the agent on (a) read all of them at task start, (b) write back to `activeContext.md` and `progress.md` at task end, (c) propose updates to `systemPatterns.md` when discovering recurring patterns.

**Fit for dispatch.** Very strong fit *for v1*. Dispatch already lives in markdown — `CLAUDE.md`, plan files, Allium specs — and already has hooks to inject prompt prefixes when dispatching. The Memory Bank pattern can be implemented with zero new infrastructure: pick a path under each repo (e.g. `.dispatch/memory/`), define a six-file schema, and inject "read/update these files" into the dispatch prompt. The downside is the same as the strength: it's prompt discipline rather than enforced behaviour, so quality depends on prompt quality.

### 2.4 Cursor rules / `.cursor/rules/*.mdc` / AGENTS.md

**What.** Cursor's evolving convention for project-specific agent context. The legacy `.cursorrules` is deprecated; the modern shape is `.cursor/rules/*.mdc` — markdown files with YAML frontmatter declaring `description`, `globs` (which file patterns the rule applies to), and `alwaysApply`. The cross-tool successor is `AGENTS.md` in the project root, supported by Claude Code, Cursor, Copilot, Aider, Gemini CLI, Windsurf, Zed and others. ([Cursor docs](https://cursor.com/docs/rules))

**How it works.** Rules are *static* context the editor injects. The agent does not write to them; humans curate. Glob-scoped rules limit token cost (only inject the testing rule when editing tests).

**Fit for dispatch.** Useful as a *baseline*: dispatch should make sure dispatched agents see `AGENTS.md` / `.cursor/rules/*.mdc` if present, since the user may already have them. But Cursor rules answer a different question — "what rules should always apply" — not "what did agents learn last week". They are an output surface a learning system might write *into*, not a learning system themselves.

### 2.5 Aider conventions + repo map

**What.** Aider auto-builds a "repo map" (concise listing of important classes/functions and their signatures) and ships it with each request. Optionally loads a `CONVENTIONS.md` (or any list of files configured in `.aider.conf.yml`) with natural-language conventions. ([repo map](https://aider.chat/docs/repomap.html), [conventions](https://aider.chat/docs/usage/conventions.html))

**How it works.** Map is generated, conventions are static. No write-back loop.

**Fit for dispatch.** Same as Cursor — static input, not a learning system. Worth noting because the *repo map* idea ("here is a token-cheap structural index of the repo, freshly generated each time") is something dispatch could consider as a complement to learned context: structure from code, learnings from past runs.

### 2.6 MemGPT / Letta

**What.** Research project from UC Berkeley (paper: [arXiv 2310.08560](https://arxiv.org/abs/2310.08560)) and the company productising it. Frames the LLM as an OS managing its own memory: a small core in-context block, a recall layer (searchable conversation history), and an archival layer (long-term store the agent queries by tool). Letta v1 modernises this around standard ReAct-style agent loops. ([repo](https://github.com/letta-ai/letta), [Letta docs](https://docs.letta.com/concepts/memgpt/))

**How it works.** Agent itself decides what to *page in* and *page out* via tool calls. Persisted state is a per-agent identity + memory blocks. Aimed at long-lived stateful agents.

**Fit for dispatch.** Mismatch on lifecycle. Dispatch agents are short-lived (one worktree, one task), not long-lived stateful entities. MemGPT's core insight (paged memory inside a single agent's lifetime) is solving a different problem than dispatch has. The *shape* of "core / recall / archival" is interesting as a metaphor for "always-included CLAUDE.md / per-repo memory bank / searchable past-task archive" — but adopting Letta as infrastructure would force dispatch into a stateful agent platform it doesn't need.

### 2.7 Mem0

**What.** Production-leaning memory layer for AI agents. Extracts durable facts from conversation, stores them in a vector store with optional knowledge-graph layer (`Mem0g`), retrieves with hybrid (semantic + BM25 + entity) scoring. Benchmarks claim ~90% token reduction vs. full-context prompting. ([repo](https://github.com/mem0ai/mem0), [paper arXiv 2504.19413](https://arxiv.org/abs/2504.19413))

**How it works.** Two-phase: extraction pipeline summarises conversation into discrete fact units (e.g. "user prefers Python", "user is in CET"); retrieval pipeline re-injects relevant facts on each turn. Tunable per project (inclusion/exclusion prompts, memory depth, usecase profile).

**Fit for dispatch.** Genuinely useful for the *user-preferences* slice (e.g. "this user always wants TDD before code"), less so for the *repo-conventions* slice (which is better expressed as written rules a human can read and edit). Heavyweight to bundle: needs a vector store, an extraction model, an extraction schedule. For dispatch, would only make sense if user-preference learning is shown to be valuable enough to justify the infrastructure — not as v1.

### 2.8 LangMem (LangChain)

**What.** SDK for adding long-term memory to agents, integrates with LangGraph but works standalone. Distinguishes three memory types explicitly: semantic (facts), episodic (past situations + the reasoning that worked), procedural (the agent's own prompt, evolved over time via a metaprompt update loop). ([docs](https://langchain-ai.github.io/langmem/concepts/conceptual_guide/), [launch post](https://blog.langchain.com/langmem-sdk-launch/))

**How it works.** "Subconscious" reflection step after each interaction extracts insights into the chosen memory store; procedural memory updates the system prompt itself based on observed performance.

**Fit for dispatch.** The *taxonomy* is the most useful thing here, more than the SDK. Dispatch should think about memory in the same three buckets:
- **Semantic** — repo conventions, library quirks, user preferences. Maps to per-repo markdown.
- **Episodic** — past task transcripts and the plan + outcome. Maps to a queryable archive of past dispatch tasks (which dispatch already has in its DB!).
- **Procedural** — the dispatch prompt prefix itself, evolved over time. Maps to a learned-prompt component injected by `dispatch_with_prompt()` in `src/dispatch.rs`.

Adopting LangMem-the-SDK is a no — it is a Python framework and dispatch is Rust + Claude Code. Adopting LangMem-the-taxonomy is a strong yes.

### 2.9 Graphiti / Zep

**What.** Temporally-aware knowledge graph engine for agent memory. Facts have validity windows; old facts get *invalidated* rather than deleted; queries can ask "what's true now?" or "what was true at time T?". Hybrid retrieval (embeddings + keyword + graph traversal). Outperforms MemGPT on the DMR benchmark. ([Graphiti repo](https://github.com/getzep/graphiti), [Zep paper arXiv 2501.13956](https://arxiv.org/abs/2501.13956))

**How it works.** Each "episode" (a chunk of input data) is parsed into entities and relations and merged into the graph; conflict detection invalidates old edges with timestamps. Provenance from each entity back to its source episode.

**Fit for dispatch.** Most powerful, most expensive, lowest fit. The graph + temporal-fact model is overkill for the size of context any one repo accumulates. Dispatch is not building a customer-conversation memory layer; it is helping an agent remember "we use Slick, never raw SQL, and the last time someone tried `Future.sequence` we deadlocked the worker pool". That fits in markdown. Worth keeping as a reference for *what would replace markdown* if and when scale demands it.

### 2.10 Dispatch's existing learning surfaces

Already in this repo / workspace:

- **Allium specs** (`docs/specs/*.allium`) — domain rules, source of truth, human-edited.
- **Plan files** (`docs/plans/*.md`) — implementation plans, working artifacts.
- **Skills** (`.claude/skills/<name>/SKILL.md`) — invocable instructions, plugin-installed via `setup.rs`.
- **Tips** (`src/tips/*.md`, `docs/specs/tips.allium`) — startup hints; static, ID-numbered.
- **`CLAUDE.md`** at repo root and workspace root — always-on agent instructions.
- **Auto-memory** at `~/.claude/projects/<project>/memory/MEMORY.md` — *user-scoped*, written by Claude during normal sessions, indexed by topic. **Already a working semantic-memory store**, but per-user, not per-repo or per-team.
- **Retrospectives** (`kognic-claude-toolkit:retrospective` and `claude-md-management:revise-claude-md` skills) — manual self-update loops.
- **Dispatch DB** — historical record of every task, plan, dispatch, PR. *Episodic memory in disguise.* No retrieval API for it from a running agent.

These cover semantic memory partially (auto-memory + CLAUDE.md), expose almost no episodic memory (DB exists but unused), and have ad-hoc procedural memory (prompt prefix in `dispatch_with_prompt()` is hand-edited).

## 3. Synthesis

### 3.1 What dispatch needs (in LangMem's taxonomy, one unified store)

The store is **one table with a `scope` field**, not separate stores per layer. Scope answers "who/what does this learning apply to" and lets retrieval union across layers when dispatching.

| Memory | Content | Scope value(s) | Status today |
|---|---|---|---|
| Semantic — user preferences | "user wants TDD before code", "user prefers terse PRs" | `user` | partial (user auto-memory exists, but lives outside dispatch and is invisible to the TUI) |
| Semantic — repo facts | "this repo uses Slick, never raw SQL", "always run `cargo fmt --check`" | `repo:<id>` | **missing** |
| Semantic — epic context | "epic #27 is for self-learning; treat new domain entities as proposals" | `epic:<id>` | **missing** |
| Episodic — past task outcomes | "task #313 tried X, hit problem Y, settled on Z" | `task:<id>`, indexed by repo/epic | data exists in dispatch DB, **no agent retrieval pathway** |
| Procedural — dispatch prompt | the prompt prefix in `dispatch_with_prompt()` | special: rendered from learnings tagged `procedural` | static; humans edit |

Retrieval-on-dispatch unions: all `user` learnings + `repo:<this_repo>` + `epic:<this_epic>` + relevant `task:*` siblings → ranked → top-N injected into the prompt.

### 3.2 What's missing

1. **A unified "learnings" store** with a scope field, agents read at task start and propose to at task end. (Cline Memory Bank shape, cq schema, simpler than either, but generalised across user/repo/epic/task.)
2. **A capture mechanism** — explicit MCP tool the agent calls, plus an opt-in post-task reflection step that the dispatched agent runs before `wrap_up`.
3. **A retrieval-on-dispatch step** — dispatch prompt is augmented with top-N relevant learnings across all applicable scopes, not just `CLAUDE.md` and the plan.
4. **An ingestion path for past task outcomes** — agents should be able to query past plans + outcomes on this repo or epic. The dispatch DB already holds the data; what is missing is the MCP read surface and an indexing strategy that does not require parsing every old plan on every query.
5. **Migration / coexistence with user auto-memory** — existing user auto-memory at `~/.claude/projects/<project>/memory/` is the de-facto user-preferences store today. Either subsume it into the unified store (and write back to it for non-dispatch sessions), or treat it as a one-way input the unified store reads at startup. Decision deferred to WP1.

### 3.3 Recommendation

**Build dispatch-native, take cq's data shape, take Cline's storage simplicity, defer vector/graph stores until proven necessary. One table, scoped.**

Concretely:

- v1 storage: SQLite table `learnings` in dispatch's own DB, plus optional markdown export under each repo's `.dispatch/learnings/` for human readability and review.
- v1 schema (subset of cq's "knowledge unit", scoped):
  - `id`, `kind` (pitfall / convention / preference / tool-recommendation / procedural / episodic),
  - `summary`, `detail`,
  - `scope` (`user` | `repo:<id>` | `epic:<id>` | `task:<id>`),
  - `tags` (free-form),
  - `evidence` (created_at, last_confirmed_at, confirmed_count, source_task_id),
  - `status` (proposed / approved / rejected / archived).
- v1 capture: MCP tool `record_learning` (agent-callable, lands as `proposed`), plus an opt-in `wrap_up`-time reflection that nudges the agent to propose anything non-obvious it learned.
- v1 retrieval: MCP tool `query_learnings` (`scope`, `tags`, `limit`); dispatch prompt prefix is augmented with the top relevant entries unioned across `user` + `repo` + `epic` for the current task.
- v1 episodic ingestion: cheap path first — the same MCP tool also returns brief outcomes from sibling tasks in the same epic (`status`, `pr_url`, completed plan summary). No re-indexing of old plans in v1; just whatever the DB already has.
- v1 procedural memory: agents may *propose* `procedural` learnings; only human-approved entries are rendered into the dispatch prompt. No automatic prompt mutation.

Why not adopt cq directly: too young, too heavy (Python + Docker + Postgres for the remote tier), and its remote-tier story targets organisations, not a single-dev TUI. We keep cq's MCP shape compatible so a future bridge is cheap.

Why not adopt Anthropic's memory tool directly: would work, but pushes schema decisions onto each agent and exposes no retrieval surface to dispatch's TUI. Better: dispatch owns the storage, exposes MCP, and the same UI surfaces entries to the human user for review.

Why not LangMem/Mem0/Letta/Zep: heavier than this problem, all assume Python infra. We steal the *taxonomy* (LangMem) and the *schema* (cq), not the dependencies.

### 3.4 Risks to flag

- **Scope leakage.** With one store, a `repo:foo` learning could accidentally surface in a `repo:bar` dispatch if the retrieval query is sloppy. Retrieval must always anchor on the current dispatch's `(user, repo, epic, task)` tuple and reject scopes that do not apply.
- **Markdown export drift.** If the SQLite store is the source of truth and `.dispatch/learnings/*.md` is an export, hand-edits to the markdown will get clobbered. Either make the markdown one-way (export only, view-only for humans) or commit to bidirectional sync with conflict rules. Decision in WP1.
- **Auto-memory coexistence.** User auto-memory keeps writing to `~/.claude/projects/.../memory/` regardless of what dispatch does. If both stores hold "user prefers TDD", the agent sees it twice. WP1 needs to land on one of: (a) ignore auto-memory inside dispatched sessions, (b) one-way import at session start, (c) treat dispatch's `scope=user` as canonical and stop using auto-memory for dispatch.
- **Procedural-memory updates** are the high-leverage / high-risk surface — a bad learning that ends up in every prompt poisons every agent. Keep human-in-the-loop. Agents *propose*, humans *approve*.
- **SQLite scaling.** Plain text search inside SQLite is fine until the store grows large. Build the MCP surface around the *query semantics* (scope + tags + free-text), not the storage details, so swapping in FTS5 or embeddings later is a one-module change.

## 4. Refined epic goal (proposed)

**Epic #27 — "Build a unified learning loop covering every aspect of development"**

Dispatched agents repeatedly rediscover the same things: user preferences, repo conventions and library quirks, the context of an ongoing epic, and the outcomes of past tasks. Build one unified learning store inside dispatch covering all four scopes (`user`, `repo`, `epic`, `task`). Agents query relevant learnings on dispatch and propose new ones on success; humans review proposals in the TUI before they affect future dispatches. Storage is SQLite-first with optional markdown export; capture and retrieval are MCP tools so the backend can evolve. Existing user auto-memory at `~/.claude/projects/.../memory/` is reconciled (subsumed, imported, or coexisting — decided in WP1) so there is one canonical store for dispatched agents.

Success criteria:

1. A dispatched agent can call `query_learnings` and get the top-N relevant entries unioned across `user` + `repo` + `epic` for the current task.
2. A dispatched agent can call `record_learning` to propose a new entry at any of the four scopes; entries land as `proposed` and are reviewed in the TUI before affecting future dispatches.
3. The dispatch prompt prefix automatically includes the top-relevant approved learnings.
4. Past task outcomes (status, plan summary, PR) are reachable via the same MCP surface, scoped to the current repo / epic.
5. Allium specs (`core.allium` + a new `learning.allium` or extension to `tasks.allium`) document the unified store and its scope model.
6. End-to-end demo: dispatch task A in a repo, agent records a `repo`-scoped learning; dispatch task B in the same repo, agent's prompt cites that learning before it acts.

Out of scope for this epic: cross-repo / organisational sharing (cq's remote tier), vector or graph retrieval, automatic prompt-prefix mutation without human review.

## 5. Proposed work packages

Each can be dispatched as one task. Tag suggestions in brackets. Dependencies noted.

1. **[research] Spike: design the unified `Learning` entity, scope model, and auto-memory reconciliation.** Pin: SQLite schema (id, kind, summary, detail, scope, tags, evidence, status); rules for `scope` resolution; markdown export shape and direction (one-way vs. bidirectional); decision on user auto-memory coexistence (subsume / import / ignore); privacy/secrets handling. Inputs: this note. — *no deps*

2. **[feature] Allium spec additions for `Learning`.** Extend `core.allium` (entity + scope enum) and `tasks.allium` (rules: read learnings on dispatch, write learnings on wrap-up, review flow). Use `allium:tend` to write, `allium:weed` to verify alignment. — *deps: 1*

3. **[feature] Database migration + service layer.** New `learnings` table, `LearningStore` trait, `LearningService` mirroring `TaskService`. Tests with `Database::open_in_memory()`. — *deps: 2*

4. **[feature] MCP tools: `query_learnings`, `record_learning`, `confirm_learning`.** Wire into `src/mcp/handlers/`, schema via `tool_definitions()`. Query unions across applicable scopes for the calling task. Keep the surface cq-shape-compatible. — *deps: 3*

5. **[feature] Episodic ingestion: past task outcomes via the same MCP surface.** `query_learnings` (or a sibling tool) returns brief summaries of sibling tasks in the same epic / repo (status, plan summary, PR). No re-indexing of old plans — use what the DB already has. — *deps: 4*

6. **[feature] Dispatch-prompt augmentation.** In `dispatch_with_prompt()`, fetch top-relevant approved learnings for the task and prepend them. Simple ranking first (keyword + tag + scope match); defer embeddings. — *deps: 4*

7. **[feature] TUI surface for reviewing proposed learnings.** A new view (or extension of the tip browser) that lists `proposed` entries grouped by scope, lets the user approve / edit / reject. — *deps: 4*

8. **[feature] Post-task reflection hook.** Opt-in step at `wrap_up` time: dispatch nudges the agent to call `record_learning` with anything non-obvious learned during the task. Behind a config flag. — *deps: 4*

9. **[research] Spike: safe procedural-memory updates.** When may agent-proposed `procedural` learnings flow into the dispatch prompt? Recommend the review/approval gates and any guardrails (size cap, tag restrictions, opt-out). — *no hard dep; parallel*

10. **[chore] Documentation: scoping rules within the unified store.** Short doc (in `docs/reference.md`) explaining how `user` / `repo` / `epic` / `task` scopes interact at retrieval time, with examples. — *deps: 6*

11. **[research] Evaluation: did it work?** After 6 + 7 land, dispatch the same task twice (cold vs. warm) in a small repo; compare quality and token use. Decide whether the next epic invests in vector retrieval, cross-repo sharing, or auto-memory deprecation. — *deps: 6, 7*

(11 is intentionally last — it's how we earn the right to start the *next* epic.)

## 6. Sources

- mozilla-ai/cq — [repo](https://github.com/mozilla-ai/cq), [architecture](https://github.com/mozilla-ai/cq/blob/main/docs/architecture.md), [Hacker News thread](https://news.ycombinator.com/item?id=47491466)
- Anthropic memory tool — [API docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/memory-tool), [effective harnesses for long-running agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- Cline Memory Bank — [docs](https://docs.cline.bot/features/memory-bank), [methodology blog](https://cline.bot/blog/memory-bank-how-to-make-cline-an-ai-agent-that-never-forgets)
- Cursor rules — [docs](https://cursor.com/docs/rules)
- Aider — [repo map](https://aider.chat/docs/repomap.html), [conventions](https://aider.chat/docs/usage/conventions.html)
- MemGPT / Letta — [paper arXiv 2310.08560](https://arxiv.org/abs/2310.08560), [repo](https://github.com/letta-ai/letta)
- Mem0 — [paper arXiv 2504.19413](https://arxiv.org/abs/2504.19413), [repo](https://github.com/mem0ai/mem0)
- LangMem — [conceptual guide](https://langchain-ai.github.io/langmem/concepts/conceptual_guide/), [SDK launch](https://blog.langchain.com/langmem-sdk-launch/)
- Graphiti / Zep — [Graphiti repo](https://github.com/getzep/graphiti), [Zep paper arXiv 2501.13956](https://arxiv.org/abs/2501.13956)
