# ADR-075: MCP server manager

**Status:** Accepted

## Context

Each provider CLI keeps its own MCP servers in its own file and format, so a person who uses several providers has no single place to see or add them. The owner asked for an MCP settings tab with a paste-JSON modal and a `/mcp` composer picker in Sirus.

## Decision

- **Source of truth stays with the providers.** Sirus keeps no server registry. Settings → MCP servers reads the providers' files through a closed `mcp_action` (list, add, remove): Claude Code (`~/.claude.json` `mcpServers`, its per-project `local` entries, project `.mcp.json`), Codex (`$CODEX_HOME/config.toml` `[mcp_servers]`, project `.codex/config.toml`), OpenCode (`opencode.json[c]` `mcp`, user and project) and Cursor (`~/.cursor/mcp.json`, project `.cursor/mcp.json`). Each row shows name, command or URL, transport, scope, provider and file; the same name in several providers is grouped.
- **Secrets stay native.** Only the names of environment variables and headers cross IPC; the UI shows `KEY=••••`. TOML parse errors, which quote the failing line, are replaced by a generic message, and logs carry provider, scope and server name only.
- **Add.** The dialog accepts a standard `mcpServers` block, OpenCode's `mcp` map, a bare map of servers or one server object, or a small form (name, command + arguments + environment, or URL + headers). The person picks providers and the user or project scope. Native code validates again (names `[A-Za-z0-9_-]{1,64}`, `sirus_` reserved, command xor http(s) URL, bounded arguments, environment and headers) and writes each provider's own shape. An existing name is never overwritten.
- **Safe writes.** One in-process lock serializes edits. Each edit rereads the file, refuses one that does not parse, copies it to `<file>.sirus-backup`, changes one entry, and replaces the file through a synced temporary sibling with the original permissions (0600 for new files), following symlinks to the real file. Codex files are edited with `toml_edit`, so comments and layout survive; JSON files keep every field but may change key order.
- **`/mcp`.** A composer app command (ADR-060) opens a small card above the composer with the servers of the session's provider for its project, and a Manage link to Settings. It animates in and out.

## Consequences

- **Positive:** one view of every provider's servers; adding one server to four providers is one paste.
- **Negative:** JSON key order may change on write, and a provider that rewrites its file at the same moment can race with the edit (the backup covers it). Provider account profiles (`CLAUDE_CONFIG_DIR`, per-account `CODEX_HOME`) and Claude Desktop are not read. Enable/disable and sign-in flows are not offered.

## Alternatives considered

- **A Sirus registry injected per session.** Rejected for now: it duplicates what the CLIs already manage, and servers would differ between Sirus and the terminal.
- **Shelling out to `claude mcp add` / `codex mcp add`.** Rejected: not every CLI has one, and their flags differ by version.
