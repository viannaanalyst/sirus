use std::path::{Component, Path};

use crate::error::{Error, Result};
use crate::models::{FileEntry, WorkspaceEntryKind};
#[cfg(unix)]
use crate::paths::display_path;
use crate::paths::is_ignored_dir;

fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
        || is_ignored_dir(&name.to_ascii_lowercase())
    {
        return Err(Error::invalid_path("Use a single printable name up to 255 bytes, without separators or reserved workspace names."));
    }
    Ok(())
}

/// One explicit empty entry. Unix descriptor-relative admission prevents linked
/// components and leaf replacement from redirecting creation. It is not an OS
/// jail: another process can rename a verified directory after the final check.
pub fn create_entry(
    root: &Path,
    parent: Option<&Path>,
    kind: WorkspaceEntryKind,
    name: &str,
    admit: impl FnOnce() -> Result<()>,
) -> Result<FileEntry> {
    validate_name(name)?;
    let parent = parent.unwrap_or(root);
    let raw = parent.to_string_lossy();
    if !parent.is_absolute()
        || raw.len() > 4096
        || raw
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
        || raw.chars().any(|ch| ch.is_control() || ch == '\\')
    {
        return Err(Error::invalid_path("Invalid workspace destination."));
    }
    let relative = parent
        .strip_prefix(root)
        .map_err(|_| Error::invalid_path("Workspace destination is outside this session."))?;
    if relative.components().count() > 128 {
        return Err(Error::invalid_path("Invalid workspace destination."));
    }
    for component in relative.components() {
        if !matches!(component, Component::Normal(_))
            || is_ignored_dir(&component.as_os_str().to_string_lossy().to_ascii_lowercase())
        {
            return Err(Error::invalid_path("Invalid workspace destination."));
        }
    }
    #[cfg(unix)]
    return unix::create(root, parent, relative, kind, name, admit);
    #[cfg(not(unix))]
    {
        let _ = (root, parent, relative, kind, admit);
        Err(Error::invalid_path(
            "Workspace creation is unavailable on this platform.",
        ))
    }
}

/// Moves one existing entry inside the workspace to the system Trash, where it can be restored.
/// Never deletes permanently, never the workspace root, reserved folders or anything reached
/// through a linked parent. Like creation, this is a preflight check, not an OS jail.
pub fn trash_entry(root: &Path, path: &Path) -> Result<()> {
    let raw = path.to_string_lossy();
    if !path.is_absolute()
        || raw.len() > 4096
        || raw
            .split(['/', '\\'])
            .any(|part| part == "." || part == "..")
        || raw.chars().any(|ch| ch.is_control() || ch == '\\')
    {
        return Err(Error::invalid_path("Invalid workspace entry."));
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| Error::invalid_path("Workspace entry is outside this session."))?;
    let components = relative.components().collect::<Vec<_>>();
    if components.is_empty() || components.len() > 128 {
        return Err(Error::invalid_path("Invalid workspace entry."));
    }
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        if !matches!(component, Component::Normal(_))
            || is_ignored_dir(&component.as_os_str().to_string_lossy().to_ascii_lowercase())
        {
            return Err(Error::invalid_path(
                "This entry cannot be moved to the Trash here.",
            ));
        }
        current.push(component);
        let meta = std::fs::symlink_metadata(&current)
            .map_err(|_| Error::invalid_path("Workspace entry no longer exists."))?;
        if index + 1 < components.len() && (!meta.is_dir() || meta.file_type().is_symlink()) {
            return Err(Error::invalid_path("Invalid workspace entry."));
        }
    }
    move_to_trash(&current)
}

#[cfg(target_os = "macos")]
fn move_to_trash(path: &Path) -> Result<()> {
    use objc2_foundation::{NSFileManager, NSString, NSURL};
    let path = path
        .to_str()
        .ok_or_else(|| Error::invalid_path("Invalid workspace entry."))?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // A link itself goes to the Trash; its target is never followed.
    NSFileManager::defaultManager()
        .trashItemAtURL_resultingItemURL_error(&url, None)
        .map_err(|_| Error::invalid_path("Could not move this entry to the Trash."))
}

#[cfg(not(target_os = "macos"))]
fn move_to_trash(_path: &Path) -> Result<()> {
    Err(Error::invalid_path(
        "Moving to the Trash is unavailable on this platform.",
    ))
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::ffi::{CString, OsStr};
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::MetadataExt;

    fn c_name(name: &OsStr) -> Result<CString> {
        CString::new(name.as_bytes())
            .map_err(|_| Error::invalid_path("Invalid workspace destination."))
    }

    fn open_dir_at(parent: &File, name: &OsStr) -> Result<File> {
        let name = c_name(name)?;
        // O_NOFOLLOW applies to every component, not just the final parent.
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn open_root(root: &Path) -> Result<File> {
        if !root.is_absolute() {
            return Err(Error::invalid_path("Invalid workspace destination."));
        }
        let mut handle = File::open("/")?;
        for component in root.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => handle = open_dir_at(&handle, name)?,
                _ => return Err(Error::invalid_path("Invalid workspace destination.")),
            }
        }
        Ok(handle)
    }

    fn open_parent(root: &File, relative: &Path) -> Result<File> {
        let mut handle = root.try_clone()?;
        for component in relative.components() {
            handle = open_dir_at(&handle, component.as_os_str())?;
        }
        Ok(handle)
    }

    fn same_directory(a: &File, b: &File) -> Result<()> {
        let a = a.metadata()?;
        let b = b.metadata()?;
        if !a.is_dir() || !b.is_dir() || a.dev() != b.dev() || a.ino() != b.ino() {
            return Err(Error::invalid_path(
                "Workspace destination changed. Try again.",
            ));
        }
        Ok(())
    }

    pub(super) fn create(
        root: &Path,
        parent: &Path,
        relative: &Path,
        kind: WorkspaceEntryKind,
        name: &str,
        admit: impl FnOnce() -> Result<()>,
    ) -> Result<FileEntry> {
        let root_handle = open_root(root)?;
        let parent_handle = open_parent(&root_handle, relative)?;
        crate::paths::verify_open_handle(root, &root_handle)?;
        crate::paths::verify_open_handle(root, &parent_handle)?;
        // The caller rechecks the live session/project while its metadata lock
        // remains held through the mutation; deletion cannot overtake admission.
        admit()?;
        if crate::paths::ensure_dir(root)? != root {
            return Err(Error::invalid_path(
                "Workspace destination changed. Try again.",
            ));
        }
        let fresh_root = open_root(root)?;
        same_directory(&root_handle, &fresh_root)?;
        same_directory(&parent_handle, &open_parent(&fresh_root, relative)?)?;
        crate::paths::verify_open_handle(root, &parent_handle)?;
        let leaf = c_name(OsStr::new(name))?;
        let created = match kind {
            WorkspaceEntryKind::File => {
                let fd = unsafe {
                    libc::openat(
                        parent_handle.as_raw_fd(),
                        leaf.as_ptr(),
                        libc::O_WRONLY
                            | libc::O_CREAT
                            | libc::O_EXCL
                            | libc::O_NOFOLLOW
                            | libc::O_CLOEXEC,
                        0o666,
                    )
                };
                if fd < 0 {
                    return Err(creation_error());
                }
                unsafe { File::from_raw_fd(fd) }
            }
            WorkspaceEntryKind::Directory => {
                if unsafe { libc::mkdirat(parent_handle.as_raw_fd(), leaf.as_ptr(), 0o777) } != 0 {
                    return Err(creation_error());
                }
                open_dir_at(&parent_handle, OsStr::new(name))?
            }
        };
        let metadata = created.metadata()?;
        if match kind {
            WorkspaceEntryKind::File => !metadata.is_file(),
            WorkspaceEntryKind::Directory => !metadata.is_dir(),
        } {
            return Err(Error::invalid_path(
                "Workspace destination changed. Try again.",
            ));
        }
        crate::paths::verify_open_handle(root, &created)?;
        same_directory(&parent_handle, &open_parent(&open_root(root)?, relative)?)?;
        let path = parent.join(name);
        let published = std::fs::symlink_metadata(&path)?;
        if published.file_type().is_symlink()
            || published.dev() != metadata.dev()
            || published.ino() != metadata.ino()
        {
            return Err(Error::invalid_path(
                "Workspace destination changed. Try again.",
            ));
        }
        Ok(FileEntry {
            name: name.into(),
            path: display_path(&path),
            is_dir: matches!(kind, WorkspaceEntryKind::Directory),
            ignored: false,
        })
    }

    fn creation_error() -> Error {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            Error::invalid_path("An entry with this name already exists.")
        } else {
            error.into()
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

    fn create(
        root: &Path,
        parent: Option<&Path>,
        kind: WorkspaceEntryKind,
        name: &str,
    ) -> Result<FileEntry> {
        create_entry(root, parent, kind, name, || Ok(()))
    }

    #[test]
    fn creates_empty_file_and_directory() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let file = create(&root, None, WorkspaceEntryKind::File, "new file.txt").unwrap();
        assert!(!file.is_dir);
        assert_eq!(fs::read(&file.path).unwrap(), b"");
        let dir = create(&root, None, WorkspaceEntryKind::Directory, "new folder").unwrap();
        assert!(dir.is_dir);
        assert_eq!(fs::read_dir(dir.path).unwrap().count(), 0);
    }

    #[test]
    fn creates_in_existing_nested_parent_only() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let parent = root.join("src/nested");
        fs::create_dir_all(&parent).unwrap();
        let entry = create(&root, Some(&parent), WorkspaceEntryKind::File, "file.rs").unwrap();
        assert_eq!(Path::new(&entry.path), parent.join("file.rs"));
        assert!(create(
            &root,
            Some(&root.join("missing")),
            WorkspaceEntryKind::File,
            "file"
        )
        .is_err());
        assert!(!root.join("missing").exists());
    }

    #[test]
    fn duplicate_admission_preserves_file_contents_and_directories() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        fs::write(root.join("existing"), "keep me").unwrap();
        fs::create_dir(root.join("folder")).unwrap();
        for name in ["existing", "folder"] {
            for kind in [WorkspaceEntryKind::File, WorkspaceEntryKind::Directory] {
                assert!(create(&root, None, kind, name).is_err());
            }
        }
        assert_eq!(
            fs::read_to_string(root.join("existing")).unwrap(),
            "keep me"
        );
        assert!(root.join("folder").is_dir());
    }

    #[test]
    fn refuses_traversal_reserved_control_and_oversized_names() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        for name in [
            "",
            " ",
            ".",
            "..",
            "a/b",
            "a\\b",
            "/tmp",
            "a\n",
            "a\0",
            ".git",
            ".GIT",
            "node_modules",
            "target",
            ".cache",
        ] {
            assert!(
                create(&root, None, WorkspaceEntryKind::File, name).is_err(),
                "{name:?}"
            );
        }
        assert!(create(&root, None, WorkspaceEntryKind::File, &"a".repeat(256)).is_err());
        assert!(create(&root, None, WorkspaceEntryKind::File, &"é".repeat(128)).is_err());
        assert!(create(&root, None, WorkspaceEntryKind::File, &"a".repeat(255)).is_ok());
    }

    #[test]
    fn refuses_foreign_or_traversing_parents() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        for parent in [
            foreign.path().to_path_buf(),
            root.join("../"),
            root.join("./"),
            Path::new("relative").to_path_buf(),
        ] {
            assert!(create(&root, Some(&parent), WorkspaceEntryKind::File, "nope").is_err());
        }
        assert!(!foreign.path().join("nope").exists());
    }

    #[test]
    fn failed_owner_admission_does_not_create() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        assert!(
            create_entry(&root, None, WorkspaceEntryKind::File, "nope", || Err(
                Error::not_found("session not found")
            ))
            .is_err()
        );
        assert!(!root.join("nope").exists());
    }

    #[test]
    fn rejects_replaced_parent_before_mutation() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let parent = root.join("parent");
        fs::create_dir(&parent).unwrap();
        let moved = root.join("moved");
        let result = create_entry(
            &root,
            Some(&parent),
            WorkspaceEntryKind::File,
            "nope",
            || {
                fs::rename(&parent, &moved)?;
                fs::create_dir(&parent)?;
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(!parent.join("nope").exists());
        assert!(!moved.join("nope").exists());
    }

    #[test]
    fn rejects_replaced_root_before_mutation() {
        let fixture = tempfile::tempdir().unwrap();
        let base = fixture.path().canonicalize().unwrap();
        let root = base.join("root");
        let moved = base.join("moved");
        fs::create_dir(&root).unwrap();
        let result = create_entry(&root, None, WorkspaceEntryKind::Directory, "nope", || {
            fs::rename(&root, &moved)?;
            fs::create_dir(&root)?;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!root.join("nope").exists());
        assert!(!moved.join("nope").exists());
    }

    #[test]
    fn kind_is_a_closed_wire_enum() {
        for (wire, expected) in [("\"file\"", "\"file\""), ("\"directory\"", "\"directory\"")] {
            let kind: WorkspaceEntryKind = serde_json::from_str(wire).unwrap();
            assert_eq!(serde_json::to_string(&kind).unwrap(), expected);
        }
        assert!(serde_json::from_str::<WorkspaceEntryKind>("\"symlink\"").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_linked_parents_and_existing_link_leaves() {
        use std::os::unix::fs::symlink;
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let foreign = tempfile::tempdir().unwrap();
        fs::create_dir(root.join("real")).unwrap();
        for (name, target) in [("foreign", foreign.path()), ("local", &root.join("real"))] {
            symlink(target, root.join(name)).unwrap();
            assert!(create(
                &root,
                Some(&root.join(name)),
                WorkspaceEntryKind::File,
                "nope"
            )
            .is_err());
        }
        symlink(foreign.path().join("absent"), root.join("linked")).unwrap();
        for kind in [WorkspaceEntryKind::File, WorkspaceEntryKind::Directory] {
            assert!(create(&root, None, kind, "linked").is_err());
        }
        assert!(!foreign.path().join("absent").exists());
    }
}

#[cfg(test)]
mod trash_tests {
    use super::*;

    #[test]
    fn trash_refuses_roots_reserved_folders_linked_parents_and_outside_paths() {
        let root = std::env::temp_dir().join(format!("sirus-trash-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        let root = root.canonicalize().unwrap();
        assert!(trash_entry(&root, &root).is_err());
        assert!(trash_entry(&root, &root.join(".git")).is_err());
        assert!(trash_entry(&root, &root.join("node_modules/pkg")).is_err());
        assert!(trash_entry(&root, &root.join("src/../src")).is_err());
        assert!(trash_entry(&root, Path::new("src")).is_err());
        assert!(trash_entry(&root, &std::env::temp_dir()).is_err());
        assert!(trash_entry(&root, &root.join("missing")).is_err());
        #[cfg(unix)]
        {
            let outside =
                std::env::temp_dir().join(format!("sirus-trash-out-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&outside).unwrap();
            std::fs::write(outside.join("keep.txt"), "keep").unwrap();
            std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
            // A file reached through a linked parent is never touched.
            assert!(trash_entry(&root, &root.join("link/keep.txt")).is_err());
            assert!(outside.join("keep.txt").exists());
            let _ = std::fs::remove_dir_all(&outside);
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
