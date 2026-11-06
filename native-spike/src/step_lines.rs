//! What a row in the activity tree says: an icon, a verb and the thing acted on, in place of the
//! host's raw "Tool call computer_launch".
use crate::i18n::t;

pub struct Line {
    pub icon: &'static str,
    pub verb: &'static str,
    pub target: Option<String>,
}

/// The verb and icon of a browser or computer tool. None for any other tool name.
fn known_tool(tool: &str) -> Option<(&'static str, &'static str)> {
    Some(match tool {
        "computer_launch" => ("app-window", t("Abriu")),
        "computer_apps" => ("app-window", t("Listou os apps")),
        "computer_focus" => ("app-window", t("Trouxe para a frente")),
        "computer_observe" => ("eye", t("Observou")),
        "computer_click" | "browser_click" => ("pointer", t("Clicou")),
        "computer_type" | "browser_type" => ("keyboard", t("Digitou")),
        "computer_key" | "browser_press" => ("keyboard", t("Pressionou uma tecla")),
        "computer_scroll" | "browser_scroll" => ("pointer", t("Rolou")),
        "computer_screenshot" | "browser_screenshot" => ("eye", t("Captura da tela")),
        "browser_open" | "browser_navigate" => ("globe", t("Abriu")),
        "browser_back" => ("globe", t("Voltou")),
        "browser_forward" => ("globe", t("Avançou")),
        "browser_reload" => ("globe", t("Recarregou")),
        "browser_close" => ("globe", t("Fechou o navegador")),
        "browser_snapshot" => ("eye", t("Leu a página")),
        "browser_wait_for" => ("globe", t("Esperou a página")),
        _ => return None,
    })
}

fn file_name(path: &str) -> String {
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn some(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
}

/// `tool` is the MCP tool's name when the host sent it; older rows only have it as their detail.
pub fn line(label: &str, detail: &str, tool: Option<&str>, target: Option<&str>) -> Line {
    let name = tool.unwrap_or(detail);
    if let Some((icon, verb)) = known_tool(name) {
        return Line {
            icon,
            verb,
            target: target.and_then(some),
        };
    }
    // Claude loading a tool's schema: housekeeping, and its query is an internal name.
    if detail.starts_with("select:") {
        return Line {
            icon: "wrench",
            verb: t("Carregou ferramentas"),
            target: None,
        };
    }
    let (icon, verb, shown) = match label {
        "Read / search" => ("file-text", t("Leu"), file_name(detail)),
        "File changes" => ("pencil-line", t("Editou"), file_name(detail)),
        "Command" => ("square-terminal", t("Executou"), detail.to_owned()),
        "Web search" => ("search", t("Buscou"), detail.to_owned()),
        "Skill" => ("wrench", t("Usou a skill"), detail.to_owned()),
        "Delegate task" => ("bot", t("Delegou"), detail.to_owned()),
        "Tool call" | "" => ("wrench", t("Usou"), detail.to_owned()),
        _ => {
            return Line {
                icon: "wrench",
                verb: "",
                target: some(&format!("{label} {detail}").trim().to_owned()),
            };
        }
    };
    Line {
        icon,
        verb,
        target: some(&shown),
    }
}

/// The row as one string, for the shimmering live row.
pub fn text(line: &Line) -> String {
    match &line.target {
        Some(target) if !line.verb.is_empty() => format!("{} {target}", line.verb),
        Some(target) => target.clone(),
        None => line.verb.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tool_call_reads_as_a_verb_with_its_target() {
        let open = line("Tool call", "computer_launch", Some("computer_launch"), Some("Calculator"));
        assert_eq!((open.icon, open.verb), ("app-window", t("Abriu")));
        assert_eq!(open.target.as_deref(), Some("Calculator"));
    }

    #[test]
    fn loading_a_tool_schema_hides_its_internal_name() {
        let load = line("Tool call", "select:mcp__sirus_computer__computer_launch", None, None);
        assert_eq!((load.verb, load.target), (t("Carregou ferramentas"), None));
    }

    #[test]
    fn older_rows_keep_the_tool_in_their_detail() {
        let click = line("Tool call", "computer_click", None, None);
        assert_eq!((click.verb, click.target), (t("Clicou"), None));
    }

    #[test]
    fn a_read_shows_the_file_name_and_an_unknown_tool_its_own_name() {
        assert_eq!(
            line("Read / search", "/a/b/SKILL.md", None, None).target.as_deref(),
            Some("SKILL.md")
        );
        let other = line("Tool call", "sirus_show_visual", None, None);
        assert_eq!((other.verb, other.target.as_deref()), (t("Usou"), Some("sirus_show_visual")));
    }
}
