//! The app's fonts (fonts.rs, MIT): one fixed choice, nothing to pick. The interface
//! is GPUI's default `.SystemUIFont`, code is Menlo, and the terminal leads with a Nerd Font patched
//! JetBrains Mono when one is installed, so prompt icons draw, else Menlo.
use gpui::{Font, FontFallbacks, TextSystem};
use std::sync::OnceLock;

const TERMINAL_PRIMARY: &[&str] = &[
    "JetBrainsMono NFM",
    "JetBrainsMono Nerd Font Mono",
    "JetBrainsMono NF",
];
/// Fill the Private Use Area glyphs (powerline arrows, git and folder icons) a plain mono lacks.
const TERMINAL_ICONS: &[&str] = &[
    "Symbols Nerd Font Mono",
    "Symbols Nerd Font",
    "MesloLGS NF",
    "MesloLGS Nerd Font Mono",
    "FiraCode Nerd Font Mono",
    "Hack Nerd Font Mono",
    "CaskaydiaCove Nerd Font Mono",
];

/// The installed families the terminal's font is built from: the lead face, then its fallbacks.
fn terminal_stack(installed: &[String]) -> (String, Vec<String>) {
    let has = |name: &str| installed.iter().any(|known| known == name);
    let lead = TERMINAL_PRIMARY
        .iter()
        .find(|name| has(name))
        .map_or("Menlo", |name| *name)
        .to_owned();
    let icons = TERMINAL_ICONS
        .iter()
        .filter(|name| has(name))
        .map(|name| (*name).to_owned())
        .collect();
    (lead, icons)
}

/// The terminal's font. The list of installed families is read once.
pub fn terminal(text_system: &TextSystem) -> Font {
    static STACK: OnceLock<(String, Vec<String>)> = OnceLock::new();
    let (lead, icons) = STACK.get_or_init(|| terminal_stack(&text_system.all_font_names()));
    let mut font = gpui::font(lead.clone());
    if !icons.is_empty() {
        font.fallbacks = Some(FontFallbacks::from_fonts(icons.clone()));
    }
    font
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_prefers_jetbrains_and_falls_back_to_menlo() {
        let names = |list: &[&str]| list.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        assert_eq!(terminal_stack(&names(&["Arial"])).0, "Menlo");
        let (lead, icons) = terminal_stack(&names(&["JetBrainsMono NF", "Hack Nerd Font Mono"]));
        assert_eq!(lead, "JetBrainsMono NF");
        assert_eq!(icons, ["Hack Nerd Font Mono"]);
    }
}
