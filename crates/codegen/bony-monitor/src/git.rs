//! Read repository history via `git` CLI.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::catalog::CatalogSnapshot;
use crate::impact::{ChangeImpact, analyze};

#[derive(Debug, Clone, Serialize)]
pub struct FileStat {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChangeEntry {
    pub sha: String,
    pub short_sha: String,
    pub subject: String,
    pub body: String,
    pub author: String,
    pub email: String,
    pub date: String,
    pub additions: u32,
    pub deletions: u32,
    pub files: Vec<FileStat>,
    pub impact: ChangeImpact,
}

pub fn find_repo_root(start: &Path) -> Result<PathBuf> {
    let mut cur = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    loop {
        if cur.join(".git").exists() {
            return Ok(cur);
        }
        if !cur.pop() {
            bail!(
                "not inside a git repository (started at {})",
                start.display()
            );
        }
    }
}

pub fn list_changes(
    repo: &Path,
    limit: usize,
    catalog: &CatalogSnapshot,
) -> Result<Vec<ChangeEntry>> {
    let output = Command::new("git")
        .args([
            "-C",
            repo.to_str().context("repo path utf-8")?,
            "log",
            &format!("-n{limit}"),
            "--date=iso-strict",
            // Leading RS so each record is: header fields, then numstat lines.
            "--pretty=format:%x1e%H%x1f%h%x1f%s%x1f%b%x1f%an%x1f%ae%x1f%ad",
            "--numstat",
        ])
        .output()
        .context("failed to run git log")?;

    if !output.status.success() {
        bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_git_log(&text, catalog)
}

/// Analyze the two mutable Git layers independently so the dashboard updates
/// before a commit exists.
pub fn working_changes(repo: &Path, catalog: &CatalogSnapshot) -> Result<Vec<ChangeEntry>> {
    let mut out = Vec::new();
    if let Some(staged) = working_change(repo, true, catalog)? {
        out.push(staged);
    }
    if let Some(unstaged) = working_change(repo, false, catalog)? {
        out.push(unstaged);
    }
    Ok(out)
}

fn working_change(
    repo: &Path,
    staged: bool,
    catalog: &CatalogSnapshot,
) -> Result<Option<ChangeEntry>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(["diff", "--numstat"]);
    if staged {
        command.arg("--cached");
    }
    let output = command.output().context("failed to read working diff")?;
    if !output.status.success() {
        bail!(
            "git diff failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut files = parse_numstat(&String::from_utf8_lossy(&output.stdout));
    if !staged {
        let untracked = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()
            .context("failed to list untracked files")?;
        for raw in untracked
            .stdout
            .split(|b| *b == 0)
            .filter(|v| !v.is_empty())
        {
            let path = String::from_utf8_lossy(raw).into_owned();
            if !files.iter().any(|f| f.path == path) {
                files.push(FileStat {
                    path,
                    additions: 0,
                    deletions: 0,
                });
            }
        }
    }
    if files.is_empty() {
        return Ok(None);
    }
    let additions = files.iter().map(|f| f.additions).sum();
    let deletions = files.iter().map(|f| f.deletions).sum();
    let paths: Vec<_> = files.iter().map(|f| f.path.clone()).collect();
    let short_sha = if staged { "staged" } else { "unstaged" };
    let subject = if staged {
        "当前已暂存修改"
    } else {
        "当前未暂存修改"
    };
    Ok(Some(ChangeEntry {
        sha: short_sha.into(),
        short_sha: short_sha.into(),
        subject: subject.into(),
        body: String::new(),
        author: "Working tree".into(),
        email: String::new(),
        date: String::new(),
        additions,
        deletions,
        files,
        impact: analyze(catalog, subject, "", &paths),
    }))
}

fn parse_numstat(text: &str) -> Vec<FileStat> {
    text.lines()
        .filter_map(|line| {
            let mut columns = line.splitn(3, '\t');
            let additions = columns.next()?.parse().unwrap_or(0);
            let deletions = columns.next()?.parse().unwrap_or(0);
            let path = columns.next()?.to_string();
            Some(FileStat {
                path,
                additions,
                deletions,
            })
        })
        .collect()
}

pub fn change_detail(repo: &Path, sha: &str, catalog: &CatalogSnapshot) -> Result<ChangeEntry> {
    let list = list_changes(repo, 200, catalog)?;
    list.into_iter()
        .find(|c| c.sha.starts_with(sha) || c.short_sha == sha)
        .with_context(|| format!("commit not found: {sha}"))
}

fn parse_git_log(text: &str, catalog: &CatalogSnapshot) -> Result<Vec<ChangeEntry>> {
    let mut entries = Vec::new();
    for chunk in text.split('\x1e') {
        let chunk = chunk.trim();
        if chunk.is_empty() {
            continue;
        }
        // Body may contain newlines, so split the whole chunk on unit separator.
        let parts: Vec<&str> = chunk.split('\x1f').collect();
        if parts.len() < 7 {
            continue;
        }
        let sha = parts[0].trim().to_string();
        let short_sha = parts[1].to_string();
        let subject = parts[2].to_string();
        let body = parts[3].replace('\r', "");
        let author = parts[4].to_string();
        let email = parts[5].to_string();

        let mut date_and_stats = parts[6].lines();
        let date = date_and_stats.next().unwrap_or("").trim().to_string();

        let mut files = Vec::new();
        let mut additions = 0u32;
        let mut deletions = 0u32;
        for line in date_and_stats {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 3 {
                continue;
            }
            let add = cols[0].parse::<u32>().unwrap_or(0);
            let del = cols[1].parse::<u32>().unwrap_or(0);
            additions += add;
            deletions += del;
            files.push(FileStat {
                path: cols[2].to_string(),
                additions: add,
                deletions: del,
            });
        }

        let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();
        let impact = analyze(catalog, &subject, &body, &paths);

        entries.push(ChangeEntry {
            sha,
            short_sha,
            subject,
            body,
            author,
            email,
            date,
            additions,
            deletions,
            files,
            impact,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_single_commit_chunk() {
        let sample = "\x1e\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1fbbbbbbb\x1fAdd feature\x1fImpact: better UI\nRisk: none\x1fAda\x1fada@ex.com\x1f2026-07-17T00:00:00+08:00\n\
10\t2\tcrates/codegen/bony-build/src/app.rs\n";
        let catalog = CatalogSnapshot::empty();
        let entries = parse_git_log(sample, &catalog).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].subject, "Add feature");
        assert_eq!(entries[0].additions, 10);
        assert!(entries[0].impact.tags.iter().any(|t| t == "desktop"));
    }

    #[test]
    fn parses_binary_and_text_numstat() {
        let files = parse_numstat("12\t3\tsrc/app.rs\n-\t-\timage.png\n");
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].additions, 12);
        assert_eq!(files[1].additions, 0);
    }
}
