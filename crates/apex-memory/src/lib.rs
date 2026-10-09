//! Durable storage for tasks, events and scoped memory.
//!
//! SQLite is used for the local-first deployment. Every method is synchronous
//! and short-lived; access is serialised by an internal mutex so the store can
//! be shared across async tasks without holding a lock across an await point.
//!
//! The repository-style API here is deliberately provider-agnostic in shape so
//! a PostgreSQL backend can be introduced for multi-user deployments later.

use std::path::Path;
use std::sync::Mutex;

use apex_core::error::{ApexError, Result};
use apex_protocol::{Event, EventKind, Task, TaskStatus};
use rusqlite::{params, Connection, OptionalExtension};

/// Scope of a memory note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryScope {
    /// Shared across tasks in one project.
    Project,
    /// Specific to one agent.
    Agent,
    /// Global user preference or lesson.
    User,
}

impl MemoryScope {
    fn as_str(self) -> &'static str {
        match self {
            MemoryScope::Project => "project",
            MemoryScope::Agent => "agent",
            MemoryScope::User => "user",
        }
    }
}

/// A stored memory note.
#[derive(Debug, Clone)]
pub struct MemoryNote {
    pub id: String,
    pub scope: String,
    pub project_root: Option<String>,
    pub agent_id: Option<String>,
    pub kind: String,
    pub content: String,
    pub created_at: String,
}

/// A SQLite-backed store.
pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// Open (or create) the database at `path`.
    pub fn open(path: &Path) -> Result<Store> {
        if let Some(parent) = path.parent() {
            apex_core::paths::ensure_dir(parent)?;
        }
        let conn = Connection::open(path)
            .map_err(|e| ApexError::Storage(format!("could not open {}: {e}", path.display())))?;
        Self::from_connection(conn)
    }

    /// Open an in-memory database (used by tests).
    pub fn open_in_memory() -> Result<Store> {
        let conn = Connection::open_in_memory()
            .map_err(|e| ApexError::Storage(format!("could not open in-memory database: {e}")))?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Store> {
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "foreign_keys", "ON").ok();
        let store = Store {
            conn: Mutex::new(conn),
        };
        store.migrate()?;
        Ok(store)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Create tables if they do not exist.
    pub fn migrate(&self) -> Result<()> {
        let conn = self.lock();
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                id             TEXT PRIMARY KEY,
                status         TEXT NOT NULL,
                project_root   TEXT NOT NULL,
                created_at     TEXT NOT NULL,
                updated_at     TEXT NOT NULL,
                data           TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
            CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_root);

            CREATE TABLE IF NOT EXISTS events (
                id         TEXT PRIMARY KEY,
                task_id    TEXT NOT NULL,
                seq        INTEGER NOT NULL,
                timestamp  TEXT NOT NULL,
                data       TEXT NOT NULL,
                UNIQUE(task_id, seq)
            );
            CREATE INDEX IF NOT EXISTS idx_events_task ON events(task_id, seq);

            CREATE TABLE IF NOT EXISTS memory_notes (
                id           TEXT PRIMARY KEY,
                scope        TEXT NOT NULL,
                project_root TEXT,
                agent_id     TEXT,
                kind         TEXT NOT NULL,
                content      TEXT NOT NULL,
                created_at   TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_notes_scope ON memory_notes(scope, project_root, agent_id);
            "#,
        )
        .map_err(|e| ApexError::Storage(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Insert a new task.
    pub fn create_task(&self, task: &Task) -> Result<()> {
        let data = serde_json::to_string(task)?;
        let conn = self.lock();
        conn.execute(
            "INSERT INTO tasks (id, status, project_root, created_at, updated_at, data)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                task.id,
                task.status.as_str(),
                task.project_root,
                task.created_at,
                task.updated_at,
                data
            ],
        )
        .map_err(|e| ApexError::Storage(format!("could not insert task: {e}")))?;
        Ok(())
    }

    /// Update an existing task.
    pub fn update_task(&self, task: &Task) -> Result<()> {
        let data = serde_json::to_string(task)?;
        let conn = self.lock();
        let changed = conn
            .execute(
                "UPDATE tasks SET status=?2, updated_at=?3, data=?4 WHERE id=?1",
                params![task.id, task.status.as_str(), task.updated_at, data],
            )
            .map_err(|e| ApexError::Storage(format!("could not update task: {e}")))?;
        if changed == 0 {
            return Err(ApexError::Storage(format!("task {} not found", task.id)));
        }
        Ok(())
    }

    /// Fetch a task by id.
    pub fn get_task(&self, id: &str) -> Result<Option<Task>> {
        let conn = self.lock();
        let data: Option<String> = conn
            .query_row("SELECT data FROM tasks WHERE id=?1", params![id], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| ApexError::Storage(format!("could not read task: {e}")))?;
        match data {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    /// List tasks, newest first.
    pub fn list_tasks(&self, limit: usize) -> Result<Vec<Task>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT data FROM tasks ORDER BY created_at DESC LIMIT ?1")
            .map_err(|e| ApexError::Storage(format!("could not prepare list: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |row| row.get::<_, String>(0))
            .map_err(|e| ApexError::Storage(format!("could not list tasks: {e}")))?;
        let mut tasks = Vec::new();
        for row in rows {
            let json = row.map_err(|e| ApexError::Storage(e.to_string()))?;
            tasks.push(serde_json::from_str(&json)?);
        }
        Ok(tasks)
    }

    /// Count tasks by status.
    pub fn count_tasks(&self) -> Result<usize> {
        let conn = self.lock();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(count as usize)
    }

    /// Count non-terminal tasks.
    pub fn count_active_tasks(&self) -> Result<usize> {
        let conn = self.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE status NOT IN ('completed','failed','cancelled')",
                [],
                |row| row.get(0),
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(count as usize)
    }

    fn next_seq(conn: &Connection, task_id: &str) -> Result<i64> {
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) + 1 FROM events WHERE task_id=?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(seq)
    }

    /// Append an event, assigning it the next sequence number.
    pub fn append_event(&self, task_id: &str, kind: EventKind) -> Result<Event> {
        let conn = self.lock();
        let seq = Self::next_seq(&conn, task_id)?;
        let event = Event {
            id: apex_core::new_id("evt"),
            task_id: task_id.to_string(),
            seq,
            timestamp: apex_core::now_rfc3339(),
            kind,
        };
        let data = serde_json::to_string(&event)?;
        conn.execute(
            "INSERT INTO events (id, task_id, seq, timestamp, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![event.id, task_id, seq, event.timestamp, data],
        )
        .map_err(|e| ApexError::Storage(format!("could not append event: {e}")))?;
        Ok(event)
    }

    /// Fetch events after `after_seq` (exclusive), ordered by sequence.
    pub fn events_after(&self, task_id: &str, after_seq: i64) -> Result<Vec<Event>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare("SELECT data FROM events WHERE task_id=?1 AND seq>?2 ORDER BY seq ASC")
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(params![task_id, after_seq], |row| row.get::<_, String>(0))
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        let mut events = Vec::new();
        for row in rows {
            let json = row.map_err(|e| ApexError::Storage(e.to_string()))?;
            events.push(serde_json::from_str(&json)?);
        }
        Ok(events)
    }

    /// The latest sequence number for a task (0 when it has no events).
    pub fn last_seq(&self, task_id: &str) -> Result<i64> {
        let conn = self.lock();
        let seq: i64 = conn
            .query_row(
                "SELECT COALESCE(MAX(seq), 0) FROM events WHERE task_id=?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(seq)
    }

    /// Count events for a task.
    pub fn event_count(&self, task_id: &str) -> Result<usize> {
        let conn = self.lock();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE task_id=?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(count as usize)
    }

    /// Store a scoped memory note.
    pub fn add_note(
        &self,
        scope: MemoryScope,
        project_root: Option<&str>,
        agent_id: Option<&str>,
        kind: &str,
        content: &str,
    ) -> Result<String> {
        let id = apex_core::new_id("note");
        let conn = self.lock();
        conn.execute(
            "INSERT INTO memory_notes (id, scope, project_root, agent_id, kind, content, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                id,
                scope.as_str(),
                project_root,
                agent_id,
                kind,
                content,
                apex_core::now_rfc3339()
            ],
        )
        .map_err(|e| ApexError::Storage(format!("could not add note: {e}")))?;
        Ok(id)
    }

    /// Retrieve recent notes for a scope/project/agent, newest first.
    pub fn notes(
        &self,
        scope: MemoryScope,
        project_root: Option<&str>,
        agent_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        let conn = self.lock();
        let mut stmt = conn
            .prepare(
                "SELECT id, scope, project_root, agent_id, kind, content, created_at
                 FROM memory_notes
                 WHERE scope=?1
                   AND (project_root IS ?2)
                   AND (agent_id IS ?3)
                 ORDER BY created_at DESC LIMIT ?4",
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        let rows = stmt
            .query_map(
                params![scope.as_str(), project_root, agent_id, limit as i64],
                |row| {
                    Ok(MemoryNote {
                        id: row.get(0)?,
                        scope: row.get(1)?,
                        project_root: row.get(2)?,
                        agent_id: row.get(3)?,
                        kind: row.get(4)?,
                        content: row.get(5)?,
                        created_at: row.get(6)?,
                    })
                },
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        let mut notes = Vec::new();
        for row in rows {
            notes.push(row.map_err(|e| ApexError::Storage(e.to_string()))?);
        }
        Ok(notes)
    }

    /// Delete events older than the newest `keep` for a task.
    pub fn trim_events(&self, task_id: &str, keep: usize) -> Result<usize> {
        let conn = self.lock();
        let total = self.event_count_locked(&conn, task_id)?;
        if total <= keep {
            return Ok(0);
        }
        let cutoff: i64 = conn
            .query_row(
                "SELECT seq FROM events WHERE task_id=?1 ORDER BY seq DESC LIMIT 1 OFFSET ?2",
                params![task_id, (keep.saturating_sub(1)) as i64],
                |row| row.get(0),
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        let removed = conn
            .execute(
                "DELETE FROM events WHERE task_id=?1 AND seq<?2",
                params![task_id, cutoff],
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(removed)
    }

    fn event_count_locked(&self, conn: &Connection, task_id: &str) -> Result<usize> {
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE task_id=?1",
                params![task_id],
                |row| row.get(0),
            )
            .map_err(|e| ApexError::Storage(e.to_string()))?;
        Ok(count as usize)
    }
}

/// Convenience: build a fresh task header.
pub fn new_task(
    objective: impl Into<String>,
    project_root: impl Into<String>,
    model: Option<String>,
    agent_id: Option<String>,
) -> Task {
    let now = apex_core::now_rfc3339();
    Task {
        id: apex_core::new_id("task"),
        objective: objective.into(),
        project_root: project_root.into(),
        model,
        agent_id,
        mode: Default::default(),
        status: TaskStatus::Pending,
        created_at: now.clone(),
        updated_at: now,
        started_at: None,
        finished_at: None,
        budget: Default::default(),
        usage: Default::default(),
        tool_calls: 0,
        steps: 0,
        repair_attempts: 0,
        summary: None,
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_roundtrip_and_events() {
        let store = Store::open_in_memory().unwrap();
        let task = new_task("do a thing", "/tmp/proj", Some("fake/model".into()), None);
        store.create_task(&task).unwrap();

        let fetched = store.get_task(&task.id).unwrap().unwrap();
        assert_eq!(fetched.objective, "do a thing");

        let e1 = store
            .append_event(
                &task.id,
                EventKind::Log {
                    level: "info".into(),
                    message: "hello".into(),
                },
            )
            .unwrap();
        let e2 = store
            .append_event(
                &task.id,
                EventKind::StatusChanged {
                    status: TaskStatus::Running,
                    reason: None,
                },
            )
            .unwrap();
        assert_eq!(e1.seq, 1);
        assert_eq!(e2.seq, 2);

        let events = store.events_after(&task.id, 0).unwrap();
        assert_eq!(events.len(), 2);
        let after = store.events_after(&task.id, 1).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].seq, 2);
    }

    #[test]
    fn note_scoping() {
        let store = Store::open_in_memory().unwrap();
        store
            .add_note(MemoryScope::Project, Some("/a"), None, "lesson", "use tabs")
            .unwrap();
        store
            .add_note(
                MemoryScope::Project,
                Some("/b"),
                None,
                "lesson",
                "use spaces",
            )
            .unwrap();
        let notes = store
            .notes(MemoryScope::Project, Some("/a"), None, 10)
            .unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].content, "use tabs");
    }
}
