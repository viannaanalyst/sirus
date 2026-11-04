# ADR-137: Connectors

- Status: accepted
- Date: 2026-10-16

## Context

Agents become far more useful when they can reach the services a project lives on (its Supabase database, its Linear issues, its Sentry errors). Remote MCP servers give that, but each provider CLI keeps its own MCP configuration (ADR-075), so the person had to add a server to Claude, Codex and OpenCode one by one, and sign in from each CLI. Headless agents cannot open a service's sign-in page at all. The expected shape: a catalog with the brands' marks; connect once, every agent receives it.

## Decision

- **One list, in the settings.** `AppSettings.connectors` holds each connector: catalog id or none (custom URL), name, an https URL, the agents that receive it, on or off, the sign-in kind (`oauth`, `token`, `none`) and either one project or all projects. A conversation receives the all-project connectors plus its own project's, nothing from other projects: three projects can each have their own Supabase, pinned by `project_ref` and optionally `read_only=true` in the URL, even on different Supabase accounts.
- **Sirus signs in, not the CLIs.** `connectors.rs` follows the MCP authorization flow: the server's 401 challenge names its protected-resource metadata (RFC 9728), whose authorization server metadata is found with RFC 8414 path insertion (Stripe and monday.com need it), Sirus registers itself (dynamic client registration), opens the authorize page in the browser with PKCE (S256), `state` and the `resource` indicator, takes the code on a one-shot `127.0.0.1` redirect, and exchanges it. GitHub's remote server has no registration, so it takes a pasted token; Context7 needs none.
- **Tokens stay out of settings, argv and logs.** They live in the macOS Keychain (`security -i`, the value on stdin) or, on a headless host, in a 0600 file next to the state; they are refreshed when a session starts within two minutes of expiry. Sessions get them as follows:
  - Claude: in its per-session `--mcp-config` file, created 0600, as HTTP servers with an `Authorization` header;
  - Codex: `-c mcp_servers.<name>.url` and `bearer_token_env_var`, the token in the process environment (ADR-100);
  - ACP agents (OpenCode, Cursor, Devin, Hermes): `mcpServers` HTTP entries when the agent advertises `mcpCapabilities.http`.
  Grok, Antigravity, Droid and Pi take none yet. A change of connectors or tokens is part of a kept process's pool key. Signed-out connectors are left out; the session runs without them.
- **Approvals.** In "ask" mode a Claude connector tool (`mcp__<connector>__<tool>`) reaches the request card like a built-in tool instead of being refused.
- **Settings → Connectors** replaces the MCP servers page title: search, "Add custom", an All / This project filter, Connected rows (scope tag, sign-in button, a menu to sign in again, sign out, edit, turn off, remove), and the catalog in Popular, Developer tools and Productivity. The CLIs' own servers (ADR-075) and the session tools switch (ADR-076) move under "Advanced". Marks are the brands' official ones: Simple Icons 16.35.0 paths (CC0) in each brand's color, near-black ones following the text color, and the marks of monday.com, Canva and Context7 from their own sites.
- **The catalog was checked live** (October 2026): Supabase, Linear, Notion, Stripe, Vercel, Sentry, Cloudflare, Neon, PostHog, Netlify, Railway, Prisma, Figma, Hugging Face, Atlassian, Asana, Todoist, monday.com, Canva, Intercom, Webflow, Wix, PayPal and Square all answer with metadata and registration (an ignored test, `catalog_services_offer_sign_in`, repeats the check). Slack's server has no registration and was left out.

## Consequences

- Connect once, on the Mac; every supported agent of the right projects then has the service.
- Sign-in needs the Mac app: the redirect comes back to the machine's loopback. A headless host accepts token connectors and the list; signing in there is for later.
- A Claude connector tool verified end to end with Context7 (`mcp__context7__resolve-library-id`), and Codex with the same `-c` overrides and a bearer variable.

## Amendment, 2026-11-04: Cloudflare moved

The catalog's Cloudflare entry pointed at `bindings.mcp.cloudflare.com`, which Cloudflare marked as deprecated; it only reaches Workers bindings, so an agent could not add a domain or change DNS. It now points at `mcp.cloudflare.com/mcp`, which covers the whole API through `search` and `execute`. At startup, `connectors::start` moves catalog connectors still on the old address, saves, and only then drops their old tokens, so the person signs in once more. Custom connectors keep the address they were given. That server's consent page starts at the minimal scopes: to change anything, choose the access on that page, or the agent can only read.
