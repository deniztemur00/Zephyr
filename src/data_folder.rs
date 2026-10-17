use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::{credentials, stats};

pub const CAPTURES: &str = "captures";
const ENTRIES: [&str; 4] = [
    credentials::SESSION_FILE,
    credentials::WEB_PROFILE_DIR,
    stats::FILE,
    CAPTURES,
];

#[derive(Debug, Error)]
pub enum MoveError {
    #[error("the new folder can't be inside the current one, or the other way round")]
    Nested,
    #[error("{0} already holds Zephyr data; pick another folder")]
    Occupied(PathBuf),
    #[error("could not copy to {path}: {source}")]
    Copy { path: PathBuf, source: io::Error },
}

// Step 1 of a move: copies the app's files and leaves `from` untouched.
// On failure the partial copies are removed again.
pub fn copy(from: &Path, to: &Path) -> Result<(), MoveError> {
    if from.starts_with(to) || to.starts_with(from) {
        return Err(MoveError::Nested);
    }
    if ENTRIES.iter().any(|entry| to.join(entry).exists()) {
        return Err(MoveError::Occupied(to.to_owned()));
    }
    let present: Vec<&str> = ENTRIES
        .into_iter()
        .filter(|entry| from.join(entry).exists())
        .collect();
    let copied = present.iter().try_for_each(|entry| {
        copy_tree(&from.join(entry), &to.join(entry)).map_err(|source| MoveError::Copy {
            path: to.join(entry),
            source,
        })
    });
    if copied.is_err() {
        // These did not exist before the copy, so removing them only undoes our own work.
        let _ = remove(to);
    }
    copied
}

// Step 2, once the config points at the new folder: removes the app's files from the old one.
pub fn remove(dir: &Path) -> io::Result<()> {
    ENTRIES.iter().try_for_each(|entry| {
        let path = dir.join(entry);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => fs::remove_dir_all(&path),
            Ok(_) => fs::remove_file(&path),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err),
        }
    })
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        fs::create_dir_all(to)?;
        fs::read_dir(from)?.try_for_each(|entry| {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))
        })
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(from, to).map(drop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data-folder")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn fill(dir: &Path) {
        fs::create_dir_all(dir.join("webview").join("Default")).unwrap();
        fs::write(dir.join("webview").join("Default").join("Cookies"), b"c").unwrap();
        fs::write(dir.join("session.bin"), b"s").unwrap();
        fs::write(dir.join("unrelated.txt"), b"u").unwrap();
    }

    #[test]
    fn copy_then_remove_moves_only_app_files() {
        let root = test_dir("move");
        let (from, to) = (root.join("old"), root.join("new"));
        fill(&from);
        copy(&from, &to).unwrap();
        assert_eq!(fs::read(to.join("session.bin")).unwrap(), b"s");
        assert!(to.join("webview").join("Default").join("Cookies").exists());
        assert!(!to.join("unrelated.txt").exists());
        assert!(from.join("session.bin").exists());
        remove(&from).unwrap();
        assert!(!from.join("session.bin").exists());
        assert!(!from.join("webview").exists());
        assert!(from.join("unrelated.txt").exists());
    }

    #[test]
    fn refuses_a_folder_that_already_has_data() {
        let root = test_dir("occupied");
        let (from, to) = (root.join("old"), root.join("new"));
        fill(&from);
        fill(&to);
        assert!(matches!(copy(&from, &to), Err(MoveError::Occupied(_))));
    }

    #[test]
    fn refuses_nested_folders() {
        let root = test_dir("nested");
        fill(&root);
        assert!(matches!(
            copy(&root, &root.join("inner")),
            Err(MoveError::Nested)
        ));
        assert!(matches!(
            copy(&root.join("inner"), &root),
            Err(MoveError::Nested)
        ));
    }
}
