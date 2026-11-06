# ADR-183: React Compiler, guarded by a test

**Status:** Accepted (2026-10-28).

## Context

The UI leans on hand-written `memo`, `useMemo` and `useCallback`. Another open-source client (github.com/pingdotgg/another open-source client, MIT)
compiles every web file in a test and fails on any compiler bailout, because a file the compiler skips
still builds but silently loses its automatic memoization. Another open-source client (MIT) turns the compiler on for builds and
tests and makes it opt-in in dev, since large chat modules make the transform slow.

## Decision

- **Build.** `vite.config.ts` adds `@rolldown/plugin-babel` with `reactCompilerPreset()`
  (`babel-plugin-react-compiler` 1.0.0). It applies to `vite build` and tests; in dev it is off unless
  `SIRUS_DEV_REACT_COMPILER=1`. Measured on this repo, the dev transform of `SessionPane.tsx` goes from
  about 0.6 s to 1.6 s and `ConversationSidebar.tsx` from 0.6 s to 2.4 s, and a cold first file pays about
  5 s of Babel start-up, so dev keeps Oxc only.
- **Babel 7.** `@babel/core` is pinned to 7.29: with Babel 8 the compiler 1.0.0 rejects every destructured
  parameter with a default (`({ a = 1 })`, "Expected object property value to be an LVal"), which would
  skip most components without any message.
- **Test.** `tests/react-compiler-coverage.test.ts` (part of `node --test`) compiles each `.tsx` file and
  `use*.ts` hook under `src/` with the compiler's logger and fails on any `CompileError`. Files that
  cannot be made safe cheaply are on a commented allowlist grouped by reason (try/finally, render-time
  refs, `eslint-disable` of the hooks rules, memoization it cannot preserve, render-time counters,
  unsupported syntax). An allowlisted file that starts compiling fails the test, so the list only shrinks.
- **Fixes.** Counters mutated while rendering became derived values (`SearchText`, `FileSearch`,
  `SubagentTeam`), a captured reassigned variable became a field (`ConversationHoverCard`), and
  `useStore.getState()` read during render moved into a module-level helper (`ScriptRunNotice`,
  `UsageLimitNotice`, `MoveSessionDialog`). No component was rewritten.
- **Left alone.** Existing `memo`, `useMemo` and `useCallback` stay: they are harmless with the compiler,
  and removing them is not trivial where components are on the allowlist.

## Consequences

- Bundle JavaScript grows about 1.9% (+0.63 MB of 34.2 MB; the compiler's cache slots) in exchange for fewer re-renders.
- A skipped function behaves exactly as before; only its automatic memoization is missing.
- New code that bails out fails `npm test` until it is fixed or allowlisted with a reason.

Credit: the coverage-test idea is another client's; the build setup (compiled builds, opt-in dev) is another client's.
