//! Read-mostly Git integration and isolated task worktrees.

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

#[derive(Debug, Clone)]
pub struct FileChange {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub kind: ChangeKind,
    pub staged: bool,
}

#[derive(Debug, Clone)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: String,
}

pub struct GitWorkspaceService;

impl GitWorkspaceService {
    pub fn primary_repo_root(path: &Path) -> Result<Option<PathBuf>, String> {
        main_repo_root(path)
    }

    pub fn repo_root(path: &Path) -> Result<Option<PathBuf>, String> {
        let out = git(path, ["rev-parse", "--show-toplevel"])?;
        Ok(out
            .status
            .success()
            .then(|| PathBuf::from(out.stdout.trim())))
    }

    pub fn changes(path: &Path) -> Result<Vec<FileChange>, String> {
        let out = git(path, ["status", "--porcelain=v1", "-z"])?;
        if !out.status.success() {
            return Err(out.stderr);
        }
        parse_porcelain_z(out.stdout.as_bytes())
    }

    pub fn diff(path: &Path, file: Option<&Path>, staged: bool) -> Result<String, String> {
        let mut cmd = Command::new("git");
        cmd.current_dir(path)
            .arg("diff")
            .arg("--no-ext-diff")
            .arg("--no-color");
        if staged {
            cmd.arg("--cached");
        }
        if let Some(file) = file {
            cmd.arg("--").arg(file);
        }
        let out = cmd.output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    pub fn create_worktree(project: &Path, task_id: &str, title: &str) -> Result<Worktree, String> {
        let root = main_repo_root(project)?.ok_or_else(|| "目录不是 Git 仓库".to_string())?;
        let slug = slug(title);
        let short = &task_id[..task_id.len().min(8)];
        let branch = format!("codex/{slug}-{short}");
        let parent = worktree_parent(&root)?;
        let path = parent.join(short);
        let branch_ref = format!("refs/heads/{branch}");
        let exists = Command::new("git")
            .current_dir(&root)
            .args(["show-ref", "--verify", "--quiet", &branch_ref])
            .status()
            .map_err(|e| e.to_string())?
            .success();
        if exists {
            return Err(format!("任务分支已存在：{branch}"));
        }
        let out = Command::new("git")
            .current_dir(&root)
            .args(["-c", "core.longpaths=true", "worktree", "add", "-b"])
            .arg(&branch)
            .arg(&path)
            .arg("HEAD")
            .output()
            .map_err(|e| e.to_string())?;
        if !out.status.success() {
            rollback_failed_worktree(&root, &parent, &path, &branch);
            return Err(String::from_utf8_lossy(&out.stderr).into_owned());
        }
        Ok(Worktree { path, branch })
    }

    pub fn stage(path: &Path, file: &Path) -> Result<(), String> {
        git_write(path, ["add"], Some(file))
    }
    pub fn unstage(path: &Path, file: &Path) -> Result<(), String> {
        git_write(path, ["restore", "--staged"], Some(file))
    }
}

/// Resolve the primary checkout even when `project` is itself a linked worktree.
fn main_repo_root(project: &Path) -> Result<Option<PathBuf>, String> {
    let out = git(
        project,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(PathBuf::from(out.stdout.trim())
        .parent()
        .map(Path::to_path_buf))
}

fn worktree_parent(root: &Path) -> Result<PathBuf, String> {
    let fallback = root.parent().unwrap_or(root).join(".bwt");
    #[cfg(target_os = "windows")]
    let candidates = {
        let drive = root
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .unwrap_or_else(|| "C:".into());
        vec![PathBuf::from(format!(r"{drive}\tmp\bwt")), fallback]
    };
    #[cfg(not(target_os = "windows"))]
    let candidates = vec![fallback];

    for candidate in candidates {
        if std::fs::create_dir_all(&candidate).is_ok() {
            return Ok(candidate);
        }
    }
    Err("无法创建 worktree 根目录".into())
}

fn rollback_failed_worktree(root: &Path, managed_parent: &Path, path: &Path, branch: &str) {
    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "remove", "--force"])
        .arg(path)
        .status();
    let _ = Command::new("git")
        .current_dir(root)
        .args(["worktree", "prune"])
        .status();
    if path.parent() == Some(managed_parent) && path.is_dir() {
        let _ = std::fs::remove_dir_all(path);
    }
    let _ = Command::new("git")
        .current_dir(root)
        .args(["branch", "-D", branch])
        .status();
}

struct GitOutput {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}
fn git<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<GitOutput, String> {
    let out = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    Ok(GitOutput {
        status: out.status,
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    })
}
fn git_write<const N: usize>(
    cwd: &Path,
    args: [&str; N],
    file: Option<&Path>,
) -> Result<(), String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(cwd).args(args);
    if let Some(file) = file {
        cmd.arg("--").arg(file);
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn parse_porcelain_z(bytes: &[u8]) -> Result<Vec<FileChange>, String> {
    let fields: Vec<&[u8]> = bytes.split(|b| *b == 0).filter(|s| !s.is_empty()).collect();
    let mut result = Vec::new();
    let mut i = 0;
    while i < fields.len() {
        let field = String::from_utf8_lossy(fields[i]);
        if field.len() < 4 {
            return Err("无效的 git status 输出".into());
        }
        let code = &field[..2];
        let path = PathBuf::from(&field[3..]);
        let kind = if code == "??" {
            ChangeKind::Untracked
        } else if code.contains('U') || code == "AA" || code == "DD" {
            ChangeKind::Conflicted
        } else if code.contains('R') {
            ChangeKind::Renamed
        } else if code.contains('A') {
            ChangeKind::Added
        } else if code.contains('D') {
            ChangeKind::Deleted
        } else {
            ChangeKind::Modified
        };
        let old_path = if kind == ChangeKind::Renamed && i + 1 < fields.len() {
            i += 1;
            Some(PathBuf::from(String::from_utf8_lossy(fields[i]).as_ref()))
        } else {
            None
        };
        let staged = code
            .as_bytes()
            .first()
            .is_some_and(|c| *c != b' ' && *c != b'?');
        result.push(FileChange {
            path,
            old_path,
            kind,
            staged,
        });
        i += 1;
    }
    Ok(result)
}

fn slug(value: &str) -> String {
    let s: String = value
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let s = s
        .split('-')
        .filter(|p| !p.is_empty())
        .take(5)
        .collect::<Vec<_>>()
        .join("-");
    if s.is_empty() { "task".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_changes() {
        let rows = parse_porcelain_z(b" M src/a.rs\0?? new.txt\0R  next.rs\0old.rs\0").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].kind, ChangeKind::Untracked);
        assert_eq!(rows[2].old_path.as_deref(), Some(Path::new("old.rs")));
    }
    #[test]
    fn safe_slug() {
        assert_eq!(slug("Fix: login flow!"), "fix-login-flow");
        assert_eq!(slug("中文"), "task");
    }

    #[test]
    fn creates_isolated_worktree_and_reads_changes() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .current_dir(dir.path())
                .args(args)
                .output()
                .unwrap()
        };
        if !run(&["init"]).status.success() {
            return;
        }
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "Bony Test"]);
        std::fs::write(dir.path().join("README.md"), "hello").unwrap();
        assert!(run(&["add", "README.md"]).status.success());
        assert!(run(&["commit", "-m", "init"]).status.success());

        let worktree = GitWorkspaceService::create_worktree(
            dir.path(),
            &uuid::Uuid::new_v4().to_string(),
            "Test task",
        )
        .unwrap();
        assert!(worktree.path.is_dir());
        assert!(worktree.branch.starts_with("codex/test-task-"));
        assert_eq!(
            main_repo_root(&worktree.path).unwrap(),
            Some(dir.path().to_path_buf())
        );
        std::fs::write(worktree.path.join("README.md"), "changed").unwrap();
        let changes = GitWorkspaceService::changes(&worktree.path).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
        let _ = Command::new("git")
            .current_dir(dir.path())
            .args(["worktree", "remove", "--force"])
            .arg(&worktree.path)
            .status();
    }
}
