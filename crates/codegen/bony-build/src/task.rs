//! Durable task metadata. ACP remains the source of truth for conversation bodies.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Draft,
    Running,
    WaitingApproval,
    Completed,
    Failed,
    Archived,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Archived => "archived",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "running" => Self::Running,
            "waiting_approval" => Self::WaitingApproval,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "archived" => Self::Archived,
            _ => Self::Draft,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Draft => "草稿",
            Self::Running => "运行中",
            Self::WaitingApproval => "等待审批",
            Self::Completed => "已完成",
            Self::Failed => "失败",
            Self::Archived => "已归档",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    ReadOnly,
    Ask,
    AllowEdits,
    FullControl,
}

impl PermissionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ask => "ask",
            Self::AllowEdits => "allow_edits",
            Self::FullControl => "full_control",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "read_only" => Self::ReadOnly,
            "allow_edits" => Self::AllowEdits,
            "full_control" => Self::FullControl,
            _ => Self::Ask,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentMode {
    Plan,
    Execute,
}

impl AgentMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Execute => "execute",
        }
    }

    fn parse(value: &str) -> Self {
        if value == "plan" {
            Self::Plan
        } else {
            Self::Execute
        }
    }
}

#[derive(Debug, Clone)]
pub struct TaskState {
    pub id: String,
    pub title: String,
    pub project_path: PathBuf,
    pub worktree_path: PathBuf,
    pub branch: Option<String>,
    pub session_id: Option<String>,
    pub model_id: String,
    pub permission_mode: PermissionMode,
    pub agent_mode: AgentMode,
    pub status: TaskStatus,
    pub isolated: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl TaskState {
    pub fn draft(project_path: PathBuf, model_id: String) -> Self {
        let now = unix_time();
        Self {
            id: Uuid::new_v4().to_string(),
            title: "新任务".into(),
            worktree_path: project_path.clone(),
            project_path,
            branch: None,
            session_id: None,
            model_id,
            permission_mode: PermissionMode::Ask,
            agent_mode: AgentMode::Execute,
            status: TaskStatus::Draft,
            isolated: false,
            created_at: now,
            updated_at: now,
        }
    }
}

pub trait TaskRepository {
    fn list(&self, include_archived: bool) -> Result<Vec<TaskState>, String>;
    fn get(&self, id: &str) -> Result<Option<TaskState>, String>;
    fn save(&self, task: &TaskState) -> Result<(), String>;
    fn delete(&self, id: &str) -> Result<(), String>;
}

pub struct SqliteTaskRepository {
    conn: Connection,
}

impl SqliteTaskRepository {
    pub fn open(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let conn = Connection::open(path).map_err(|e| e.to_string())?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS tasks (
               id TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               project_path TEXT NOT NULL,
               worktree_path TEXT NOT NULL,
               branch TEXT,
               session_id TEXT,
               model_id TEXT NOT NULL,
               permission_mode TEXT NOT NULL,
               agent_mode TEXT NOT NULL DEFAULT 'execute',
               status TEXT NOT NULL,
               isolated INTEGER NOT NULL DEFAULT 0,
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS tasks_updated ON tasks(updated_at DESC);",
        )
        .map_err(|e| e.to_string())?;
        Ok(Self { conn })
    }

    pub fn open_default() -> Result<Self, String> {
        Self::open(&data_dir().join("tasks.sqlite3"))
    }
}

impl TaskRepository for SqliteTaskRepository {
    fn list(&self, include_archived: bool) -> Result<Vec<TaskState>, String> {
        let sql = if include_archived {
            "SELECT id,title,project_path,worktree_path,branch,session_id,model_id,permission_mode,agent_mode,status,isolated,created_at,updated_at FROM tasks ORDER BY updated_at DESC"
        } else {
            "SELECT id,title,project_path,worktree_path,branch,session_id,model_id,permission_mode,agent_mode,status,isolated,created_at,updated_at FROM tasks WHERE status != 'archived' ORDER BY updated_at DESC"
        };
        let mut stmt = self.conn.prepare(sql).map_err(|e| e.to_string())?;
        let rows = stmt.query_map([], row_to_task).map_err(|e| e.to_string())?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())
    }

    fn get(&self, id: &str) -> Result<Option<TaskState>, String> {
        self.conn
            .query_row(
                "SELECT id,title,project_path,worktree_path,branch,session_id,model_id,permission_mode,agent_mode,status,isolated,created_at,updated_at FROM tasks WHERE id=?1",
                [id],
                row_to_task,
            )
            .optional()
            .map_err(|e| e.to_string())
    }

    fn save(&self, task: &TaskState) -> Result<(), String> {
        self.conn.execute(
            "INSERT INTO tasks(id,title,project_path,worktree_path,branch,session_id,model_id,permission_mode,agent_mode,status,isolated,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
             ON CONFLICT(id) DO UPDATE SET title=excluded.title,project_path=excluded.project_path,worktree_path=excluded.worktree_path,branch=excluded.branch,session_id=excluded.session_id,model_id=excluded.model_id,permission_mode=excluded.permission_mode,agent_mode=excluded.agent_mode,status=excluded.status,isolated=excluded.isolated,updated_at=excluded.updated_at",
            params![task.id, task.title, task.project_path.to_string_lossy(), task.worktree_path.to_string_lossy(), task.branch, task.session_id, task.model_id, task.permission_mode.as_str(), task.agent_mode.as_str(), task.status.as_str(), task.isolated, task.created_at, task.updated_at],
        ).map_err(|e| e.to_string())?;
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        self.conn
            .execute("DELETE FROM tasks WHERE id=?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskState> {
    Ok(TaskState {
        id: row.get(0)?,
        title: row.get(1)?,
        project_path: PathBuf::from(row.get::<_, String>(2)?),
        worktree_path: PathBuf::from(row.get::<_, String>(3)?),
        branch: row.get(4)?,
        session_id: row.get(5)?,
        model_id: row.get(6)?,
        permission_mode: PermissionMode::parse(&row.get::<_, String>(7)?),
        agent_mode: AgentMode::parse(&row.get::<_, String>(8)?),
        status: TaskStatus::parse(&row.get::<_, String>(9)?),
        isolated: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

pub fn data_dir() -> PathBuf {
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }
    .unwrap_or_else(std::env::temp_dir)
    .join("bony-build")
}

pub fn unix_time() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repository_round_trip_and_archive_filter() {
        let dir = tempfile::tempdir().unwrap();
        let repo = SqliteTaskRepository::open(&dir.path().join("tasks.db")).unwrap();
        let mut task = TaskState::draft(dir.path().to_path_buf(), "model".into());
        repo.save(&task).unwrap();
        assert_eq!(repo.list(false).unwrap().len(), 1);
        task.status = TaskStatus::Archived;
        task.title = "renamed".into();
        repo.save(&task).unwrap();
        assert!(repo.list(false).unwrap().is_empty());
        assert_eq!(repo.get(&task.id).unwrap().unwrap().title, "renamed");
        repo.delete(&task.id).unwrap();
        assert!(repo.get(&task.id).unwrap().is_none());
    }
}
