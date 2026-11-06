# Graph Report - src-tauri  (2026-09-30)

## Corpus Check
- 17 files · ~9,171 words
- Verdict: corpus is large enough that graph structure adds value.
- Unclassified: 5 file(s) not represented in the graph (top: .xml 2, (none) 1, .icns 1)

## Summary
- 213 nodes · 536 edges · 13 communities (10 shown, 3 thin omitted)
- Extraction: 97% EXTRACTED · 3% INFERRED · 0% AMBIGUOUS · INFERRED: 18 edges (avg confidence: 0.85)
- Token cost: 0 input · 0 output

## Community Hubs (Navigation)
- commands.rs
- models.rs
- PtySession
- paths.rs
- tauri.conf.json
- error.rs
- agent.rs
- git.rs
- default.json
- persist.rs
- switchyard

## God Nodes (most connected - your core abstractions)
1. `AppState` - 30 edges
2. `Session` - 16 edges
3. `send_prompt()` - 13 edges
4. `PtySession` - 13 edges
5. `AppData` - 12 edges
6. `set_session_agent()` - 11 edges
7. `complete_agent()` - 11 edges
8. `now_rfc3339()` - 11 edges
9. `session_cwd()` - 10 edges
10. `find_session_mut()` - 10 edges

## Surprising Connections (you probably didn't know these)
- `add_project()` --calls--> `display_path()`  [INFERRED]
  src/commands.rs → src/paths.rs
- `send_prompt()` --calls--> `display_path()`  [INFERRED]
  src/commands.rs → src/paths.rs
- `start_terminal()` --calls--> `display_path()`  [INFERRED]
  src/commands.rs → src/paths.rs
- `list_children()` --calls--> `is_ignored_dir()`  [INFERRED]
  src/fs_tree.rs → src/paths.rs
- `list_worktrees()` --calls--> `display_path()`  [INFERRED]
  src/git.rs → src/paths.rs

## Import Cycles
- None detected.

## Communities (13 total, 3 thin omitted)

### Community 0 - "commands.rs"
Cohesion: 0.23
Nodes (38): PtyMap, add_project(), append_agent_output(), AppState, complete_agent(), create_session(), delete_session(), detect_agents() (+30 more)

### Community 1 - "models.rs"
Cohesion: 0.12
Nodes (33): Default, pathbuf, serde, detect(), KNOWN, KnownAgent, resolve(), resolve_known() (+25 more)

### Community 2 - "PtySession"
Cohesion: 0.08
Nodes (25): appstate, arc, Box, HashMap, manager, MasterPty, portable_pty, ptyoutputevent (+17 more)

### Community 3 - "paths.rs"
Cohesion: 0.16
Nodes (23): fs, list_children(), Path, Result, Vec, Worktree, canonicalize_existing(), display_path() (+15 more)

### Community 4 - "tauri.conf.json"
Cohesion: 0.11
Nodes (17): app, security, windows, build, beforeBuildCommand, beforeDevCommand, devUrl, frontendDist (+9 more)

### Community 5 - "error.rs"
Cohesion: 0.29
Nodes (9): Into, Ok, S, Serialize, Error, ErrorPayload, Result, Self (+1 more)

### Community 6 - "agent.rs"
Cohesion: 0.18
Nodes (14): io, process, R, AgentProcess, AppHandle, Arc, Child, Mutex (+6 more)

### Community 7 - "git.rs"
Cohesion: 0.41
Nodes (12): display_path, Output, diff_file(), identity(), list_worktrees(), Path, Result, String (+4 more)

### Community 8 - "default.json"
Cohesion: 0.33
Nodes (5): description, identifier, permissions, $schema, windows

### Community 9 - "persist.rs"
Cohesion: 0.80
Nodes (4): load_or_create(), PathBuf, Result, save()

## Knowledge Gaps
- **20 isolated node(s):** `switchyard`, `$schema`, `identifier`, `description`, `windows` (+15 more)
  These have ≤1 connection - possible missing edges or undocumented components. (Counts symbols only; 61 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **3 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `AppState` connect `commands.rs` to `models.rs`, `PtySession`, `agent.rs`?**
  _High betweenness centrality (0.047) - this node is a cross-community bridge._
- **What connects `switchyard`, `$schema`, `identifier` to the rest of the system?**
  _20 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `models.rs` be split into smaller, more focused modules?**
  _Cohesion score 0.11904761904761904 - nodes in this community are weakly interconnected._
- **Should `PtySession` be split into smaller, more focused modules?**
  _Cohesion score 0.07956989247311828 - nodes in this community are weakly interconnected._
- **Should `tauri.conf.json` be split into smaller, more focused modules?**
  _Cohesion score 0.1111111111111111 - nodes in this community are weakly interconnected._