# Documentation conventions

How Sirus is documented, and why. Read this before writing or restructuring documentation.

**The rule everything else serves:**

> Document enough that a new maintainer or an agent understands *why the system is like this* without rediscovering old decisions.

Documentation that does not serve that is duplication or a changelog. Git already has the changelog.

The *application* of these frameworks is Sirus-specific: a local-first Tauri desktop host for coding agents, not a browser, not a business-process repo, not an Electron chat wrapper.

## Language

| Surface | Language |
| --- | --- |
| Code, identifiers, commit messages, filenames | **English** |
| `docs/`, `AGENTS.md`, ADRs | **English** |
| `README.md` | Portuguese today (owner-facing). Do not duplicate it in English unless it drifts from `docs/development/building.md`. |
| Application UI | Português (Brasil) by default, with persisted English preference. External tool output, code, paths and branches keep their original language. Do not translate docs into pt-BR to match the UI. |

Do not maintain parallel doc trees per language.

## One source of truth per topic

Duplicated documentation is worse than missing documentation.

| File | Contains | Does **not** contain |
| --- | --- | --- |
| [`AGENTS.md`](../AGENTS.md) | Operational rules for agents, current architecture they must preserve, Graphify usage, security invariants | Changelog, future-remote design fiction |
| [`docs/decisions/`](decisions/README.md) | **Why** a decision was made, what was rejected, what it cost | How to operate the result |
| [`docs/development/building.md`](development/building.md) | Build, typecheck, run | Architecture rationale |
| [`README.md`](../README.md) | Overview and how to start | Depth |
| [`graphify-out/GRAPH_REPORT.md`](../graphify-out/GRAPH_REPORT.md) | Generated structural report | Hand-written architecture |
| `docs/documentation-conventions.md` | This file | Product behaviour |

Cross-reference with **relative links**. Never paste the same paragraph into two files.

There is **no** `ARCHITECTURE.md` yet. V1 is small enough that AGENTS.md plus ADRs are the architecture. Split an `ARCHITECTURE.md` only when AGENTS.md starts mixing “how to work” with long structural narrative.

There is **no** `SECURITY.md` yet. Security rules live in AGENTS.md because they are operational constraints on every change. Split a threat-model doc when remote access or credentials appear.

## Frameworks, and where they earn their place

These conventions descend from software *and* process documentation. Applying SIPOC/RACI/BPMN to a single-user desktop app produces documents nobody reads.

Adopted, because they answer questions this codebase actually has:

### C4 Model — architecture

Simon Brown's four levels. Sirus uses **two**, and no more:

- **Level 1, Context:** a local desktop app that talks to the OS, Git, and already-installed coding-agent CLIs (Codex, Claude Code). No required cloud. README + AGENTS.md opening.
- **Level 2, Containers:** React UI (Vite webview) ↔ Sirus Client API ↔ LocalTransport ↔ Tauri IPC ↔ Rust core. This is a *build and trust-boundary* fact, not a pretty diagram. `src/client`, `src-tauri/src/lib.rs`, and AGENTS.md are the authority.

Level 3 (components) is the source tree. Level 4 (code) is the code. **Do not draw those.** A diagram that drifts is worse than none.

Graphify is the living Level 2/3 map. Query it; do not duplicate it in Markdown.

### ADR — decisions

Michael Nygard's format. Highest-value docs in this repo: they capture reasoning the source cannot.

- Template and index: [`docs/decisions/README.md`](decisions/README.md)
- Write one when a change constrains future work, is expensive to reverse, or resolved an argument (IPC shape, agent execution, persistence, destructive Git).
- Do **not** write one for a colour token, an icon, a dependency patch bump, or anything recoverable from the code in under a minute.

### Diátaxis — check what kind of document you are writing

Not a folder structure to fill:

| Quadrant | In Sirus today |
| --- | --- |
| **Tutorial** | README “Uso inicial” |
| **How-to** | `docs/development/building.md` |
| **Reference** | AGENTS.md (IPC list, module map, agent argv) |
| **Explanation** | `docs/decisions/*` |

If a doc mixes quadrants and it hurts, split it. Do not split preemptively.

## Frameworks that are conditional

**Not** required for a feature:

| Framework | Use only when |
| --- | --- |
| **BPMN** | A real multi-actor process with branches (release with approvals). Never for code flow. |
| **SIPOC** | Scoping inputs/consumers *outside* the repo. Not needed for V1. |
| **RACI** | More than three people share recurring responsibility. Sirus is currently one owner plus coding agents; a RACI would be theatre. |
| **VSM** | Measuring lead time in a repeated process with a wait state. Plausible later for release, not now. |
| **Data dictionary** | When persisted JSON grows a public schema with non-obvious field values. `src-tauri/src/models.rs` + `src/client/types.ts` are the schema today. |

If you reach for one of these, say in the document why the simpler option was not enough.

## Rules

### Documentation describes the current state, not the history

Do not write “this used to be X”. If a decision was reversed, the ADR records `Superseded by ADR-YYY`; AGENTS.md describes the new state.

No public changelog until there are public releases.

### Documentation changes in the same task as the code

Update docs in the same change when the work alters:

- the UI → Client → Transport → Rust boundary
- Tauri command surface or capabilities
- path jail / Git / PTY / agent process model
- persisted `state.json` shape
- AgentProvider set or CLI argv
- worktree layout or destructive-Git policy
- design-system primitives / tokens that others must reuse
- Graphify conventions

Do **not** update docs for copy, padding, typography, or an isolated component tweak.

### Never document an unverified claim as fact

If a CLI flag, Git behaviour, or Tauri capability was not checked against this tree, say so. Do not invent “agent connected” status in docs any more than in the UI.

### Do not fragment

A new file must earn itself: separate audience or separate lifecycle. ADRs are separate because they freeze. GRAPH_REPORT.md is separate because it is generated.

## Quality checklist

**Accuracy**

- [ ] Every claim is true of the code **as it is now**.
- [ ] Command names, crate names, paths, and CLI argv match the repo.
- [ ] Unverified claims say so.

**Usefulness**

- [ ] A reader who has never seen this repository can tell *why*, not only *what*.
- [ ] Rejected alternatives are recorded so they are not re-litigated.

**Maintainability**

- [ ] The fact lives in one place; others link.
- [ ] Links are relative and resolve.
- [ ] It is not a changelog entry.
- [ ] No process framework was applied that the content did not need.

## Repository skills vs AGENTS.md

Keep agent instructions in `AGENTS.md`. A skill under `.agents/skills/` or `.cursor/skills/` **in this repo** is only justified if it captures a repeatable Sirus-specific procedure too detailed for AGENTS.md (for example: adding an AgentProvider end-to-end, or a worktree safety checklist with commands).

Nothing has been created in-repo. Host skills (Graphify, React, Tauri-adjacent, review) live in the developer’s environment — see AGENTS.md § Skills.
