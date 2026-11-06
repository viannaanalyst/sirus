//! Image files an agent read or changed. The activity tree shows each one as a row that opens the
//! picture in the side panel's Files pane (panel_files.rs), read through the host's `reply_image`.
//!
//! A screenshot a tool returned is a step too: the host saves its picture and names the file in
//! `imagePath`, and `row_label` names the row "Captura da tela" (the glyph is the file's own).

use std::sync::Arc;

/// The formats `reply_image` opens (the host's own list).
const EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "gif", "webp"];

/// True when the last component of `path` has an image extension.
pub fn is_image_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    match name.rsplit_once('.') {
        Some((stem, extension)) => {
            !stem.is_empty() && EXTENSIONS.iter().any(|known| known.eq_ignore_ascii_case(extension))
        }
        None => false,
    }
}

/// The image file a step names. Only a read or an edit counts, and only an absolute (or `~/`)
/// path: a glob (`/x/**/*.png`) or a search pattern is not a file.
pub fn step_image(kind: &str, detail: &str) -> Option<String> {
    let path = detail.trim();
    let is_file = matches!(kind, "read" | "edit");
    let absolute = path.starts_with('/') || path.starts_with("~/");
    let plain = !path.contains(['*', '?', '[', '{', '\n']);
    (is_file && absolute && plain && is_image_path(path)).then(|| path.to_owned())
}

/// The picture inside a `data:image/...;base64,` URL from `reply_image`.
pub fn picture(url: &str) -> Option<Arc<gpui::Image>> {
    use base64::Engine as _;
    let (head, data) = url.strip_prefix("data:")?.split_once(";base64,")?;
    let format = crate::attachment_doc::image_format("", head)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()?;
    Some(Arc::new(gpui::Image::from_bytes(format, bytes)))
}

/// Whether a path is a picture the host saved from an agent's tool, named only by its hash.
pub fn is_capture(path: &str) -> bool {
    path.contains("/tool-images/")
}

/// The name a row shows: the path's last component.
pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The text of an image row: "Captura da tela" for a screenshot, else the file's name.
pub fn row_label(path: &str, capture: bool) -> String {
    if capture {
        crate::i18n::t("Captura da tela").to_owned()
    } else {
        file_name(path).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_paths_are_recognised_by_extension_in_any_case() {
        assert!(is_image_path("/Users/me/shot.png"));
        assert!(is_image_path("/tmp/Photo.JPEG"));
        assert!(is_image_path("~/Desktop/a.webp"));
        assert!(!is_image_path("/tmp/notes.txt"));
        assert!(!is_image_path("/tmp/png"));
        assert!(!is_image_path("/tmp/.png"), "a dot file named .png has no stem");
        assert!(!is_image_path("/tmp/dir.png/readme"));
    }

    #[test]
    fn a_read_or_edit_of_an_absolute_image_is_a_row() {
        assert_eq!(
            step_image("read", "/Users/me/shot.png").as_deref(),
            Some("/Users/me/shot.png")
        );
        assert_eq!(
            step_image("edit", " /tmp/logo.jpg ").as_deref(),
            Some("/tmp/logo.jpg")
        );
        assert_eq!(step_image("read", "~/Pictures/a.gif").as_deref(), Some("~/Pictures/a.gif"));
    }

    #[test]
    fn globs_relative_paths_commands_and_other_kinds_are_not_rows() {
        assert_eq!(step_image("read", "/Users/me/**/*.png"), None, "a glob");
        assert_eq!(step_image("read", "src/shot.png"), None, "relative");
        assert_eq!(step_image("read", "icon.png"), None, "a search pattern");
        assert_eq!(step_image("command", "/tmp/shot.png"), None);
        assert_eq!(step_image("agent", "/tmp/shot.png"), None);
        assert_eq!(step_image("read", "/tmp/notes.md"), None);
    }

    #[test]
    fn file_name_is_the_last_component() {
        assert_eq!(file_name("/Users/me/shot.png"), "shot.png");
        assert_eq!(file_name("shot.png"), "shot.png");
    }

    #[test]
    fn a_screenshot_row_is_named_for_the_capture_and_a_file_by_its_name() {
        assert_eq!(row_label("/a/3f9c.png", true), "Captura da tela");
        assert_eq!(row_label("/Users/me/shot.png", false), "shot.png");
    }
}
