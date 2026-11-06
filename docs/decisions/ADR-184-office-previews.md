# ADR-184: Word, PowerPoint and Excel previews

**Status:** Accepted. Extends [ADR-039](ADR-039-attachment-document-reader.md). The approach and most of the worker code follow another client's Office previews.

## Context

The document reader (ADR-039) showed PDFs as pages, but Word and Excel files only as a typed text and table view, and PowerPoint not at all. Another open-source client (MIT, <https://github.com/pingdotgg/another open-source client>) previews `.docx`, `.xlsx` and `.pptx` by laying the files out with the Better Office engines (`@betteroffice/docx`, `@betteroffice/pptx`, `@betteroffice/xlsx`, `@betteroffice/fonts`; WebAssembly in a worker) and painting pages, slides and a spreadsheet grid on canvas. Project files and files an agent wrote also had no reader: the file pane only opened text and video.

## Decision

- **Engines.** `src/components/office/` is another client's `apps/web/src/components/files/office/` adapted to Sirus: the worker protocol (`worker-rpc.ts`), the Word, slide and sheet workers, the canvas painters and the selectable text layers are kept; the chrome (loading, notices, slide navigation, sheet tabs) uses Sirus tokens and primitives and EN/PT strings. Another open-source client adapted parts of it from opencode (MIT); both notices are kept. Licences: the three engines are Apache-2.0, `@betteroffice/fonts` is Apache-2.0 with SIL OFL 1.1 faces, `effect` (used by the workers) is MIT. Texts are in `public/licenses/office/` and `THIRD-PARTY-NOTICES.md`. Versions are pinned exactly to the ones another open-source client ships (docx 0.4.3, pptx 0.2.0, xlsx 0.3.0, fonts 0.4.0, effect 4.0.5).
- **Formats.** `.docx`, `.xlsx`, `.pptx`. The engines read Open XML only, so `.doc`, `.xls` and `.ppt` keep the existing conversion notice. Read-only: no editing, no recalculation, no macros, links open only through the host's `open_external_url` (http, https and mailto), as chat links do.
- **Lazy loading.** `OfficeReader` loads each viewer with `lazy()`, and each viewer starts its own module worker, so the engines, their WebAssembly and the fonts are separate chunks fetched only when an Office file is opened. The main bundle gains only `OfficeReader` and a few helpers.
- **One bounded read path.** Nothing new reads unbounded data. `attachment_preview` (owner-bound snapshot, 10 MiB) and the new `workspace_document` (session id plus a path inside its workspace; `open_regular_within`, reserved folders refused, the same 10 MiB bound and single reader permit) both end in the same `parse_bytes`. For DOCX/XLSX/PPTX it runs the existing archive preflight (`Office::new`: no ZIP64/multi-disk, 512 entries, 512 KiB central directory, 8 MiB per part, 32 MiB declared expanded size, no escaping names) and returns `{ type: "office", format, data }`, the bounded bytes in base64 like a PDF. The front end re-checks the size before decoding (`documentBytes`) and the engines bound pages (1,000), slides (500) and image pixels.
- **Text view fallback.** `plain: true` asks the same commands for the ADR-039 typed view of a Word or Excel file. The reader offers it ("Show text view") when the engine cannot draw a file, for example on a browser without worker canvases. PowerPoint has none.
- **Where it opens.** Attachment chips and sent attachments (the modal and the old dock pane) through `DocumentBody`; project files and agent-written files in the file pane (`EditorPane` routes `.pdf`, `.docx`, `.xlsx`, `.pptx` to `WorkspaceDocument` before any text read, so large files are not refused by the 2 MiB editor limit); and the phone's file browser (`MobileFiles`, `.is-document`).
- **Errors.** Same pattern as the PDF reader: a 30 second open timeout, an alert with the reason (limit, damaged, timeout, busy, unavailable), Try again.
- **Themes.** The surrounding chrome follows the light and dark tokens; pages, slides and the grid are drawn as the document is (white paper), as the PDF pages are.
- **Remote page.** The phone page's CSP gains `'wasm-unsafe-eval'` in `script-src` (the desktop CSP already had it, ADR-170), which the engines' WebAssembly needs.
- `tsconfig.json` targets the `ES2023` library (`toSorted` in the ported code; all supported WebViews have it).

## Consequences

- **Positive:** faithful pages, slides and cells (fonts, images, tables, merged cells, sheets) instead of extracted text; PowerPoint support; documents in the workspace are readable beside the conversation and on the phone.
- **Negative:** the engines add several megabytes of lazy assets (see the bundle report in the change description) and public licence texts; a deck whose archive has more than 512 entries or a part over 8 MiB is refused as over the reader's limits; no search, no text selection across canvas pages beyond the invisible text layer, no printing.
- **Accepted trade-off:** a Rust-side archive preflight duplicates the engines' own checks, so a hostile archive is rejected before bytes reach a worker.

## Alternatives considered

- Converting on the Mac (LibreOffice, Quick Look): external processes and path authority; not portable to the phone.
- A new streaming protocol for large files (another open-source client reads up to 50 MB over HTTP): the existing 10 MiB bounded read already covers the attachment and workspace paths.
- Dropping the typed view: kept as the fallback because it needs no worker, canvas or WebAssembly.

## References

- Another open-source client `apps/web/src/components/files/` (`OfficeViewer`, `office/`) and `packages/shared/src/filePreview.ts`.
- <https://github.com/openooxml/betteroffice> (Apache-2.0).
