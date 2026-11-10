//! The macOS menu bar: Sirus, Arquivo, Visualizar, Janela and Ajuda. Each item runs what the
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
            MenuItem::action(t("Configurações…"), OpenSettings),
            MenuItem::separator(),
            MenuItem::action(t("Ocultar Sirus"), HideApp),
            MenuItem::action(t("Ocultar outros"), HideOthers),
            MenuItem::action(t("Mostrar tudo"), ShowAll),
            MenuItem::separator(),
            MenuItem::action(t("Sair do Sirus"), QuitApp),
        ]),
        Menu::new(t("Arquivo")).items([
            MenuItem::action(t("Nova conversa"), NewConversation),
            MenuItem::action(t("Fechar aba"), CloseTab),
        ]),
        Menu::new(t("Visualizar")).items([
            MenuItem::action(t("Barra lateral"), ToggleSidebar),
            MenuItem::action(t("Painel lateral"), ToggleDock),
            MenuItem::separator(),
            MenuItem::action(t("Paleta de comandos"), OpenPalette),
            MenuItem::action(t("Buscar em tudo"), SearchEverything),
        ]),
        Menu::new(t("Janela")).items([
            MenuItem::action(t("Minimizar"), Minimize),
            MenuItem::action(t("Zoom"), Zoom),
        ]),
        Menu::new(t("Ajuda")).items([MenuItem::action(t("Site do Sirus"), OpenSite)]),
    ]);
}
