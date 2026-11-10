//! The macOS menu bar: Sirus, Arquivo, Visualizar, Janela and Ajuda, each item with its SF Symbol
//! as the system's own apps show them. Each item runs what the
//! window's own keys run. No shortcuts are bound here, so the keys keep going through the shell's
//! key handler as before; the menus are for clicking.
use gpui::{App, Menu, MenuItem, actions};

actions!(
    sirus_menu,
    [
        OpenSettings,
        HideApp,
        HideOthers,
        ShowAll,
        QuitApp,
        NewConversation,
        CloseTab,
        ToggleSidebar,
        ToggleDock,
        OpenPalette,
        SearchEverything,
        Minimize,
        Zoom,
        OpenSite,
    ]
);

/// An item's title with its SF Symbol, which the vendored macOS menu code splits at `\u{1f}`.
fn icon(symbol: &str, title: &str) -> String {
    format!("{symbol}\u{1f}{title}")
}

/// The menus, in the app's language, and the items that need no window.
pub fn install(cx: &mut App) {
    use crate::i18n::t;
    cx.on_action(|_: &HideApp, cx| cx.hide());
    cx.on_action(|_: &HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &QuitApp, cx| cx.quit());
    cx.on_action(|_: &OpenSite, cx| cx.open_url("https://siruscode.com"));
    cx.set_menus([
        Menu::new("Sirus").items([
            MenuItem::action(icon("gearshape", t("Configurações…")), OpenSettings),
            MenuItem::separator(),
            MenuItem::action(icon("eye.slash", t("Ocultar Sirus")), HideApp),
            MenuItem::action(icon("eye.slash.circle", t("Ocultar outros")), HideOthers),
            MenuItem::action(icon("eye", t("Mostrar tudo")), ShowAll),
            MenuItem::separator(),
            MenuItem::action(icon("power", t("Sair do Sirus")), QuitApp),
        ]),
        Menu::new(t("Arquivo")).items([
            MenuItem::action(
                icon("square.and.pencil", t("Nova conversa")),
                NewConversation,
            ),
            MenuItem::action(icon("xmark.square", t("Fechar aba")), CloseTab),
        ]),
        Menu::new(t("Visualizar")).items([
            MenuItem::action(icon("sidebar.left", t("Barra lateral")), ToggleSidebar),
            MenuItem::action(icon("sidebar.right", t("Painel lateral")), ToggleDock),
            MenuItem::separator(),
            MenuItem::action(icon("command", t("Paleta de comandos")), OpenPalette),
            MenuItem::action(
                icon("magnifyingglass", t("Buscar em tudo")),
                SearchEverything,
            ),
        ]),
        Menu::new(t("Janela")).items([
            MenuItem::action(icon("minus.square", t("Minimizar")), Minimize),
            MenuItem::action(icon("arrow.up.left.and.arrow.down.right", t("Zoom")), Zoom),
        ]),
        Menu::new(t("Ajuda")).items([MenuItem::action(
            icon("globe", t("Site do Sirus")),
            OpenSite,
        )]),
    ]);
}
