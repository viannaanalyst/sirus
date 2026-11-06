# ADR-002: UI talks only to the Sirus Client API

**Status:** Accepted

## Context

The same Session must eventually be controllable from desktop, browser, and iPhone. If React components call `@tauri-apps/api` directly, remote clients cannot reuse the UI logic and every screen becomes a Tauri adapter.

## Decision

```
UI → SirusClient (`src/client`) → Transport → core
```

Today `LocalTransport` dynamically imports Tauri `invoke` / `listen`. `RemoteTransport` does not exist. The only intended Tauri imports outside `local-transport.ts` are none.

Lives in `src/client/index.ts`, `src/client/transport.ts`, `src/client/local-transport.ts`.

## Consequences

- **Positive:** UI is host-agnostic; IPC can be listed and versioned in one class.
- **Negative:** an extra hop; easy to violate by importing Tauri in a component.
- **Accepted trade-off:** remote protocol is not designed yet. Preserve the seam; do not invent WebSockets.

## Alternatives considered

- **Call `invoke` from Zustand/components:** faster to write V1, fatal for remote.
- **GraphQL/tRPC over localhost:** premature; Tauri commands are already the API.
