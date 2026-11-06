# ADR-006: Radix for behaviour, Sirus-owned visuals

**Status:** Accepted

## Context

The product must feel like a Mac developer tool, not a shadcn dashboard. Accessible menus still need primitives.

## Decision

- Tailwind v4 + CSS variables in `src/styles/index.css`.
- Radix: dialog, dropdown, popover, tooltip, context-menu, scroll-area.
- Visuals: existing first-party primitives adapt vendored UI Arc MIT components. Sirus owns theme, motion, focus and sizing tokens; upstream source and license are retained. The component explorer is lazy and its local examples are separate from product data.
- Motion for React only where it adds press/presence; CSS tokens for duration/easing.
- Control icons: Lucide. Provider/model identity uses bundled vendor assets with provenance; unavailable official assets use a documented neutral placeholder.
- UI state: Zustand (`src/store/app-store.ts`). Do not add Redux/Query/another store without an ADR.

## Consequences

- **Positive:** one look; behaviour is accessible; tokens enable a later light theme.
- **Negative:** vendored components require upstream review and CSS integration; the complete optional explorer increases its own lazy bundle.
- **Accepted trade-off:** reuse a licensed kit while preserving host behavior and the existing primitive API. See [Arc integration](../development/ui-arc.md).

## Alternatives considered

- **Unmodified shadcn/ui:** rejected in the design spec.
- **Framer Motion as a second animation library:** we use `motion` (Motion for React) only.
