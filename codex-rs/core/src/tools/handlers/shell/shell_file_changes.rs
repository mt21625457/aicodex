use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::Path;
use std::path::PathBuf;

use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::FileSystemSandboxContext;
use codex_protocol::protocol::FileChange;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_path_uri::PathUri;

const MAX_SCANNED_DIRS: usize = 1024;
const MAX_SCANNED_FILES: usize = 20_000;
const MAX_DEPTH: usize = 24;

#[derive(Debug)]
pub(crate) struct ShellFileSnapshot {
    files: BTreeMap<PathBuf, ShellFileEntry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ShellFileEntry {
    created_at_ms: i64,
    modified_at_ms: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotLimit {
    DirectoryCount,
    FileCount,
    Depth,
}

pub(crate) async fn snapshot_shell_files(
    fs: &dyn ExecutorFileSystem,
    root: &AbsolutePathBuf,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Option<ShellFileSnapshot> {
    match try_snapshot_shell_files(fs, root, sandbox).await {
        Ok(snapshot) => Some(snapshot),
        Err(limit) => {
            tracing::debug!(?limit, cwd = %root.as_path().display(), "skipping shell file change detection");
            None
        }
    }
}

async fn try_snapshot_shell_files(
    fs: &dyn ExecutorFileSystem,
    root: &AbsolutePathBuf,
    sandbox: Option<&FileSystemSandboxContext>,
) -> Result<ShellFileSnapshot, SnapshotLimit> {
    let mut files = BTreeMap::new();
    let mut dirs_scanned = 0usize;
    let mut pending = VecDeque::from([(root.clone(), PathBuf::new(), 0usize)]);

    while let Some((abs_dir, rel_dir, depth)) = pending.pop_front() {
        if depth > MAX_DEPTH {
            return Err(SnapshotLimit::Depth);
        }
        dirs_scanned += 1;
        if dirs_scanned > MAX_SCANNED_DIRS {
            return Err(SnapshotLimit::DirectoryCount);
        }

        let abs_dir_uri = PathUri::from_abs_path(&abs_dir);
        let Ok(mut entries) = fs.read_directory(&abs_dir_uri, sandbox).await else {
            continue;
        };
        entries.sort_by(|left, right| left.file_name.cmp(&right.file_name));

        for entry in entries {
            let name = entry.file_name;
            if name == "." || name == ".." {
                continue;
            }

            let rel_path = rel_dir.join(&name);
            let abs_path = abs_dir.join(&name);

            if entry.is_directory {
                if should_skip_dir(&name) {
                    continue;
                }
                let abs_path_uri = PathUri::from_abs_path(&abs_path);
                let Ok(metadata) = fs.get_metadata(&abs_path_uri, sandbox).await else {
                    continue;
                };
                if metadata.is_symlink {
                    continue;
                }
                pending.push_back((abs_path, rel_path, depth + 1));
                continue;
            }

            if !entry.is_file {
                continue;
            }
            if files.len() >= MAX_SCANNED_FILES {
                return Err(SnapshotLimit::FileCount);
            }
            let abs_path_uri = PathUri::from_abs_path(&abs_path);
            let Ok(metadata) = fs.get_metadata(&abs_path_uri, sandbox).await else {
                continue;
            };
            if !metadata.is_file || metadata.is_symlink {
                continue;
            }
            files.insert(
                normalize_snapshot_path(&rel_path),
                ShellFileEntry {
                    created_at_ms: metadata.created_at_ms,
                    modified_at_ms: metadata.modified_at_ms,
                },
            );
        }
    }

    Ok(ShellFileSnapshot { files })
}

pub(crate) fn diff_shell_snapshots(
    before: &ShellFileSnapshot,
    after: &ShellFileSnapshot,
) -> HashMap<PathBuf, FileChange> {
    let mut changes = HashMap::new();

    for (path, before_entry) in &before.files {
        match after.files.get(path) {
            Some(after_entry) if before_entry == after_entry => {}
            Some(_) => {
                changes.insert(
                    path.clone(),
                    FileChange::Update {
                        unified_diff: String::new(),
                        move_path: None,
                    },
                );
            }
            None => {
                changes.insert(
                    path.clone(),
                    FileChange::Delete {
                        content: String::new(),
                    },
                );
            }
        }
    }

    for path in after.files.keys() {
        if !before.files.contains_key(path) {
            changes.insert(
                path.clone(),
                FileChange::Add {
                    content: String::new(),
                },
            );
        }
    }

    changes
}

fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git"
            | ".hg"
            | ".svn"
            | ".cache"
            | ".mypy_cache"
            | ".next"
            | ".pytest_cache"
            | ".ruff_cache"
            | ".turbo"
            | ".venv"
            | "__pycache__"
            | "build"
            | "coverage"
            | "dist"
            | "node_modules"
            | "target"
            | "venv"
    )
}

fn normalize_snapshot_path(path: &Path) -> PathBuf {
    path.components().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_shell_snapshots_detects_add_update_and_delete() {
        let before = ShellFileSnapshot {
            files: BTreeMap::from([
                (
                    PathBuf::from("deleted.txt"),
                    ShellFileEntry {
                        created_at_ms: 1,
                        modified_at_ms: 1,
                    },
                ),
                (
                    PathBuf::from("updated.txt"),
                    ShellFileEntry {
                        created_at_ms: 1,
                        modified_at_ms: 1,
                    },
                ),
            ]),
        };
        let after = ShellFileSnapshot {
            files: BTreeMap::from([
                (
                    PathBuf::from("added.txt"),
                    ShellFileEntry {
                        created_at_ms: 2,
                        modified_at_ms: 2,
                    },
                ),
                (
                    PathBuf::from("updated.txt"),
                    ShellFileEntry {
                        created_at_ms: 1,
                        modified_at_ms: 3,
                    },
                ),
            ]),
        };

        let changes = diff_shell_snapshots(&before, &after);

        assert!(matches!(
            changes.get(Path::new("added.txt")),
            Some(FileChange::Add { .. })
        ));
        assert!(matches!(
            changes.get(Path::new("updated.txt")),
            Some(FileChange::Update { .. })
        ));
        assert!(matches!(
            changes.get(Path::new("deleted.txt")),
            Some(FileChange::Delete { .. })
        ));
    }
}
