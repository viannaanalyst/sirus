# ADR-186: Browser login vault the agent can use without seeing passwords

**Status:** Accepted (2026-10-28). Built and unit-tested. The Keychain item, the real WKWebView script-message path, and a real sign-in on a real site are checked by hand on a Mac, not in CI.

## Context

Agents that drive the Sirus browser (ADR-030, ADR-087, ADR-150) hit sign-in pages. Until now the choices were `request_secret` (ADR-077), which writes a value to a file or an env var and cannot type it into a page, cookie import (ADR-152), or the person signing in by hand each time. Another desktop agent app ships a login vault with a master password and a "save after sign-in" prompt. We want the same convenience under Sirus's rule that a secret never enters the model's context.

## Decision

### Storage

- Saved logins (origin, username, password, optional TOTP seed, an "agents may use" flag) are one JSON document in one macOS Keychain generic-password item (service `Sirus browser logins`, account `vault`), through `security-framework` like connectors (ADR-137). Nothing is written to the app data folder or `state.db`.
- The store sits behind a `SecretStore` trait (`read`/`write` of that document). The Keychain implementation is the only one in the app; tests use an in-memory double. On non-macOS builds the store reports that the Keychain is needed.
- Writes are applied to a copy, saved, then adopted, so a refused Keychain write changes nothing. The decrypted list is cached in memory after the first read; passwords are zeroed when a login is dropped.
- The renderer receives `LoginSummary` only (origin, username, a fixed mask, `hasTotp`, `agentUse`). There is no command that returns a password or a seed. Passwords go in through `login_vault_save`; on an edit an empty password keeps the saved one.
- No master password. The Keychain item is the protection (see the threat model). A second layer would add a prompt on every launch for little gain on a Mac whose login keychain is already locked with the account.

### Origins

`origin_of` reduces an address to `scheme://host[:port]`, lower case, default port dropped. Only `https` qualifies, plus `http` for loopback (`localhost`, `*.localhost`, `127.0.0.1`). User info, bracketed IPs, non-ASCII hosts and anything else are rejected. Matching is string equality on that origin: no subdomain, no public-suffix or "same site" guessing, no port widening. `www.example.com` and `example.com` are different logins.

### Save offer

- A document-start script runs in WebKit's client content world (invisible to the page, with a message handler the page cannot call) in the main frame of every tab. On a trusted submit, Enter in an input, or a sign-in-looking button click it reads a single non-empty visible password field and the username field before it, and posts `{u, p}`.
- It ignores untrusted events, empty passwords, change-password forms (two different password values), hidden fields, and the "show password" eye. Duplicate reports within two seconds count once.
- Rust takes the site from the webview's frame address, never from the message. It queues an offer only for an eligible origin, when the report has a username, in a non-private tab, and when the same login is not already saved with that password. The same site and username with a different password becomes "Update the saved password".
- The offer (site and username, never the password) appears in the browser pane: Save, Save with agents allowed, Not now. The password stays in native memory (zeroed on drop) until answered, for at most ten minutes, and is dropped at quit.

### Agent use

- New tool `browser_fill_login { username?, part? }` on the browser MCP bridge (behind Agent browser access like every `browser_*` tool). The model can name a username when several logins match and choose `part: "totp"` for the authenticator code. It cannot pass a password, an origin or a login id.
- The origin is read from the active tab's address on the Rust side. Only logins with that exact origin and "agents may use" on are candidates; with several, the model must name a username (the error lists usernames only).
- The first use per site and session asks the person with a card in the conversation (Allow or Deny, like the computer-use card). The answer is remembered for that session and origin until the app quits; a waiting card is declined when the turn ends or after two minutes. The login is looked up again after the card, so an edit or a page change during the wait is honoured.
- The values travel from Rust to the page as the JSON `args` of a fixed script run with `callAsyncJavaScript`; they are never spliced into source. The script re-checks `location.origin` against the expected origin, fills the username and password fields (or the code field or one digit per box), sets values through the native setter and dispatches `input` and `change`. It does not submit.
- The tool result is `filled for <origin>` plus the names of the fields filled. A failure says why (no login, not marked for agents, several logins, the person declined, no fields on the page) with no value in it. The password and any code are also registered with the secret scrubber (ADR-077), so an echo in output or an activity row is masked for the rest of the turn.
- The agent instructions mention the tool so agents try it before `request_secret` for a site password.

### The person's side

- Settings → Browser → Saved logins: add, edit (password and seed are write-only), delete (with confirmation), and the "Agents may use this login" switch. Logins default to not shared with agents.
- A key icon in the address bar appears when saved logins match the page's exact origin; it fills the chosen login (and the authenticator code when saved) into the active tab. No confirmation is asked, since it is the person's own click. The same exact-origin check applies.

### TOTP

A seed is optional: Base32 or an `otpauth://` link, at least 80 bits. Codes are RFC 6238 (SHA-1, 30 seconds, 6 digits), checked against the RFC's test vectors, computed in Rust at fill time and sent to the page like a password.

## Threat model

**Protected against**
- The agent, the model provider, transcripts, logs and `state.db`: the model never receives a password, seed or code; tool results and activity carry field names only; the scrubber masks echoes; nothing sensitive is in the renderer's store or events.
- Another app or a stolen disk without the login keychain unlocked: the vault is encrypted by the Keychain. Another process running as the same user can ask the Keychain for the item and macOS shows its access prompt for apps that are not the creator.
- A page on another origin, a look-alike or an HTTP downgrade: exact-origin matching, ASCII-only hosts, `https` only, the origin checked again inside the page.
- A page forging a save offer or reading the capture channel: the handler lives in the client world, the origin comes from the webview, and an offer needs the person to click Save.
- A silent agent sign-in: first use per site per session needs the person's Allow, and only for logins the person marked.

**Not protected against**
- A malicious or compromised page at the matching origin. A filled password is in that page's DOM, like in any password manager, and the page's scripts can read it (and could replace the value setter). Saving a login for a site means trusting that site's scripts.
- An agent with `browser_evaluate`, `browser_snapshot` or a screenshot after a fill: it can read the password field's value, or see it if a page shows it. Masking catches exact echoes in output, not a transformed copy. Turning off Agent browser access removes those tools; marking a login as not usable by agents removes the fill.
- Malware running as the user, or someone using an unlocked Mac, who can read the Keychain after approving its prompt.
- An agent that gets the person to click Allow for a site it should not touch: the card names the site and username, and the first fill is the only prompt.
- A login saved from a page that lied about being a sign-in form: the person sees the site and username before choosing Save.

## Consequences

- Agents can sign in to sites without anything secret reaching their context, and the person gets autofill and a save prompt in the same pane.
- One Keychain item holds everything. A refused or locked Keychain fails the whole vault with a message; there is no per-login recovery. Up to 500 logins are kept.
- Sign-ins that never use a visible password field and a submit, Enter or button (passkeys, some OAuth pop-ups, cross-origin iframes) are not captured or filled. Fill acts on the main frame only.
- The agent cannot sign in on a site the person has not saved and enabled, and cannot enumerate sites: the only answer for a wrong page is that no usable login exists there.
- Not done: a master password and auto-lock, "never save for this site", import from other password managers, passkeys, filling inside iframes.
- Verified by unit tests: origin matching (subdomains, ports, schemes, user info, look-alikes), the Keychain round trip against a double (including a refused write), summaries that carry no password, agent filtering (exact origin, usable flag, several logins), save-offer parsing and classification, RFC 6238 vectors, the fill result that carries no value, and both page scripts against a fake DOM (capture rules and fill rules). Not verified here: the real Keychain item and its access prompt, the WKScriptMessageHandler delivery in the running app, and signing in to a real site.
