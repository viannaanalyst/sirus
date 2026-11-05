//! A small block creature per provider for the terminal's banner, in the same three-line block
//! style as Claude Code's own: the terminal draws these characters solid, with no gaps.
pub fn sprite(provider: &str) -> ([&'static str; 3], (u8, u8, u8)) {
    match provider {
        "claude" => (
            [
                " \u{2590}\u{259B}\u{2588}\u{2588}\u{2588}\u{259C}\u{258C}",
                "\u{259D}\u{259C}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{259B}\u{2598}",
                "  \u{2598}\u{2598} \u{259D}\u{259D}",
            ],
            (217, 119, 87),
        ),
        "codex" => (
            [
                " \u{259F}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2599}",
                " \u{2588}\u{2596}\u{2588}\u{2588}\u{2588}\u{2597}\u{2588}",
                " \u{259C}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{259B}",
            ],
            (122, 140, 255),
        ),
        "cursor" => (
            [
                "   \u{2597}\u{2588}\u{2596}",
                " \u{2597}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2596}",
                "   \u{259D}\u{2588}\u{2598}",
            ],
            (205, 205, 205),
        ),
        "opencode" => (
            [
                " \u{2588}\u{2580}\u{2580}\u{2580}\u{2580}\u{2580}\u{2588}",
                " \u{2588} \u{2596} \u{2597} \u{2588}",
                " \u{2588}\u{2584}\u{2584}\u{2584}\u{2584}\u{2584}\u{2588}",
            ],
            (205, 200, 200),
        ),
        "grok" => (
            [
                " \u{259E}\u{2580}\u{2580}\u{2580}\u{2580}\u{259E}",
                " \u{258C} \u{2597}\u{259E} \u{2590}",
                " \u{259E}\u{2584}\u{2584}\u{2584}\u{2584}\u{259E}",
            ],
            (205, 205, 205),
        ),
        "antigravity" => (
            [
                "   \u{2597}\u{2588}\u{2596}",
                "  \u{259F}\u{259B}\u{2580}\u{259C}\u{2599}",
                " \u{259F}\u{259B}   \u{259C}\u{2599}",
            ],
            (143, 180, 245),
        ),
        "droid" => (
            [
                "  \u{2596}   \u{2597}",
                " \u{2590}\u{259B}\u{2588}\u{2588}\u{2588}\u{259C}\u{258C}",
                "  \u{259C}\u{2588}\u{2588}\u{2588}\u{259B}",
            ],
            (238, 138, 79),
        ),
        "pi" => (
            [
                " \u{2584}\u{2584}\u{2584}\u{2584}\u{2584}\u{2584}\u{2584}",
                "  \u{2590}\u{258C} \u{2590}\u{258C}",
                "  \u{2590}\u{258C} \u{259D}\u{2599}\u{2596}",
            ],
            (205, 205, 205),
        ),
        "devin" => (
            [
                "  \u{2597}\u{259F}\u{2588}\u{2599}\u{2596}",
                " \u{2590}\u{259B}\u{259C}\u{2588}\u{259B}\u{259C}\u{258C}",
                " \u{259D}\u{2598}\u{2580}\u{2598}\u{2580}\u{259D}\u{2598}",
            ],
            (127, 196, 184),
        ),
        _ => (
            [
                " \u{2599} \u{2597}\u{2588}\u{2596} \u{259F}",
                " \u{259C}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{259B}",
                "   \u{2580}\u{259D}\u{2598}\u{2580}",
            ],
            (217, 184, 113),
        ),
    }
}

/// The banner as terminal text: the creature, then name and version, model, folder.
pub fn banner(provider: &str, name: &str, version: &str, model: &str, folder: &str) -> String {
    let (rows, (r, g, b)) = sprite(provider);
    let lines = [
        format!("\x1b[1m{name}\x1b[22m \x1b[90m{version}\x1b[0m"),
        format!("\x1b[90m{model}\x1b[0m"),
        format!("\x1b[90m{folder}\x1b[0m"),
    ];
    let mut out = String::new();
    for (row, text) in rows.iter().zip(lines.iter()) {
        let pad = 11usize.saturating_sub(row.chars().count());
        out.push_str(&format!(
            "\x1b[38;2;{r};{g};{b}m{row}\x1b[0m{}{text}\r\n",
            " ".repeat(pad)
        ));
    }
    out.push_str("\r\n");
    out
}
