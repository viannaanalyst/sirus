# Provider branding and catalog expansion

Goal: show original, transparent vendor marks throughout Sirus and support nine CLI providers.

1. Bundle glyphs from first-party brand assets. Record provenance and transformations; remove favicon tiles without redrawing marks. Keep monochrome marks legible in both themes.
2. Resolve model families independently of the host CLI, including OpenCode's DeepSeek, Kimi, GLM, Qwen and other recognized upstream families. Test qualified IDs and unknown families.
3. Extend shared provider IDs, settings, detection, diagnostics and registry with Antigravity (`agy`), Factory Droid (`droid`), Pi (`pi`) and Devin (`devin`). Preserve existing sessions and defaults.
4. Implement fixed, documented headless argv, bounded output normalization and real CLI catalog discovery. Preserve process ownership, cancellation and bounded fallback conversation context. Do not add IPC, arbitrary flags, credential reads, automatic login or permission bypasses. Expose only supported execution modes.
5. Inspect official installers and install missing CLIs without login. Verify help/version/catalog using disposable working directories and bounded probes. Authentication-dependent inference requires the user's existing vendor login; report that boundary accurately.
6. Run frontend checks and build, native fmt/check/clippy/tests, a security review and incremental Graphify. Update provider/runtime docs and the desktop bundle.

Validation targets: background-free SVGs, theme contrast, family classification, all nine IDs in both languages, option-like prompt isolation, unsupported permission rejection, tool metadata suppression, duplicate stream suppression and protocol-error propagation.
