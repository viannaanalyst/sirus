# ADR-192: The phone is rebuilt around the Mac's conversation, and other conversations announce themselves

- Status: accepted
- Date: 2026-11-06

## Context

Working in several projects at once, nothing told you when another one finished or asked for permission: the host emitted `notification-activity` and `notification-native`, but the window listened to neither, and the phone app has no system push. The phone's conversation also showed less than the Mac's: no subagents, a flat list of steps, and a composer without `/`, `@` or modes.

## Decision

- **Cards on the Mac (`activity_cards.rs`).** Another conversation's permission, question, finish or failure shows as a card at the top right, at most three and then "+N avisos". Permission is answered on the card; finished cards leave after 6 s; the rest stay until answered. A project row carries a badge. No card when that conversation is on screen in a focused window.
- **Banners on the phone (`NotificationBanners`).** The same events drop a banner from the top while the app is open. There is no system push with the app closed: it needs a paid Apple account.
- **The phone's conversation matches the Mac's:** the step tree in the Mac's words, the subagent card and sheet, a working pill, and a composer with `+`, `/`, `@`, photos, modes and a model button showing only the provider's mark.
- **New navigation.** Novo, Conversas and Painel in a top bar, a projects drawer, a Painel replacing Home and a Custo screen.
- **The Mac's accent reaches the phone** through `phone_look_set` and `phone_look_get`, and cost through `usage_summary`, a few KB instead of megabytes of `usage_history`.
- **Type and bubbles follow Monocode's measures** on both, and the phone uses the system font.

## Consequences

- Price logic exists in the window (`usage_page.rs`) and the host (`usage_summary.rs`), which are separate crates; a test holds them to the same figures.
- The window now also receives the host's sounds and system banners, which it never listened to before.
