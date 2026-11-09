//! Ghostty's terminal core, `libghostty-vt`, behind the `TerminalEngine` trait (ADR-199).
//!
//! The library is built by `scripts/build-libghostty-vt.sh` at the revision pinned in
//! `vendor/libghostty-vt/VERSION` and linked by `build.rs` when the `ghostty` cargo feature is
//! on. Only the C ABI's subset Sirus needs is declared here, by hand, from the pinned headers in
//! `vendor/libghostty-vt/include/ghostty/vt/` (the same revision T3 Code ships).
//!
//! Gaps of the ABI at this revision, kept on the shared path or left out:
//! - OSC 52 clipboard writes and reads have no callback, so programs cannot reach the clipboard.
//! - Colours a program sets with OSC 4 do not reach the drawing: cells name palette indices and
//!   the panel paints them with Sirus's palette (queries are still answered with it).
//! - Selection is kept by Sirus in screen coordinates and handed to the terminal on every change;
//!   once the scrollback is full and trims its oldest lines, a held selection can drift.
//! - Keys and the mouse are encoded by the panel (`key_bytes`), not by Ghostty's encoders.
#![allow(non_camel_case_types, dead_code)]

use crate::terminal_engine::{
    Cell, CellColor, CellFlags, Cursor, EngineKind, Modes, ScrollTo, SelectKind, Side, Snapshot,
    TerminalEngine, ViewPoint,
};
use std::ffi::c_void;

mod ffi {
    use std::ffi::c_void;

    pub type GhosttyResult = i32;
    pub const SUCCESS: GhosttyResult = 0;
    pub const OUT_OF_SPACE: GhosttyResult = -3;

    pub type Terminal = *mut c_void;
    pub type RenderState = *mut c_void;
    pub type RowIterator = *mut c_void;
    pub type RowCells = *mut c_void;
    pub type GhosttyCell = u64;
    pub type GhosttyRow = u64;

    #[repr(C)]
    pub struct TerminalOptions {
        pub cols: u16,
        pub rows: u16,
        pub max_scrollback: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union ScrollValue {
        pub delta: isize,
        pub padding: [u64; 2],
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct ScrollViewport {
        pub tag: i32,
        pub value: ScrollValue,
    }
    pub const SCROLL_BOTTOM: i32 = 1;
    pub const SCROLL_DELTA: i32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    pub struct PointCoordinate {
        pub x: u16,
        pub y: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union PointValue {
        pub coordinate: PointCoordinate,
        pub padding: [u64; 2],
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Point {
        pub tag: i32,
        pub value: PointValue,
    }
    pub const POINT_VIEWPORT: i32 = 1;
    pub const POINT_SCREEN: i32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct GridRef {
        pub size: usize,
        pub node: *mut c_void,
        pub x: u16,
        pub y: u16,
    }
    impl Default for GridRef {
        fn default() -> Self {
            Self {
                size: std::mem::size_of::<Self>(),
                node: std::ptr::null_mut(),
                x: 0,
                y: 0,
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Selection {
        pub size: usize,
        pub start: GridRef,
        pub end: GridRef,
        pub rectangle: bool,
    }
    impl Default for Selection {
        fn default() -> Self {
            Self {
                size: std::mem::size_of::<Self>(),
                start: GridRef::default(),
                end: GridRef::default(),
                rectangle: false,
            }
        }
    }

    #[repr(C)]
    pub struct SelectWordOptions {
        pub size: usize,
        pub grid_ref: GridRef,
        pub boundary_codepoints: *const u32,
        pub boundary_codepoints_len: usize,
    }
    #[repr(C)]
    pub struct SelectLineOptions {
        pub size: usize,
        pub grid_ref: GridRef,
        pub whitespace: *const u32,
        pub whitespace_len: usize,
        pub semantic_prompt_boundary: bool,
    }
    #[repr(C)]
    pub struct SelectionFormatOptions {
        pub size: usize,
        pub emit: i32,
        pub unwrap: bool,
        pub trim: bool,
        pub selection: *const Selection,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
    pub struct Rgb {
        pub r: u8,
        pub g: u8,
        pub b: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub union StyleColorValue {
        pub palette: u8,
        pub rgb: Rgb,
        pub padding: u64,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct StyleColor {
        pub tag: i32,
        pub value: StyleColorValue,
    }
    pub const STYLE_COLOR_PALETTE: i32 = 1;
    pub const STYLE_COLOR_RGB: i32 = 2;

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct Style {
        pub size: usize,
        pub fg_color: StyleColor,
        pub bg_color: StyleColor,
        pub underline_color: StyleColor,
        pub bold: bool,
        pub italic: bool,
        pub faint: bool,
        pub blink: bool,
        pub inverse: bool,
        pub invisible: bool,
        pub strikethrough: bool,
        pub overline: bool,
        pub underline: i32,
    }
    impl Default for Style {
        fn default() -> Self {
            let none = StyleColor {
                tag: 0,
                value: StyleColorValue { padding: 0 },
            };
            Self {
                size: std::mem::size_of::<Self>(),
                fg_color: none,
                bg_color: none,
                underline_color: none,
                bold: false,
                italic: false,
                faint: false,
                blink: false,
                inverse: false,
                invisible: false,
                strikethrough: false,
                overline: false,
                underline: 0,
            }
        }
    }

    #[repr(C)]
    pub struct GhosttyString {
        pub ptr: *const u8,
        pub len: usize,
    }

    #[repr(C)]
    pub struct DeviceAttributes {
        pub conformance_level: u16,
        pub features: [u16; 64],
        pub num_features: usize,
        pub device_type: u16,
        pub firmware_version: u16,
        pub rom_cartridge: u16,
        pub unit_id: u32,
    }

    #[repr(C)]
    pub struct SizeReportSize {
        pub rows: u16,
        pub columns: u16,
        pub cell_width: u32,
        pub cell_height: u32,
    }

    pub type WritePtyFn = extern "C" fn(Terminal, *mut c_void, *const u8, usize);
    pub type TitleChangedFn = extern "C" fn(Terminal, *mut c_void);
    pub type SizeFn = extern "C" fn(Terminal, *mut c_void, *mut SizeReportSize) -> bool;
    pub type ColorSchemeFn = extern "C" fn(Terminal, *mut c_void, *mut i32) -> bool;
    pub type DeviceAttributesFn =
        extern "C" fn(Terminal, *mut c_void, *mut DeviceAttributes) -> bool;

    // GhosttyTerminalOption
    pub const OPT_USERDATA: i32 = 0;
    pub const OPT_WRITE_PTY: i32 = 1;
    pub const OPT_TITLE_CHANGED: i32 = 5;
    pub const OPT_SIZE: i32 = 6;
    pub const OPT_COLOR_SCHEME: i32 = 7;
    pub const OPT_DEVICE_ATTRIBUTES: i32 = 8;
    pub const OPT_COLOR_FOREGROUND: i32 = 11;
    pub const OPT_COLOR_BACKGROUND: i32 = 12;
    pub const OPT_COLOR_CURSOR: i32 = 13;
    pub const OPT_COLOR_PALETTE: i32 = 14;
    pub const OPT_SELECTION: i32 = 21;

    // GhosttyTerminalData
    pub const DATA_ACTIVE_SCREEN: i32 = 6;
    pub const DATA_MOUSE_TRACKING: i32 = 11;
    pub const DATA_TITLE: i32 = 12;

    // GhosttyRenderStateData
    pub const RS_ROW_ITERATOR: i32 = 4;
    pub const RS_CURSOR_VISIBLE: i32 = 11;
    pub const RS_CURSOR_VIEWPORT_HAS_VALUE: i32 = 14;
    pub const RS_CURSOR_VIEWPORT_X: i32 = 15;
    pub const RS_CURSOR_VIEWPORT_Y: i32 = 16;
    // GhosttyRenderStateRowData
    pub const ROW_RAW: i32 = 2;
    pub const ROW_CELLS: i32 = 3;
    // GhosttyRenderStateRowCellsData
    pub const CELLS_RAW: i32 = 1;
    pub const CELLS_STYLE: i32 = 2;
    pub const CELLS_GRAPHEMES_LEN: i32 = 3;
    pub const CELLS_GRAPHEMES_BUF: i32 = 4;
    pub const CELLS_SELECTED: i32 = 7;
    // GhosttyCellData
    pub const CELL_CODEPOINT: i32 = 1;
    pub const CELL_CONTENT_TAG: i32 = 2;
    pub const CELL_WIDE: i32 = 3;
    pub const CELL_COLOR_PALETTE: i32 = 10;
    pub const CELL_COLOR_RGB: i32 = 11;
    pub const CONTENT_BG_PALETTE: i32 = 2;
    pub const CONTENT_BG_RGB: i32 = 3;
    pub const WIDE_WIDE: i32 = 1;
    pub const WIDE_SPACER_TAIL: i32 = 2;
    pub const WIDE_SPACER_HEAD: i32 = 3;
    // GhosttyRowData
    pub const ROW_WRAP: i32 = 1;

    #[link(name = "ghostty-vt", kind = "static")]
    unsafe extern "C" {
        pub fn ghostty_terminal_new(
            allocator: *const c_void,
            terminal: *mut Terminal,
            options: TerminalOptions,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_free(terminal: Terminal);
        pub fn ghostty_terminal_resize(
            terminal: Terminal,
            cols: u16,
            rows: u16,
            cell_width_px: u32,
            cell_height_px: u32,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_set(
            terminal: Terminal,
            option: i32,
            value: *const c_void,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_vt_write(terminal: Terminal, data: *const u8, len: usize);
        pub fn ghostty_terminal_scroll_viewport(terminal: Terminal, behavior: ScrollViewport);
        pub fn ghostty_terminal_mode_get(
            terminal: Terminal,
            mode: u16,
            out_value: *mut bool,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_get(
            terminal: Terminal,
            data: i32,
            out: *mut c_void,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_grid_ref(
            terminal: Terminal,
            point: Point,
            out_ref: *mut GridRef,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_point_from_grid_ref(
            terminal: Terminal,
            grid_ref: *const GridRef,
            tag: i32,
            out: *mut PointCoordinate,
        ) -> GhosttyResult;

        pub fn ghostty_grid_ref_row(
            grid_ref: *const GridRef,
            out_row: *mut GhosttyRow,
        ) -> GhosttyResult;
        pub fn ghostty_grid_ref_graphemes(
            grid_ref: *const GridRef,
            buf: *mut u32,
            buf_len: usize,
            out_len: *mut usize,
        ) -> GhosttyResult;
        pub fn ghostty_cell_get(cell: GhosttyCell, data: i32, out: *mut c_void) -> GhosttyResult;
        pub fn ghostty_row_get(row: GhosttyRow, data: i32, out: *mut c_void) -> GhosttyResult;

        pub fn ghostty_terminal_select_word(
            terminal: Terminal,
            options: *const SelectWordOptions,
            out: *mut Selection,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_select_line(
            terminal: Terminal,
            options: *const SelectLineOptions,
            out: *mut Selection,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_select_all(
            terminal: Terminal,
            out: *mut Selection,
        ) -> GhosttyResult;
        pub fn ghostty_terminal_selection_format_buf(
            terminal: Terminal,
            options: SelectionFormatOptions,
            buf: *mut u8,
            buf_len: usize,
            out_written: *mut usize,
        ) -> GhosttyResult;

        pub fn ghostty_render_state_new(
            allocator: *const c_void,
            state: *mut RenderState,
        ) -> GhosttyResult;
        pub fn ghostty_render_state_free(state: RenderState);
        pub fn ghostty_render_state_update(state: RenderState, terminal: Terminal)
        -> GhosttyResult;
        pub fn ghostty_render_state_get(
            state: RenderState,
            data: i32,
            out: *mut c_void,
        ) -> GhosttyResult;
        pub fn ghostty_render_state_row_iterator_new(
            allocator: *const c_void,
            out: *mut RowIterator,
        ) -> GhosttyResult;
        pub fn ghostty_render_state_row_iterator_free(iterator: RowIterator);
        pub fn ghostty_render_state_row_iterator_next(iterator: RowIterator) -> bool;
        pub fn ghostty_render_state_row_get(
            iterator: RowIterator,
            data: i32,
            out: *mut c_void,
        ) -> GhosttyResult;
        pub fn ghostty_render_state_row_cells_new(
            allocator: *const c_void,
            out: *mut RowCells,
        ) -> GhosttyResult;
        pub fn ghostty_render_state_row_cells_free(cells: RowCells);
        pub fn ghostty_render_state_row_cells_next(cells: RowCells) -> bool;
        pub fn ghostty_render_state_row_cells_get(
            cells: RowCells,
            data: i32,
            out: *mut c_void,
        ) -> GhosttyResult;
    }

    /// `ghostty_mode_new` (a `static inline` in modes.h).
    pub const fn mode(value: u16, ansi: bool) -> u16 {
        (value & 0x7fff) | ((ansi as u16) << 15)
    }
    pub const MODE_DECCKM: u16 = mode(1, false);
    pub const MODE_CURSOR_VISIBLE: u16 = mode(25, false);
    pub const MODE_ALT_SCROLL: u16 = mode(1007, false);
    pub const MODE_BRACKETED_PASTE: u16 = mode(2004, false);
}

/// The colours the terminal answers OSC 4/10/11/12 queries with, from Sirus's theme.
#[derive(Clone)]
pub struct ThemeColors {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub palette: [u32; 256],
    pub dark: bool,
}

impl ThemeColors {
    pub fn current() -> Self {
        let (foreground, background, cursor) = crate::panel_terminal::default_colors();
        let mut palette = [0; 256];
        for (index, slot) in palette.iter_mut().enumerate() {
            *slot = crate::panel_terminal::indexed_rgb(index as u8);
        }
        Self {
            foreground,
            background,
            cursor,
            palette,
            dark: !crate::theme::is_light(),
        }
    }
}

fn rgb(value: u32) -> ffi::Rgb {
    ffi::Rgb {
        r: (value >> 16) as u8,
        g: (value >> 8) as u8,
        b: value as u8,
    }
}

fn hex(c: ffi::Rgb) -> u32 {
    ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
}

/// What the callbacks write into; boxed so its address stays put for the library.
struct Shared {
    replies: Vec<u8>,
    columns: u16,
    lines: u16,
    cell_width: u32,
    cell_height: u32,
    dark: bool,
}

extern "C" fn on_write_pty(_: ffi::Terminal, user: *mut c_void, data: *const u8, len: usize) {
    if user.is_null() || data.is_null() {
        return;
    }
    let shared = unsafe { &mut *(user as *mut Shared) };
    shared
        .replies
        .extend_from_slice(unsafe { std::slice::from_raw_parts(data, len) });
}

extern "C" fn on_title_changed(_: ffi::Terminal, _: *mut c_void) {}

extern "C" fn on_size(_: ffi::Terminal, user: *mut c_void, out: *mut ffi::SizeReportSize) -> bool {
    if user.is_null() || out.is_null() {
        return false;
    }
    let shared = unsafe { &*(user as *const Shared) };
    unsafe {
        *out = ffi::SizeReportSize {
            rows: shared.lines,
            columns: shared.columns,
            cell_width: shared.cell_width,
            cell_height: shared.cell_height,
        };
    }
    true
}

extern "C" fn on_color_scheme(_: ffi::Terminal, user: *mut c_void, out: *mut i32) -> bool {
    if user.is_null() || out.is_null() {
        return false;
    }
    let shared = unsafe { &*(user as *const Shared) };
    unsafe { *out = shared.dark as i32 };
    true
}

extern "C" fn on_device_attributes(
    _: ffi::Terminal,
    _: *mut c_void,
    out: *mut ffi::DeviceAttributes,
) -> bool {
    if out.is_null() {
        return false;
    }
    // A VT220 with ANSI colour, the same family alacritty reports.
    let mut features = [0u16; 64];
    features[0] = 22;
    unsafe {
        *out = ffi::DeviceAttributes {
            conformance_level: 62,
            features,
            num_features: 1,
            device_type: 1,
            firmware_version: 10,
            rom_cartridge: 0,
            unit_id: 0,
        };
    }
    true
}

/// A selection in screen coordinates (history and active area together): where it began, and
/// how it grows.
#[derive(Clone, Copy, Debug)]
enum Held {
    /// Cell by cell from `anchor` (with the side of the cell it began on).
    Simple {
        anchor: ffi::PointCoordinate,
        anchor_side: Side,
        head: ffi::PointCoordinate,
        head_side: Side,
    },
    /// Whole words or lines: the unit around the anchor joined with the one around the head.
    Units {
        kind: SelectKind,
        first: ffi::PointCoordinate,
        last: ffi::PointCoordinate,
        anchor_first: ffi::PointCoordinate,
        anchor_last: ffi::PointCoordinate,
    },
}

pub struct GhosttyEngine {
    terminal: ffi::Terminal,
    render: ffi::RenderState,
    rows: ffi::RowIterator,
    cells: ffi::RowCells,
    shared: Box<Shared>,
    held: Option<Held>,
}

// The handles are only used behind the engine's mutex, one thread at a time.
unsafe impl Send for GhosttyEngine {}

impl Drop for GhosttyEngine {
    fn drop(&mut self) {
        unsafe {
            if !self.cells.is_null() {
                ffi::ghostty_render_state_row_cells_free(self.cells);
            }
            if !self.rows.is_null() {
                ffi::ghostty_render_state_row_iterator_free(self.rows);
            }
            if !self.render.is_null() {
                ffi::ghostty_render_state_free(self.render);
            }
            if !self.terminal.is_null() {
                ffi::ghostty_terminal_free(self.terminal);
            }
        }
    }
}

fn check(what: &str, result: ffi::GhosttyResult) -> Result<(), String> {
    if result == ffi::SUCCESS {
        Ok(())
    } else {
        Err(format!("{what} failed ({result})"))
    }
}

fn before(a: ffi::PointCoordinate, b: ffi::PointCoordinate) -> bool {
    (a.y, a.x) < (b.y, b.x)
}

impl GhosttyEngine {
    pub fn new(
        columns: usize,
        lines: usize,
        scrollback: usize,
        theme: ThemeColors,
    ) -> Result<Self, String> {
        let columns = columns.clamp(1, u16::MAX as usize) as u16;
        let lines = lines.clamp(1, u16::MAX as usize) as u16;
        let mut engine = Self {
            terminal: std::ptr::null_mut(),
            render: std::ptr::null_mut(),
            rows: std::ptr::null_mut(),
            cells: std::ptr::null_mut(),
            shared: Box::new(Shared {
                replies: Vec::new(),
                columns,
                lines,
                cell_width: 8,
                cell_height: 16,
                dark: theme.dark,
            }),
            held: None,
        };
        unsafe {
            check(
                "ghostty_terminal_new",
                ffi::ghostty_terminal_new(
                    std::ptr::null(),
                    &mut engine.terminal,
                    ffi::TerminalOptions {
                        cols: columns,
                        rows: lines,
                        max_scrollback: scrollback,
                    },
                ),
            )?;
            check(
                "ghostty_render_state_new",
                ffi::ghostty_render_state_new(std::ptr::null(), &mut engine.render),
            )?;
            check(
                "ghostty_render_state_row_iterator_new",
                ffi::ghostty_render_state_row_iterator_new(std::ptr::null(), &mut engine.rows),
            )?;
            check(
                "ghostty_render_state_row_cells_new",
                ffi::ghostty_render_state_row_cells_new(std::ptr::null(), &mut engine.cells),
            )?;
            let t = engine.terminal;
            let user = &mut *engine.shared as *mut Shared as *const c_void;
            ffi::ghostty_terminal_set(t, ffi::OPT_USERDATA, user);
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_WRITE_PTY,
                on_write_pty as ffi::WritePtyFn as *const c_void,
            );
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_TITLE_CHANGED,
                on_title_changed as ffi::TitleChangedFn as *const c_void,
            );
            ffi::ghostty_terminal_set(t, ffi::OPT_SIZE, on_size as ffi::SizeFn as *const c_void);
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_COLOR_SCHEME,
                on_color_scheme as ffi::ColorSchemeFn as *const c_void,
            );
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_DEVICE_ATTRIBUTES,
                on_device_attributes as ffi::DeviceAttributesFn as *const c_void,
            );
            let (fg, bg, cursor) = (
                rgb(theme.foreground),
                rgb(theme.background),
                rgb(theme.cursor),
            );
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_COLOR_FOREGROUND,
                (&fg as *const ffi::Rgb).cast(),
            );
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_COLOR_BACKGROUND,
                (&bg as *const ffi::Rgb).cast(),
            );
            ffi::ghostty_terminal_set(
                t,
                ffi::OPT_COLOR_CURSOR,
                (&cursor as *const ffi::Rgb).cast(),
            );
            let palette: Vec<ffi::Rgb> = theme.palette.iter().map(|c| rgb(*c)).collect();
            ffi::ghostty_terminal_set(t, ffi::OPT_COLOR_PALETTE, palette.as_ptr().cast());
        }
        Ok(engine)
    }

    fn mode(&self, mode: u16) -> bool {
        let mut on = false;
        unsafe { ffi::ghostty_terminal_mode_get(self.terminal, mode, &mut on) };
        on
    }

    fn grid_ref(&self, tag: i32, x: u16, y: u32) -> Option<ffi::GridRef> {
        let mut out = ffi::GridRef::default();
        let point = ffi::Point {
            tag,
            value: ffi::PointValue {
                coordinate: ffi::PointCoordinate { x, y },
            },
        };
        let result = unsafe { ffi::ghostty_terminal_grid_ref(self.terminal, point, &mut out) };
        (result == ffi::SUCCESS).then_some(out)
    }

    fn screen_of_ref(&self, grid_ref: &ffi::GridRef) -> Option<ffi::PointCoordinate> {
        let mut out = ffi::PointCoordinate::default();
        let result = unsafe {
            ffi::ghostty_terminal_point_from_grid_ref(
                self.terminal,
                grid_ref,
                ffi::POINT_SCREEN,
                &mut out,
            )
        };
        (result == ffi::SUCCESS).then_some(out)
    }

    /// The screen coordinate of a viewport cell.
    fn screen_point(&self, at: ViewPoint) -> Option<ffi::PointCoordinate> {
        let column = at
            .column
            .min(self.shared.columns.saturating_sub(1) as usize) as u16;
        let r = self.grid_ref(ffi::POINT_VIEWPORT, column, at.line as u32)?;
        self.screen_of_ref(&r)
    }

    fn selection_bounds(
        &self,
        selection: &ffi::Selection,
    ) -> Option<(ffi::PointCoordinate, ffi::PointCoordinate)> {
        let a = self.screen_of_ref(&selection.start)?;
        let b = self.screen_of_ref(&selection.end)?;
        Some(if before(b, a) { (b, a) } else { (a, b) })
    }

    /// The word or line around a screen point, in screen coordinates.
    fn unit_at(
        &self,
        kind: SelectKind,
        at: ffi::PointCoordinate,
    ) -> Option<(ffi::PointCoordinate, ffi::PointCoordinate)> {
        let grid_ref = self.grid_ref(ffi::POINT_SCREEN, at.x, at.y)?;
        let mut out = ffi::Selection::default();
        let result = unsafe {
            match kind {
                SelectKind::Line => {
                    let options = ffi::SelectLineOptions {
                        size: std::mem::size_of::<ffi::SelectLineOptions>(),
                        grid_ref,
                        whitespace: std::ptr::null(),
                        whitespace_len: 0,
                        semantic_prompt_boundary: false,
                    };
                    ffi::ghostty_terminal_select_line(self.terminal, &options, &mut out)
                }
                _ => {
                    let options = ffi::SelectWordOptions {
                        size: std::mem::size_of::<ffi::SelectWordOptions>(),
                        grid_ref,
                        boundary_codepoints: std::ptr::null(),
                        boundary_codepoints_len: 0,
                    };
                    ffi::ghostty_terminal_select_word(self.terminal, &options, &mut out)
                }
            }
        };
        if result != ffi::SUCCESS {
            // A click on a blank cell selects just that cell.
            return Some((at, at));
        }
        self.selection_bounds(&out)
    }

    fn next_cell(&self, p: ffi::PointCoordinate) -> ffi::PointCoordinate {
        if p.x + 1 < self.shared.columns {
            ffi::PointCoordinate { x: p.x + 1, y: p.y }
        } else {
            ffi::PointCoordinate { x: 0, y: p.y + 1 }
        }
    }

    fn previous_cell(&self, p: ffi::PointCoordinate) -> Option<ffi::PointCoordinate> {
        if p.x > 0 {
            Some(ffi::PointCoordinate { x: p.x - 1, y: p.y })
        } else if p.y > 0 {
            Some(ffi::PointCoordinate {
                x: self.shared.columns.saturating_sub(1),
                y: p.y - 1,
            })
        } else {
            None
        }
    }

    /// The cells a held selection covers, first and last, or None when it covers none.
    fn covered(&self, held: &Held) -> Option<(ffi::PointCoordinate, ffi::PointCoordinate)> {
        match *held {
            Held::Simple {
                anchor,
                anchor_side,
                head,
                head_side,
            } => {
                let (mut first, first_side, mut last, last_side) = if before(head, anchor) {
                    (head, head_side, anchor, anchor_side)
                } else {
                    (anchor, anchor_side, head, head_side)
                };
                if first_side == Side::Right {
                    first = self.next_cell(first);
                }
                if last_side == Side::Left {
                    last = self.previous_cell(last)?;
                }
                (!before(last, first)).then_some((first, last))
            }
            Held::Units { first, last, .. } => Some((first, last)),
        }
    }

    /// Hands the held selection to the terminal, which marks the cells for the render state.
    fn apply_selection(&mut self) {
        let covered = self.held.as_ref().and_then(|held| self.covered(held));
        let selection = covered.and_then(|(first, last)| {
            Some(ffi::Selection {
                start: self.grid_ref(ffi::POINT_SCREEN, first.x, first.y)?,
                end: self.grid_ref(ffi::POINT_SCREEN, last.x, last.y)?,
                ..ffi::Selection::default()
            })
        });
        unsafe {
            match &selection {
                Some(selection) => ffi::ghostty_terminal_set(
                    self.terminal,
                    ffi::OPT_SELECTION,
                    (selection as *const ffi::Selection).cast(),
                ),
                None => {
                    ffi::ghostty_terminal_set(self.terminal, ffi::OPT_SELECTION, std::ptr::null())
                }
            };
        }
    }

    /// The text of a screen row and whether it wraps into the next.
    fn row_text(&self, y: u32) -> Option<(Vec<char>, bool)> {
        let start = self.grid_ref(ffi::POINT_SCREEN, 0, y)?;
        let mut row: ffi::GhosttyRow = 0;
        let mut wraps = false;
        unsafe {
            if ffi::ghostty_grid_ref_row(&start, &mut row) == ffi::SUCCESS {
                ffi::ghostty_row_get(row, ffi::ROW_WRAP, (&mut wraps as *mut bool).cast());
            }
        }
        let mut text = Vec::with_capacity(self.shared.columns as usize);
        for x in 0..self.shared.columns {
            let ch = self
                .grid_ref(ffi::POINT_SCREEN, x, y)
                .and_then(|r| {
                    let mut buf = [0u32; 16];
                    let mut len = 0usize;
                    let result = unsafe {
                        ffi::ghostty_grid_ref_graphemes(&r, buf.as_mut_ptr(), buf.len(), &mut len)
                    };
                    (result == ffi::SUCCESS && len > 0)
                        .then(|| char::from_u32(buf[0]))
                        .flatten()
                })
                .unwrap_or(' ');
            text.push(ch);
        }
        Some((text, wraps))
    }

    fn style_color(color: ffi::StyleColor) -> Option<CellColor> {
        unsafe {
            match color.tag {
                ffi::STYLE_COLOR_PALETTE => Some(CellColor::Indexed(color.value.palette)),
                ffi::STYLE_COLOR_RGB => Some(CellColor::Rgb(hex(color.value.rgb))),
                _ => None,
            }
        }
    }
}

impl TerminalEngine for GhosttyEngine {
    fn kind(&self) -> EngineKind {
        EngineKind::Ghostty
    }

    fn feed(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        unsafe { ffi::ghostty_terminal_vt_write(self.terminal, bytes.as_ptr(), bytes.len()) };
    }

    fn resize(&mut self, columns: usize, lines: usize, cell_width: u32, cell_height: u32) {
        let columns = columns.clamp(1, u16::MAX as usize) as u16;
        let lines = lines.clamp(1, u16::MAX as usize) as u16;
        let (cell_width, cell_height) = (cell_width.max(1), cell_height.max(1));
        let shared = &mut self.shared;
        if (
            shared.columns,
            shared.lines,
            shared.cell_width,
            shared.cell_height,
        ) == (columns, lines, cell_width, cell_height)
        {
            return;
        }
        shared.columns = columns;
        shared.lines = lines;
        shared.cell_width = cell_width;
        shared.cell_height = cell_height;
        unsafe {
            ffi::ghostty_terminal_resize(self.terminal, columns, lines, cell_width, cell_height)
        };
        // Reflow moves the cells under a held selection.
        self.held = None;
        unsafe { ffi::ghostty_terminal_set(self.terminal, ffi::OPT_SELECTION, std::ptr::null()) };
    }

    fn snapshot(&mut self) -> Snapshot {
        let mut lines = Vec::with_capacity(self.shared.lines as usize);
        let mut cursor = Cursor::default();
        unsafe {
            if ffi::ghostty_render_state_update(self.render, self.terminal) != ffi::SUCCESS {
                return Snapshot::default();
            }
            let mut rows = self.rows;
            if ffi::ghostty_render_state_get(
                self.render,
                ffi::RS_ROW_ITERATOR,
                (&mut rows as *mut ffi::RowIterator).cast(),
            ) != ffi::SUCCESS
            {
                return Snapshot::default();
            }
            while ffi::ghostty_render_state_row_iterator_next(rows) {
                let mut row = Vec::new();
                let mut cells = self.cells;
                if ffi::ghostty_render_state_row_get(
                    rows,
                    ffi::ROW_CELLS,
                    (&mut cells as *mut ffi::RowCells).cast(),
                ) != ffi::SUCCESS
                {
                    lines.push(row);
                    continue;
                }
                let mut column = 0usize;
                while ffi::ghostty_render_state_row_cells_next(cells) {
                    let x = column;
                    column += 1;
                    let mut raw: ffi::GhosttyCell = 0;
                    ffi::ghostty_render_state_row_cells_get(
                        cells,
                        ffi::CELLS_RAW,
                        (&mut raw as *mut u64).cast(),
                    );
                    let mut wide: i32 = 0;
                    ffi::ghostty_cell_get(raw, ffi::CELL_WIDE, (&mut wide as *mut i32).cast());
                    if wide == ffi::WIDE_SPACER_TAIL || wide == ffi::WIDE_SPACER_HEAD {
                        continue;
                    }
                    let mut style = ffi::Style::default();
                    ffi::ghostty_render_state_row_cells_get(
                        cells,
                        ffi::CELLS_STYLE,
                        (&mut style as *mut ffi::Style).cast(),
                    );
                    let mut selected = false;
                    ffi::ghostty_render_state_row_cells_get(
                        cells,
                        ffi::CELLS_SELECTED,
                        (&mut selected as *mut bool).cast(),
                    );
                    let mut len: u32 = 0;
                    ffi::ghostty_render_state_row_cells_get(
                        cells,
                        ffi::CELLS_GRAPHEMES_LEN,
                        (&mut len as *mut u32).cast(),
                    );
                    let ch = if len == 0 {
                        ' '
                    } else {
                        let mut buf = vec![0u32; len as usize];
                        ffi::ghostty_render_state_row_cells_get(
                            cells,
                            ffi::CELLS_GRAPHEMES_BUF,
                            buf.as_mut_ptr().cast(),
                        );
                        char::from_u32(buf[0]).unwrap_or(' ')
                    };
                    // A cell erased with a colour keeps the colour in its content, not its style.
                    let mut tag: i32 = 0;
                    ffi::ghostty_cell_get(
                        raw,
                        ffi::CELL_CONTENT_TAG,
                        (&mut tag as *mut i32).cast(),
                    );
                    let bg = match tag {
                        ffi::CONTENT_BG_PALETTE => {
                            let mut index: u8 = 0;
                            ffi::ghostty_cell_get(
                                raw,
                                ffi::CELL_COLOR_PALETTE,
                                (&mut index as *mut u8).cast(),
                            );
                            Some(CellColor::Indexed(index))
                        }
                        ffi::CONTENT_BG_RGB => {
                            let mut c = ffi::Rgb::default();
                            ffi::ghostty_cell_get(
                                raw,
                                ffi::CELL_COLOR_RGB,
                                (&mut c as *mut ffi::Rgb).cast(),
                            );
                            Some(CellColor::Rgb(hex(c)))
                        }
                        _ => Self::style_color(style.bg_color),
                    };
                    let fg = Self::style_color(style.fg_color);
                    let blank = ch == ' '
                        && fg.is_none()
                        && bg.is_none()
                        && !style.inverse
                        && style.underline == 0
                        && !style.strikethrough;
                    if blank && !selected {
                        continue;
                    }
                    row.push(Cell {
                        column: x,
                        ch,
                        fg: fg.unwrap_or(CellColor::Foreground),
                        bg: bg.unwrap_or(CellColor::Background),
                        flags: CellFlags {
                            bold: style.bold,
                            italic: style.italic,
                            underline: style.underline != 0,
                            strike: style.strikethrough,
                            inverse: style.inverse,
                            dim: style.faint,
                            hidden: style.invisible,
                            wide: wide == ffi::WIDE_WIDE,
                        },
                        selected,
                    });
                }
                lines.push(row);
            }
            let mut has = false;
            ffi::ghostty_render_state_get(
                self.render,
                ffi::RS_CURSOR_VIEWPORT_HAS_VALUE,
                (&mut has as *mut bool).cast(),
            );
            let mut visible = false;
            ffi::ghostty_render_state_get(
                self.render,
                ffi::RS_CURSOR_VISIBLE,
                (&mut visible as *mut bool).cast(),
            );
            cursor.visible = visible;
            if has {
                let (mut x, mut y) = (0u16, 0u16);
                ffi::ghostty_render_state_get(
                    self.render,
                    ffi::RS_CURSOR_VIEWPORT_X,
                    (&mut x as *mut u16).cast(),
                );
                ffi::ghostty_render_state_get(
                    self.render,
                    ffi::RS_CURSOR_VIEWPORT_Y,
                    (&mut y as *mut u16).cast(),
                );
                cursor.at = Some((y as usize, x as usize));
            }
        }
        Snapshot { lines, cursor }
    }

    fn scroll(&mut self, to: ScrollTo) {
        let behavior = match to {
            ScrollTo::Bottom => ffi::ScrollViewport {
                tag: ffi::SCROLL_BOTTOM,
                value: ffi::ScrollValue { padding: [0; 2] },
            },
            // Sirus counts back into the history as positive; Ghostty counts up as negative.
            ScrollTo::Delta(lines) => ffi::ScrollViewport {
                tag: ffi::SCROLL_DELTA,
                value: ffi::ScrollValue {
                    delta: -(lines as isize),
                },
            },
        };
        unsafe { ffi::ghostty_terminal_scroll_viewport(self.terminal, behavior) };
    }

    fn modes(&self) -> Modes {
        let mut screen: i32 = 0;
        let mut mouse = false;
        unsafe {
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_ACTIVE_SCREEN,
                (&mut screen as *mut i32).cast(),
            );
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_MOUSE_TRACKING,
                (&mut mouse as *mut bool).cast(),
            );
        }
        Modes {
            app_cursor: self.mode(ffi::MODE_DECCKM),
            bracketed_paste: self.mode(ffi::MODE_BRACKETED_PASTE),
            alt_screen: screen == 1,
            alternate_scroll: self.mode(ffi::MODE_ALT_SCROLL),
            mouse_tracking: mouse,
            show_cursor: self.mode(ffi::MODE_CURSOR_VISIBLE),
        }
    }

    fn title(&self) -> Option<String> {
        let mut title = ffi::GhosttyString {
            ptr: std::ptr::null(),
            len: 0,
        };
        let result = unsafe {
            ffi::ghostty_terminal_get(
                self.terminal,
                ffi::DATA_TITLE,
                (&mut title as *mut ffi::GhosttyString).cast(),
            )
        };
        if result != ffi::SUCCESS || title.ptr.is_null() || title.len == 0 {
            return None;
        }
        let bytes = unsafe { std::slice::from_raw_parts(title.ptr, title.len) };
        Some(String::from_utf8_lossy(bytes).into_owned())
    }

    fn take_replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.shared.replies)
    }

    fn select_begin(&mut self, at: ViewPoint, side: Side, kind: SelectKind) {
        let Some(point) = self.screen_point(at) else {
            return;
        };
        self.held = match kind {
            SelectKind::Simple => Some(Held::Simple {
                anchor: point,
                anchor_side: side,
                head: point,
                head_side: side,
            }),
            unit => self.unit_at(unit, point).map(|(first, last)| Held::Units {
                kind: unit,
                first,
                last,
                anchor_first: first,
                anchor_last: last,
            }),
        };
        self.apply_selection();
    }

    fn select_extend(&mut self, at: ViewPoint, side: Side) {
        let Some(point) = self.screen_point(at) else {
            return;
        };
        let next = match self.held {
            Some(Held::Simple {
                anchor,
                anchor_side,
                ..
            }) => Some(Held::Simple {
                anchor,
                anchor_side,
                head: point,
                head_side: side,
            }),
            Some(Held::Units {
                kind,
                anchor_first,
                anchor_last,
                ..
            }) => {
                let (unit_first, unit_last) = self.unit_at(kind, point).unwrap_or((point, point));
                Some(Held::Units {
                    kind,
                    first: if before(unit_first, anchor_first) {
                        unit_first
                    } else {
                        anchor_first
                    },
                    last: if before(anchor_last, unit_last) {
                        unit_last
                    } else {
                        anchor_last
                    },
                    anchor_first,
                    anchor_last,
                })
            }
            None => None,
        };
        if next.is_some() {
            self.held = next;
            self.apply_selection();
        }
    }

    fn select_all(&mut self) {
        let mut out = ffi::Selection::default();
        if unsafe { ffi::ghostty_terminal_select_all(self.terminal, &mut out) } != ffi::SUCCESS {
            return;
        }
        if let Some((first, last)) = self.selection_bounds(&out) {
            self.held = Some(Held::Units {
                kind: SelectKind::Line,
                first,
                last,
                anchor_first: first,
                anchor_last: last,
            });
            self.apply_selection();
        }
    }

    fn clear_selection(&mut self) {
        if self.held.take().is_some() {
            self.apply_selection();
        }
    }

    fn has_selection(&self) -> bool {
        self.held.is_some()
    }

    fn selection_is_empty(&self) -> bool {
        self.held
            .as_ref()
            .is_some_and(|held| self.covered(held).is_none())
    }

    fn selection_text(&self) -> Option<String> {
        let held = self.held.as_ref()?;
        self.covered(held)?;
        let options = || ffi::SelectionFormatOptions {
            size: std::mem::size_of::<ffi::SelectionFormatOptions>(),
            emit: 0,
            unwrap: true,
            trim: true,
            selection: std::ptr::null(),
        };
        let mut needed = 0usize;
        let result = unsafe {
            ffi::ghostty_terminal_selection_format_buf(
                self.terminal,
                options(),
                std::ptr::null_mut(),
                0,
                &mut needed,
            )
        };
        if result == ffi::SUCCESS {
            return Some(String::new());
        }
        if result != ffi::OUT_OF_SPACE {
            return None;
        }
        let mut buf = vec![0u8; needed];
        let mut written = 0usize;
        let result = unsafe {
            ffi::ghostty_terminal_selection_format_buf(
                self.terminal,
                options(),
                buf.as_mut_ptr(),
                buf.len(),
                &mut written,
            )
        };
        if result != ffi::SUCCESS {
            return None;
        }
        buf.truncate(written);
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    fn wrapped_line_at(&self, at: ViewPoint) -> Option<(String, usize)> {
        let point = self.screen_point(at)?;
        let mut first = point.y;
        while first > 0 && self.row_text(first - 1).is_some_and(|(_, wraps)| wraps) {
            first -= 1;
        }
        let mut text = String::new();
        let mut under = None;
        let mut y = first;
        loop {
            let Some((row, wraps)) = self.row_text(y) else {
                break;
            };
            for (x, ch) in row.into_iter().enumerate() {
                if y == point.y && x == point.x as usize {
                    under = Some(text.len());
                }
                text.push(ch);
            }
            if !wraps {
                break;
            }
            y += 1;
        }
        Some((text, under?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> ThemeColors {
        let mut palette = [0u32; 256];
        for (i, slot) in palette.iter_mut().enumerate() {
            *slot = (i as u32) * 0x010101;
        }
        ThemeColors {
            foreground: 0xe8eef2,
            background: 0x171717,
            cursor: 0x60a5fa,
            palette,
            dark: true,
        }
    }

    fn engine(columns: usize, lines: usize) -> GhosttyEngine {
        GhosttyEngine::new(columns, lines, 1000, theme()).expect("libghostty-vt terminal")
    }

    #[test]
    fn ghostty_draws_text_and_colours() {
        let mut e = engine(20, 4);
        e.feed(b"hi \x1b[31mred\x1b[0m\r\n\x1b[1;38;2;1;2;3mx\x1b[0m \x1b[44mb\x1b[0m");
        let snap = e.snapshot();
        assert_eq!(snap.lines.len(), 4);
        assert_eq!(snap.line_text(0), "hi red");
        let r = snap.lines[0].iter().find(|c| c.ch == 'r').unwrap();
        assert_eq!(r.fg, CellColor::Indexed(1));
        let h = snap.lines[0].iter().find(|c| c.ch == 'h').unwrap();
        assert_eq!(h.fg, CellColor::Foreground);
        assert_eq!(h.bg, CellColor::Background);
        let x = &snap.lines[1][0];
        assert_eq!(x.ch, 'x');
        assert_eq!(x.fg, CellColor::Rgb(0x010203));
        assert!(x.flags.bold);
        let b = snap.lines[1].iter().find(|c| c.ch == 'b').unwrap();
        assert_eq!(b.bg, CellColor::Indexed(4));
    }

    #[test]
    fn ghostty_moves_the_cursor() {
        let mut e = engine(20, 5);
        e.feed(b"abc");
        assert_eq!(e.snapshot().cursor.at, Some((0, 3)));
        e.feed(b"\x1b[3;7H");
        let snap = e.snapshot();
        assert_eq!(snap.cursor.at, Some((2, 6)));
        assert!(snap.cursor.visible);
        e.feed(b"\x1b[?25l");
        assert!(!e.snapshot().cursor.visible);
        assert!(!e.modes().show_cursor);
        e.feed(b"\x1b[2A\x1b[4D*");
        assert_eq!(e.snapshot().line_text(0), "ab*");
    }

    #[test]
    fn ghostty_resizes_and_reflows() {
        let mut e = engine(10, 3);
        e.feed(b"0123456789abcd");
        let snap = e.snapshot();
        assert_eq!(snap.line_text(0), "0123456789");
        assert_eq!(snap.line_text(1), "abcd");
        e.resize(20, 3, 8, 16);
        let snap = e.snapshot();
        assert_eq!(snap.lines.len(), 3);
        assert_eq!(snap.line_text(0), "0123456789abcd");
        e.resize(5, 2, 8, 16);
        assert_eq!(e.snapshot().lines.len(), 2);
    }

    #[test]
    fn ghostty_switches_to_the_alternate_screen_and_back() {
        let mut e = engine(20, 4);
        e.feed(b"shell prompt");
        assert!(!e.modes().alt_screen);
        e.feed(b"\x1b[?1049h\x1b[H\x1b[2Jfull screen");
        assert!(e.modes().alt_screen);
        assert_eq!(e.snapshot().line_text(0), "full screen");
        e.feed(b"\x1b[?1049l");
        assert!(!e.modes().alt_screen);
        assert_eq!(e.snapshot().line_text(0), "shell prompt");
    }

    #[test]
    fn ghostty_reports_modes_title_and_replies() {
        let mut e = engine(20, 4);
        e.feed(b"\x1b[?1h\x1b[?2004h\x1b[?1000h\x1b]2;my title\x07");
        let modes = e.modes();
        assert!(modes.app_cursor && modes.bracketed_paste && modes.mouse_tracking);
        assert_eq!(e.title().as_deref(), Some("my title"));
        e.feed(b"\x1b[6n");
        assert_eq!(e.take_replies(), b"\x1b[1;1R");
        e.feed(b"\x1b[c");
        let da = String::from_utf8(e.take_replies()).unwrap();
        assert!(da.starts_with("\x1b[?62"), "{da:?}");
        assert!(e.take_replies().is_empty());
    }

    #[test]
    fn ghostty_scrolls_back_through_history() {
        let mut e = engine(10, 3);
        for i in 0..10 {
            e.feed(format!("line {i}\r\n").as_bytes());
        }
        assert_eq!(e.snapshot().line_text(0), "line 8");
        e.scroll(ScrollTo::Delta(3));
        let snap = e.snapshot();
        assert_eq!(snap.line_text(0), "line 5");
        assert_eq!(snap.cursor.at, None);
        e.scroll(ScrollTo::Bottom);
        assert_eq!(e.snapshot().line_text(0), "line 8");
    }

    #[test]
    fn ghostty_selects_and_copies() {
        let mut e = engine(20, 4);
        e.feed(b"hello world\r\nhttps://x.dev/a");
        let at = |line, column| ViewPoint { line, column };
        e.select_begin(at(0, 0), Side::Left, SelectKind::Simple);
        assert!(e.has_selection() && e.selection_is_empty());
        e.select_extend(at(0, 4), Side::Right);
        assert!(!e.selection_is_empty());
        assert_eq!(e.selection_text().as_deref(), Some("hello"));
        assert!(e.snapshot().lines[0][0].selected);
        e.select_begin(at(0, 7), Side::Left, SelectKind::Word);
        assert_eq!(e.selection_text().as_deref(), Some("world"));
        e.clear_selection();
        assert!(!e.has_selection());
        let (text, under) = e.wrapped_line_at(at(1, 3)).unwrap();
        assert!(text.starts_with("https://x.dev/a"));
        assert_eq!(under, 3);
    }
}
