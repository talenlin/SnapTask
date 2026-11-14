use std::{
    collections::HashMap,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::retrieval::{self, AiContext};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub progress: u8,
    pub quadrant: String,
    pub status: String,
    pub today: bool,
    pub planned_date: Option<String>,
    pub updated_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AnalysisResult {
    Match {
        task: Task,
        next_progress: u8,
        analysis_source: String,
        confidence: Option<u8>,
    },
    Ambiguous {
        candidates: Vec<Task>,
        next_progress: u8,
        analysis_source: String,
        confidence: Option<u8>,
    },
    Temporary {
        suggested_title: String,
        suggested_detail: String,
        suggested_quadrant: Option<String>,
        suggested_phrase: Option<String>,
        next_progress: u8,
        analysis_source: String,
        confidence: Option<u8>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfirmInput {
    pub task_id: Option<String>,
    pub temporary_title: Option<String>,
    pub description: String,
    pub progress: u8,
    pub quadrant: Option<String>,
    pub today: bool,
    #[serde(default)]
    pub change_planned_date: bool,
    #[serde(default)]
    pub planned_date: Option<String>,
    #[serde(default)]
    pub expected_title: Option<String>,
    #[serde(default)]
    pub expected_progress: Option<u8>,
    #[serde(default)]
    pub expected_status: Option<String>,
    #[serde(default)]
    pub expected_planned_date_known: bool,
    #[serde(default)]
    pub expected_planned_date: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PendingWrite {
    pub id: i64,
    pub input: ConfirmInput,
    pub habit_json: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWriteInfo {
    pub id: i64,
    pub input: ConfirmInput,
    pub state: String,
    pub last_error: String,
    pub created_at: i64,
}

#[derive(Clone, Debug)]
pub struct SyncedWriteLog {
    pub id: i64,
    pub source: String,
    pub task_id: String,
    pub task_title: String,
    pub description: String,
    pub previous_progress: Option<u8>,
    pub next_progress: u8,
    pub previous_status: Option<String>,
    pub created_task: bool,
    pub observed_progress: Option<u8>,
    pub observed_status: Option<String>,
    pub previous_planned_date: Option<String>,
    pub next_planned_date: Option<String>,
    pub planned_date_changed: bool,
    pub observed_planned_date: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingWriteSummary {
    pub pending: u32,
    pub conflict: u32,
    pub uncertain: u32,
    pub editing: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResult {
    pub undone: bool,
    pub task_title: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Habit {
    pub id: i64,
    pub phrase: String,
    pub interpretation: String,
    pub task_id: String,
    pub kind: String,
    pub quadrant: String,
    pub confidence: u32,
    pub hit_count: u32,
    pub updated_at: i64,
}

pub struct Storage {
    connection: Connection,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default()
}

pub fn unix_now() -> i64 {
    now()
}

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        detail: row.get(2)?,
        progress: row.get::<_, i64>(3)? as u8,
        quadrant: row.get(4)?,
        status: row.get(5)?,
        today: row.get::<_, i64>(6)? != 0,
        planned_date: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

fn table_has_column(
    connection: &Connection,
    table: &str,
    column: &str,
) -> Result<bool, rusqlite::Error> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|name| name == column))
}

fn normalize_habit_key(value: &str) -> String {
    value
        .chars()
        .filter(|character| {
            character.is_alphanumeric() || ('\u{4e00}'..='\u{9fff}').contains(character)
        })
        .flat_map(char::to_lowercase)
        .collect()
}

fn habit_rephrasing_similarity(left: &str, right: &str) -> f32 {
    let left = left.chars().collect::<Vec<_>>();
    let right = right.chars().collect::<Vec<_>>();
    if left.len() < 6 || right.len() < 6 {
        return 0.0;
    }

    let mut left_pairs = HashMap::<(char, char), usize>::new();
    let mut right_pairs = HashMap::<(char, char), usize>::new();
    for pair in left.windows(2) {
        *left_pairs.entry((pair[0], pair[1])).or_default() += 1;
    }
    for pair in right.windows(2) {
        *right_pairs.entry((pair[0], pair[1])).or_default() += 1;
    }
    let common = left_pairs
        .iter()
        .map(|(pair, left_count)| {
            (*left_count).min(right_pairs.get(pair).copied().unwrap_or_default())
        })
        .sum::<usize>();
    (2 * common) as f32 / (left.len() + right.len() - 2) as f32
}

impl Storage {
    pub fn open(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let connection = Connection::open(path)?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS tasks (
               id TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               detail TEXT NOT NULL DEFAULT '',
               progress INTEGER NOT NULL CHECK(progress BETWEEN 0 AND 100),
               quadrant TEXT NOT NULL,
               status TEXT NOT NULL,
               today INTEGER NOT NULL DEFAULT 0,
               planned_date TEXT,
               updated_at INTEGER NOT NULL
             );
             CREATE VIRTUAL TABLE IF NOT EXISTS task_search USING fts5(task_id UNINDEXED, title, detail);
             CREATE TABLE IF NOT EXISTS floating_hidden_tasks (
               connection_key TEXT NOT NULL,
               task_id TEXT NOT NULL,
               PRIMARY KEY(connection_key,task_id)
             );
             CREATE TABLE IF NOT EXISTS progress_logs (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               source TEXT NOT NULL DEFAULT 'wps',
               task_id TEXT NOT NULL,
               task_title TEXT NOT NULL,
               description TEXT NOT NULL,
               previous_progress INTEGER,
               next_progress INTEGER NOT NULL,
               previous_status TEXT,
               created_task INTEGER NOT NULL DEFAULT 0,
               sync_state TEXT NOT NULL DEFAULT 'pending',
               reverted_at INTEGER,
               observed_progress INTEGER,
               observed_status TEXT,
               previous_planned_date TEXT,
               next_planned_date TEXT,
               planned_date_changed INTEGER NOT NULL DEFAULT 0,
               observed_planned_date TEXT,
               created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS habits (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               phrase TEXT NOT NULL,
               interpretation TEXT NOT NULL,
               kind TEXT NOT NULL,
               confidence INTEGER NOT NULL DEFAULT 1,
               updated_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS app_settings (
               key TEXT PRIMARY KEY,
               value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pending_writes (
               id INTEGER PRIMARY KEY AUTOINCREMENT,
               input_json TEXT NOT NULL,
               habit_json TEXT,
               state TEXT NOT NULL DEFAULT 'pending',
               last_error TEXT NOT NULL DEFAULT '',
               created_at INTEGER NOT NULL,
               updated_at INTEGER NOT NULL
             );"
        )?;
        let has_source = table_has_column(&connection, "tasks", "source")?;
        if !has_source {
            connection.execute(
                "ALTER TABLE tasks ADD COLUMN source TEXT NOT NULL DEFAULT 'local'",
                [],
            )?;
            connection.execute(
                "UPDATE tasks SET source='local-demo' WHERE id LIKE 'task-%'",
                [],
            )?;
        }
        for (column, definition) in [
            ("task_id", "TEXT NOT NULL DEFAULT ''"),
            ("match_key", "TEXT NOT NULL DEFAULT ''"),
            ("hit_count", "INTEGER NOT NULL DEFAULT 0"),
            ("active", "INTEGER NOT NULL DEFAULT 1"),
            ("quadrant", "TEXT NOT NULL DEFAULT 'in'"),
        ] {
            if !table_has_column(&connection, "habits", column)? {
                connection.execute(
                    &format!("ALTER TABLE habits ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        for (column, definition) in [
            ("source", "TEXT NOT NULL DEFAULT 'wps'"),
            ("observed_progress", "INTEGER"),
            ("observed_status", "TEXT"),
            ("previous_planned_date", "TEXT"),
            ("next_planned_date", "TEXT"),
            ("planned_date_changed", "INTEGER NOT NULL DEFAULT 0"),
            ("observed_planned_date", "TEXT"),
        ] {
            if !table_has_column(&connection, "progress_logs", column)? {
                connection.execute(
                    &format!("ALTER TABLE progress_logs ADD COLUMN {column} {definition}"),
                    [],
                )?;
            }
        }
        if !table_has_column(&connection, "tasks", "planned_date")? {
            connection.execute("ALTER TABLE tasks ADD COLUMN planned_date TEXT", [])?;
        }
        connection.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS task_search_v2
               USING fts5(task_id UNINDEXED,title,detail,tokenize='trigram');
             CREATE VIRTUAL TABLE IF NOT EXISTS habit_search
               USING fts5(habit_id UNINDEXED,phrase,interpretation,tokenize='trigram');",
        )?;
        let storage = Self { connection };
        storage.seed_if_empty()?;
        storage.rebuild_retrieval_indexes()?;
        Ok(storage)
    }

    fn rebuild_retrieval_indexes(&self) -> Result<(), rusqlite::Error> {
        self.connection.execute_batch(
            "DELETE FROM task_search_v2;
             INSERT INTO task_search_v2(task_id,title,detail)
               SELECT id,title,detail FROM tasks;
             DELETE FROM habit_search;
             INSERT INTO habit_search(habit_id,phrase,interpretation)
               SELECT CAST(id AS TEXT),phrase,interpretation FROM habits WHERE active=1;",
        )
    }

    fn seed_if_empty(&self) -> Result<(), rusqlite::Error> {
        let count: i64 = self
            .connection
            .query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;
        if count > 0 {
            return Ok(());
        }
        let seed = [
            (
                "task-a",
                "A技术风险排查",
                "提前准备会议材料",
                0,
                "iu",
                1,
            ),
            (
                "task-b",
                "B技术资料整理",
                "核验关键资料",
                88,
                "iu",
                0,
            ),
            ("task-c", "C技术方案", "推进方案结构", 40, "un", 0),
            (
                "task-d",
                "D项目调研与规划",
                "输出需求文件",
                99,
                "in",
                0,
            ),
            ("task-e", "E技术报告", "输出需求文件并组织会议", 40, "in", 1),
        ];
        for (index, item) in seed.iter().enumerate() {
            let timestamp = now() - index as i64;
            self.connection.execute(
                "INSERT INTO tasks (id,title,detail,progress,quadrant,status,today,planned_date,updated_at,source)
                 VALUES (?1,?2,?3,?4,?5,'进行中',?6,NULL,?7,'local-demo')",
                params![item.0, item.1, item.2, item.3, item.4, item.5, timestamp],
            )?;
            self.connection.execute(
                "INSERT INTO task_search (task_id,title,detail) VALUES (?1,?2,?3)",
                params![item.0, item.1, item.2],
            )?;
        }
        Ok(())
    }

    pub fn list_tasks(&self) -> Result<Vec<Task>, String> {
        self.list_tasks_for_floating("")
    }

    pub fn list_tasks_for_floating(&self, connection_key: &str) -> Result<Vec<Task>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,title,detail,progress,quadrant,status,today,planned_date,updated_at
                 FROM tasks WHERE status != '已完成'
                   AND NOT EXISTS(SELECT 1 FROM floating_hidden_tasks h WHERE h.connection_key=?1 AND h.task_id=tasks.id)
                 ORDER BY CASE
                     WHEN planned_date IS NOT NULL AND planned_date < date('now','localtime') THEN 0
                     WHEN planned_date IS NOT NULL AND planned_date <= date('now','localtime','+3 day') THEN 1
                     WHEN planned_date IS NOT NULL AND planned_date <= date('now','localtime','+7 day') THEN 2
                     ELSE 3 END,
                   today DESC,
                   CASE quadrant WHEN 'iu' THEN 0 WHEN 'un' THEN 1 WHEN 'in' THEN 2 ELSE 3 END,
                   updated_at DESC LIMIT 5",
            )
            .map_err(|error| error.to_string())?;
        let tasks = statement
            .query_map([connection_key], row_to_task)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(tasks)
    }

    pub fn hide_floating_task(&self, key: &str, id: &str) -> Result<(), String> {
        self.connection.execute("INSERT OR IGNORE INTO floating_hidden_tasks(connection_key,task_id) VALUES(?1,?2)",params![key,id]).map_err(|e|e.to_string())?;
        Ok(())
    }
    pub fn floating_hidden_count(&self, key: &str) -> Result<i64, String> {
        self.connection.query_row("SELECT count(*) FROM floating_hidden_tasks h JOIN tasks t ON t.id=h.task_id WHERE h.connection_key=?1 AND t.status!='已完成'",[key],|r|r.get(0)).map_err(|e|e.to_string())
    }
    pub fn show_hidden_floating_tasks(&self, key: &str) -> Result<(), String> {
        self.connection.execute("DELETE FROM floating_hidden_tasks WHERE connection_key=?1",[key]).map_err(|e|e.to_string())?;
        Ok(())
    }

    pub fn get_setting(&self, key: &str) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT value FROM app_settings WHERE key=?1",
                [key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn set_setting(&self, key: &str, value: &str) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO app_settings(key,value) VALUES(?1,?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn remove_setting(&self, key: &str) -> Result<(), String> {
        self.connection
            .execute("DELETE FROM app_settings WHERE key=?1", [key])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn replace_wps_tasks(&mut self, tasks: &[Task]) -> Result<(), String> {
        self.replace_remote_tasks(tasks, "wps")
    }

    pub fn replace_remote_tasks(&mut self, tasks: &[Task], source: &str) -> Result<(), String> {
        if !["wps", "feishu"].contains(&source) {
            return Err("不支持的远程任务数据源".to_string());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM task_search WHERE task_id IN
                 (SELECT id FROM tasks WHERE source IN ('wps','feishu','local-demo'))",
                [],
            )
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "DELETE FROM tasks WHERE source IN ('wps','feishu','local-demo')",
                [],
            )
            .map_err(|error| error.to_string())?;
        for task in tasks {
            transaction
                .execute(
                    "INSERT INTO tasks
                     (id,title,detail,progress,quadrant,status,today,planned_date,updated_at,source)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                    params![
                        task.id,
                        task.title,
                        task.detail,
                        task.progress,
                        task.quadrant,
                        task.status,
                        task.today as i64,
                        task.planned_date,
                        task.updated_at,
                        source
                    ],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO task_search (task_id,title,detail) VALUES (?1,?2,?3)",
                    params![task.id, task.title, task.detail],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction
            .execute("DELETE FROM task_search_v2", [])
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "INSERT INTO task_search_v2(task_id,title,detail)
                 SELECT id,title,detail FROM tasks",
                [],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())
    }

    pub fn record_synced_wps_write(
        &self,
        input: &ConfirmInput,
        previous: Option<&Task>,
        task: &Task,
    ) -> Result<(), String> {
        self.record_synced_remote_write(input, previous, task, "wps")
    }

    pub fn record_synced_remote_write(
        &self,
        input: &ConfirmInput,
        previous: Option<&Task>,
        task: &Task,
        source: &str,
    ) -> Result<(), String> {
        if !["wps", "feishu"].contains(&source) {
            return Err("不支持的写入日志数据源".to_string());
        }
        self.connection
            .execute(
                "INSERT INTO progress_logs
                 (source,task_id,task_title,description,previous_progress,next_progress,previous_status,created_task,sync_state,observed_progress,observed_status,
                  previous_planned_date,next_planned_date,planned_date_changed,observed_planned_date,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'synced',?9,?10,?11,?12,?13,?14,?15)",
                params![
                    source,
                    task.id,
                    task.title,
                    input.description,
                    previous.map(|value| value.progress),
                    task.progress,
                    previous.map(|value| value.status.as_str()),
                    if previous.is_none() { 1 } else { 0 },
                    task.progress,
                    task.status,
                    previous.and_then(|value| value.planned_date.as_deref()),
                    task.planned_date,
                    input.change_planned_date as i64,
                    task.planned_date,
                    now()
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn record_uncertain_wps_write(
        &self,
        input: &ConfirmInput,
        previous: &Task,
        observed: Option<&Task>,
    ) -> Result<(), String> {
        self.record_uncertain_remote_write(input, previous, observed, "wps")
    }

    pub fn record_uncertain_remote_write(
        &self,
        input: &ConfirmInput,
        previous: &Task,
        observed: Option<&Task>,
        source: &str,
    ) -> Result<(), String> {
        if !["wps", "feishu"].contains(&source) {
            return Err("不支持的不确定写入数据源".to_string());
        }
        let task_id = input.task_id.as_deref().unwrap_or(&previous.id);
        let existing = self
            .connection
            .query_row(
                "SELECT id FROM progress_logs
                 WHERE source=?1 AND task_id=?2 AND previous_progress=?3 AND next_progress=?4
                   AND sync_state='uncertain' AND reverted_at IS NULL
                 ORDER BY id DESC LIMIT 1",
                params![source, task_id, previous.progress, input.progress],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(id) = existing {
            self.connection
                .execute(
                    "UPDATE progress_logs
                     SET observed_progress=?1,observed_status=?2,observed_planned_date=?3,description=?4,created_at=?5
                     WHERE id=?6",
                    params![
                        observed.map(|task| task.progress),
                        observed.map(|task| task.status.as_str()),
                        observed.and_then(|task| task.planned_date.as_deref()),
                        input.description,
                        now(),
                        id
                    ],
                )
                .map_err(|error| error.to_string())?;
            return Ok(());
        }
        self.connection
            .execute(
                "INSERT INTO progress_logs
                 (source,task_id,task_title,description,previous_progress,next_progress,previous_status,created_task,sync_state,observed_progress,observed_status,
                  previous_planned_date,next_planned_date,planned_date_changed,observed_planned_date,created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,0,'uncertain',?8,?9,?10,?11,?12,?13,?14)",
                params![
                    source,
                    task_id,
                    previous.title,
                    input.description,
                    previous.progress,
                    input.progress,
                    previous.status,
                    observed.map(|task| task.progress),
                    observed.map(|task| task.status.as_str()),
                    previous.planned_date,
                    input.planned_date,
                    input.change_planned_date as i64,
                    observed.and_then(|task| task.planned_date.as_deref()),
                    now()
                ],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn enqueue_pending_write(
        &self,
        input: &ConfirmInput,
        habit_json: Option<&str>,
        last_error: &str,
    ) -> Result<i64, String> {
        let input_json = serde_json::to_string(input).map_err(|error| error.to_string())?;
        let timestamp = now();
        self.connection
            .execute(
                "INSERT INTO pending_writes
                 (input_json,habit_json,state,last_error,created_at,updated_at)
                 VALUES (?1,?2,'pending',?3,?4,?4)",
                params![input_json, habit_json, last_error, timestamp],
            )
            .map_err(|error| error.to_string())?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn enqueue_uncertain_write(
        &self,
        input: &ConfirmInput,
        habit_json: Option<&str>,
        last_error: &str,
    ) -> Result<i64, String> {
        let input_json = serde_json::to_string(input).map_err(|error| error.to_string())?;
        let existing = self
            .connection
            .query_row(
                "SELECT id FROM pending_writes WHERE input_json=?1 AND state='uncertain' ORDER BY id DESC LIMIT 1",
                [&input_json],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        if let Some(id) = existing {
            self.connection
                .execute(
                    "UPDATE pending_writes SET habit_json=?1,last_error=?2,updated_at=?3 WHERE id=?4",
                    params![habit_json, last_error, now(), id],
                )
                .map_err(|error| error.to_string())?;
            return Ok(id);
        }
        let timestamp = now();
        self.connection
            .execute(
                "INSERT INTO pending_writes
                 (input_json,habit_json,state,last_error,created_at,updated_at)
                 VALUES (?1,?2,'uncertain',?3,?4,?4)",
                params![input_json, habit_json, last_error, timestamp],
            )
            .map_err(|error| error.to_string())?;
        Ok(self.connection.last_insert_rowid())
    }

    pub fn replace_pending_write(
        &self,
        id: i64,
        input: &ConfirmInput,
        habit_json: Option<&str>,
        last_error: &str,
    ) -> Result<(), String> {
        let input_json = serde_json::to_string(input).map_err(|error| error.to_string())?;
        let changed = self
            .connection
            .execute(
                "UPDATE pending_writes
                 SET input_json=?1,habit_json=?2,state='pending',last_error=?3,updated_at=?4
                 WHERE id=?5",
                params![input_json, habit_json, last_error, now(), id],
            )
            .map_err(|error| error.to_string())?;
        if changed == 0 {
            return Err("待处理记录不存在或已被移除".to_string());
        }
        Ok(())
    }

    pub fn list_pending_writes(&self) -> Result<Vec<PendingWrite>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,input_json,habit_json FROM pending_writes
                 WHERE state='pending' ORDER BY id ASC",
            )
            .map_err(|error| error.to_string())?;
        let writes = statement
            .query_map([], |row| {
                let input_json: String = row.get(1)?;
                let input = serde_json::from_str(&input_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        input_json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(PendingWrite {
                    id: row.get(0)?,
                    input,
                    habit_json: row.get(2)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(writes)
    }

    pub fn list_pending_write_items(&self) -> Result<Vec<PendingWriteInfo>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,input_json,state,last_error,created_at FROM pending_writes
                 ORDER BY created_at DESC,id DESC",
            )
            .map_err(|error| error.to_string())?;
        let items = statement
            .query_map([], |row| {
                let input_json: String = row.get(1)?;
                let input = serde_json::from_str(&input_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        input_json.len(),
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(PendingWriteInfo {
                    id: row.get(0)?,
                    input,
                    state: row.get(2)?,
                    last_error: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(items)
    }

    pub fn set_pending_write_state(
        &self,
        id: i64,
        state: &str,
        last_error: &str,
    ) -> Result<(), String> {
        if !["pending", "conflict", "uncertain", "editing"].contains(&state) {
            return Err("待同步状态无效".to_string());
        }
        self.connection
            .execute(
                "UPDATE pending_writes SET state=?1,last_error=?2,updated_at=?3 WHERE id=?4",
                params![state, last_error, now(), id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn delete_pending_write(&self, id: i64) -> Result<(), String> {
        self.connection
            .execute("DELETE FROM pending_writes WHERE id=?1", [id])
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn pending_write_summary(&self) -> Result<PendingWriteSummary, String> {
        let mut summary = PendingWriteSummary::default();
        let mut statement = self
            .connection
            .prepare("SELECT state,COUNT(*) FROM pending_writes GROUP BY state")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
            })
            .map_err(|error| error.to_string())?;
        for row in rows {
            let (state, count) = row.map_err(|error| error.to_string())?;
            match state.as_str() {
                "pending" => summary.pending = count,
                "conflict" => summary.conflict = count,
                "uncertain" => summary.uncertain = count,
                "editing" => summary.editing = count,
                _ => {}
            }
        }
        Ok(summary)
    }

    pub fn prepare_pending_write(&self, id: i64) -> Result<ConfirmInput, String> {
        let input_json = self
            .connection
            .query_row(
                "SELECT input_json FROM pending_writes WHERE id=?1",
                [id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "这条待同步记录已经不存在".to_string())?;
        let input = serde_json::from_str(&input_json).map_err(|error| error.to_string())?;
        self.set_pending_write_state(id, "editing", "等待用户重新分析")?;
        Ok(input)
    }

    pub fn latest_synced_write(&self) -> Result<Option<SyncedWriteLog>, String> {
        self.connection
            .query_row(
                "SELECT id,source,task_id,task_title,description,previous_progress,next_progress,previous_status,created_task,
                        observed_progress,observed_status,previous_planned_date,next_planned_date,
                        planned_date_changed,observed_planned_date
                 FROM progress_logs
                 WHERE reverted_at IS NULL AND sync_state IN ('synced','uncertain')
                 ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok(SyncedWriteLog {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        task_id: row.get(2)?,
                        task_title: row.get(3)?,
                        description: row.get(4)?,
                        previous_progress: row.get::<_, Option<i64>>(5)?.map(|value| value as u8),
                        next_progress: row.get::<_, i64>(6)? as u8,
                        previous_status: row.get(7)?,
                        created_task: row.get::<_, i64>(8)? != 0,
                        observed_progress: row.get::<_, Option<i64>>(9)?.map(|value| value as u8),
                        observed_status: row.get(10)?,
                        previous_planned_date: row.get(11)?,
                        next_planned_date: row.get(12)?,
                        planned_date_changed: row.get::<_, i64>(13)? != 0,
                        observed_planned_date: row.get(14)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    pub fn mark_synced_write_reverted(&self, id: i64) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE progress_logs SET reverted_at=?1,sync_state='reverted' WHERE id=?2",
                params![now(), id],
            )
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn resolve_uncertain_write(&self, task_id: &str, next_progress: u8) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare("SELECT id,input_json FROM pending_writes WHERE state='uncertain'")
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        drop(statement);
        for (id, input_json) in rows {
            let Ok(input) = serde_json::from_str::<ConfirmInput>(&input_json) else {
                continue;
            };
            if input.task_id.as_deref() == Some(task_id) && input.progress == next_progress {
                self.connection
                    .execute("DELETE FROM pending_writes WHERE id=?1", [id])
                    .map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub fn list_habits(&self) -> Result<Vec<Habit>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,phrase,interpretation,task_id,kind,quadrant,confidence,hit_count,updated_at
                 FROM habits WHERE active=1 ORDER BY updated_at DESC, id DESC",
            )
            .map_err(|error| error.to_string())?;
        let habits = statement
            .query_map([], |row| {
                Ok(Habit {
                    id: row.get(0)?,
                    phrase: row.get(1)?,
                    interpretation: row.get(2)?,
                    task_id: row.get(3)?,
                    kind: row.get(4)?,
                    quadrant: row.get(5)?,
                    confidence: row.get::<_, i64>(6)? as u32,
                    hit_count: row.get::<_, i64>(7)? as u32,
                    updated_at: row.get(8)?,
                })
            })
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(habits)
    }

    pub fn retrieve_ai_context(&self, description: &str) -> Result<AiContext, String> {
        retrieval::retrieve(&self.connection, description, 10, 5)
    }

    pub fn save_habit(&self, phrase: &str, task: &Task, kind: &str) -> Result<Habit, String> {
        let phrase = phrase.trim();
        let match_key = normalize_habit_key(phrase);
        if match_key.chars().count() < 2 {
            return Err("需要至少 2 个有效字符才能记住这个说法".to_string());
        }
        if !["alias", "correction"].contains(&kind) {
            return Err("习惯类型无效".to_string());
        }
        if kind == "correction" {
            self.connection
                .execute(
                    "UPDATE habits SET active=0,updated_at=?1
                     WHERE active=1 AND match_key=?2 AND task_id<>?3",
                    params![now(), match_key, task.id],
                )
                .map_err(|error| error.to_string())?;
        }
        let existing = self
            .connection
            .query_row(
                "SELECT id FROM habits WHERE active=1 AND match_key=?1 AND task_id=?2 AND kind=?3",
                params![match_key, task.id, kind],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let timestamp = now();
        let id = if let Some(id) = existing {
            self.connection
                .execute(
                    "UPDATE habits SET phrase=?1,interpretation=?2,confidence=confidence+1,updated_at=?3
                     WHERE id=?4",
                    params![phrase, task.title, timestamp, id],
                )
                .map_err(|error| error.to_string())?;
            id
        } else {
            self.connection
                .execute(
                    "INSERT INTO habits
                     (phrase,interpretation,kind,confidence,updated_at,task_id,match_key,hit_count,active)
                     VALUES (?1,?2,?3,1,?4,?5,?6,0,1)",
                    params![phrase, task.title, kind, timestamp, task.id, match_key],
                )
                .map_err(|error| error.to_string())?;
            self.connection.last_insert_rowid()
        };
        self.connection
            .execute(
                "DELETE FROM habit_search WHERE habit_id=?1",
                [id.to_string()],
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO habit_search(habit_id,phrase,interpretation)
                 SELECT CAST(id AS TEXT),phrase,interpretation FROM habits WHERE id=?1 AND active=1",
                [id],
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .query_row(
                "SELECT id,phrase,interpretation,task_id,kind,quadrant,confidence,hit_count,updated_at
                 FROM habits WHERE id=?1",
                [id],
                |row| {
                    Ok(Habit {
                        id: row.get(0)?,
                        phrase: row.get(1)?,
                        interpretation: row.get(2)?,
                        task_id: row.get(3)?,
                        kind: row.get(4)?,
                        quadrant: row.get(5)?,
                        confidence: row.get::<_, i64>(6)? as u32,
                        hit_count: row.get::<_, i64>(7)? as u32,
                        updated_at: row.get(8)?,
                    })
                },
            )
            .map_err(|error| error.to_string())
    }

    pub fn save_task_type_habit(
        &self,
        phrase: &str,
        title: &str,
        quadrant: &str,
    ) -> Result<Habit, String> {
        let phrase = phrase.trim();
        let title = title.trim();
        let match_key = normalize_habit_key(phrase);
        if match_key.chars().count() < 2 {
            return Err("任务类型关键词至少需要 2 个有效字符".to_string());
        }
        if title.is_empty() {
            return Err("任务类型名称不能为空".to_string());
        }
        if !["iu", "un", "in", "nn"].contains(&quadrant) {
            return Err("任务类型象限无效".to_string());
        }
        let timestamp = now();
        self.connection
            .execute(
                "UPDATE habits SET active=0,updated_at=?1
                 WHERE active=1 AND kind='task_type' AND match_key=?2 AND interpretation<>?3",
                params![timestamp, match_key, title],
            )
            .map_err(|error| error.to_string())?;
        let existing = self
            .connection
            .query_row(
                "SELECT id FROM habits
                 WHERE active=1 AND kind='task_type' AND match_key=?1 AND interpretation=?2",
                params![match_key, title],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let id = if let Some(id) = existing {
            self.connection
                .execute(
                    "UPDATE habits SET phrase=?1,quadrant=?2,confidence=confidence+1,updated_at=?3
                     WHERE id=?4",
                    params![phrase, quadrant, timestamp, id],
                )
                .map_err(|error| error.to_string())?;
            id
        } else {
            self.connection
                .execute(
                    "INSERT INTO habits
                     (phrase,interpretation,kind,confidence,updated_at,task_id,match_key,hit_count,active,quadrant)
                     VALUES (?1,?2,'task_type',1,?3,'',?4,0,1,?5)",
                    params![phrase, title, timestamp, match_key, quadrant],
                )
                .map_err(|error| error.to_string())?;
            self.connection.last_insert_rowid()
        };
        self.connection
            .execute(
                "DELETE FROM habit_search WHERE habit_id=?1",
                [id.to_string()],
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "INSERT INTO habit_search(habit_id,phrase,interpretation)
                 SELECT CAST(id AS TEXT),phrase,interpretation FROM habits WHERE id=?1 AND active=1",
                [id],
            )
            .map_err(|error| error.to_string())?;
        self.list_habits()?
            .into_iter()
            .find(|habit| habit.id == id)
            .ok_or_else(|| "保存任务类型习惯后无法回读".to_string())
    }

    pub fn delete_habit(&self, id: i64) -> Result<bool, String> {
        let changed = self
            .connection
            .execute(
                "UPDATE habits SET active=0,updated_at=?1 WHERE id=?2",
                params![now(), id],
            )
            .map_err(|error| error.to_string())?;
        self.connection
            .execute(
                "DELETE FROM habit_search WHERE habit_id=?1",
                [id.to_string()],
            )
            .map_err(|error| error.to_string())?;
        Ok(changed > 0)
    }

    fn match_habits(&self, description: &str, tasks: &[Task]) -> Result<Vec<Task>, String> {
        let description_key = normalize_habit_key(description);
        let habits = self.list_habits()?;
        let has_exact_match = habits.iter().any(|habit| {
            let key = normalize_habit_key(&habit.phrase);
            key.chars().count() >= 2 && description_key.contains(&key)
        });
        let mut matched = Vec::new();
        let mut matched_habit_ids = Vec::new();
        for habit in habits {
            let key = normalize_habit_key(&habit.phrase);
            let exact = key.chars().count() >= 2 && description_key.contains(&key);
            let close_rephrasing =
                !has_exact_match && habit_rephrasing_similarity(&key, &description_key) >= 0.72;
            if exact || close_rephrasing {
                let task_by_id = tasks.iter().find(|task| task.id == habit.task_id);
                let task = task_by_id.or_else(|| {
                    let same_title = tasks
                        .iter()
                        .filter(|task| task.title == habit.interpretation)
                        .collect::<Vec<_>>();
                    (same_title.len() == 1).then_some(same_title[0])
                });
                if let Some(task) = task {
                    if task_by_id.is_none() {
                        self.connection
                            .execute(
                                "UPDATE habits SET task_id=?1,updated_at=?2 WHERE id=?3",
                                params![task.id, now(), habit.id],
                            )
                            .map_err(|error| error.to_string())?;
                    }
                    if !matched.iter().any(|existing: &Task| existing.id == task.id) {
                        matched.push(task.clone());
                    }
                    matched_habit_ids.push(habit.id);
                }
            }
        }
        for id in matched_habit_ids {
            self.connection
                .execute(
                    "UPDATE habits SET hit_count=hit_count+1,updated_at=?1 WHERE id=?2",
                    params![now(), id],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(matched)
    }

    fn match_task_type_habit(&self, description: &str) -> Result<Option<Habit>, String> {
        let description_key = normalize_habit_key(description);
        let mut matches = self
            .list_habits()?
            .into_iter()
            .filter(|habit| habit.kind == "task_type")
            .filter(|habit| {
                let key = normalize_habit_key(&habit.phrase);
                key.chars().count() >= 2 && description_key.contains(&key)
            })
            .collect::<Vec<_>>();
        matches.sort_by(|left, right| {
            normalize_habit_key(&right.phrase)
                .chars()
                .count()
                .cmp(&normalize_habit_key(&left.phrase).chars().count())
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        let Some(habit) = matches.into_iter().next() else {
            return Ok(None);
        };
        self.connection
            .execute(
                "UPDATE habits SET hit_count=hit_count+1,updated_at=?1 WHERE id=?2",
                params![now(), habit.id],
            )
            .map_err(|error| error.to_string())?;
        Ok(Some(habit))
    }

    #[cfg(test)]
    pub fn analyze(&self, description: &str, progress: u8) -> Result<AnalysisResult, String> {
        self.analyze_with_intent(description, progress, "auto")
    }

    pub fn analyze_with_intent(
        &self,
        description: &str,
        progress: u8,
        intent: &str,
    ) -> Result<AnalysisResult, String> {
        let description = description.trim();
        if description.is_empty() || progress > 100 {
            return Err("请填写工作描述，并输入 0–100 的整数".to_string());
        }
        if !["auto", "update", "create"].contains(&intent) {
            return Err("分析方式无效".to_string());
        }
        let mut tasks = self.list_task_candidates(50)?;
        if intent == "update" {
            // Completed tasks stay out of daily suggestions, but a user who names
            // one explicitly must be able to reopen it or change its planned date.
            let description_key = normalize_habit_key(description);
            let mut statement = self
                .connection
                .prepare(
                    "SELECT id,title,detail,progress,quadrant,status,today,planned_date,updated_at
                 FROM tasks WHERE status = '已完成' ORDER BY updated_at DESC",
                )
                .map_err(|error| error.to_string())?;
            let completed = statement
                .query_map([], row_to_task)
                .map_err(|error| error.to_string())?;
            for task in completed {
                let task = task.map_err(|error| error.to_string())?;
                let title_key = normalized_task_title(&task.title);
                if !title_key.is_empty() && description_key.contains(&title_key) {
                    tasks.push(task);
                }
            }
        }
        if intent != "update" {
            if let Some(habit) = self.match_task_type_habit(description)? {
                return Ok(AnalysisResult::Temporary {
                    suggested_title: habit.interpretation,
                    suggested_detail: suggested_detail(description),
                    suggested_quadrant: Some(habit.quadrant),
                    suggested_phrase: Some(habit.phrase),
                    next_progress: progress,
                    analysis_source: "habit".to_string(),
                    confidence: Some(98),
                });
            }
        }
        if intent == "create" {
            return Ok(AnalysisResult::Temporary {
                suggested_title: suggested_title(description),
                suggested_detail: suggested_detail(description),
                suggested_quadrant: None,
                suggested_phrase: None,
                next_progress: progress,
                analysis_source: "local".to_string(),
                confidence: Some(0),
            });
        }
        if intent != "update" && description.contains("临时任务") {
            return Ok(AnalysisResult::Temporary {
                suggested_title: suggested_title(description),
                suggested_detail: suggested_detail(description),
                suggested_quadrant: None,
                suggested_phrase: None,
                next_progress: progress,
                analysis_source: "local".to_string(),
                confidence: Some(100),
            });
        }
        let description_key = normalize_habit_key(description);
        let mut exact = tasks
            .iter()
            .filter(|task| {
                let task_key = normalized_task_title(&task.title);
                !task_key.is_empty() && description_key.contains(&task_key)
            })
            .cloned()
            .collect::<Vec<_>>();
        if exact.len() == 1 {
            return Ok(AnalysisResult::Match {
                task: exact.remove(0),
                next_progress: progress,
                analysis_source: "local".to_string(),
                confidence: Some(100),
            });
        }
        if exact.len() > 1 {
            return Ok(AnalysisResult::Ambiguous {
                candidates: exact,
                next_progress: progress,
                analysis_source: "local".to_string(),
                confidence: Some(60),
            });
        }

        let habit_matches = self.match_habits(description, &tasks)?;
        if habit_matches.len() == 1 {
            return Ok(AnalysisResult::Match {
                task: habit_matches[0].clone(),
                next_progress: progress,
                analysis_source: "habit".to_string(),
                confidence: Some(98),
            });
        }
        if habit_matches.len() > 1 {
            return Ok(AnalysisResult::Ambiguous {
                candidates: habit_matches.into_iter().take(3).collect(),
                next_progress: progress,
                analysis_source: "habit".to_string(),
                confidence: Some(60),
            });
        }

        let aliases = [
            ("D项目", 100_u8),
            ("B技术", 90),
            ("C技术", 80),
            ("E技术", 70),
            ("A技术", 70),
        ];
        let mut ranked_matches = Vec::<(u8, Task)>::new();
        for (phrase, priority) in aliases {
            if !description.contains(phrase) {
                continue;
            }
            for task in tasks.iter().filter(|task| task.title.contains(phrase)) {
                if !ranked_matches
                    .iter()
                    .any(|(_, existing)| existing.id == task.id)
                {
                    ranked_matches.push((priority, task.clone()));
                }
            }
        }
        ranked_matches.sort_by(|left, right| right.0.cmp(&left.0));
        if ranked_matches.first().is_some_and(|first| {
            ranked_matches
                .get(1)
                .is_none_or(|second| first.0 > second.0)
        }) {
            return Ok(AnalysisResult::Match {
                task: ranked_matches[0].1.clone(),
                next_progress: progress,
                analysis_source: "local".to_string(),
                confidence: Some(95),
            });
        }
        let matches = ranked_matches
            .into_iter()
            .map(|(_, task)| task)
            .collect::<Vec<_>>();
        let candidates = matches;
        if candidates.is_empty() {
            if intent == "update" {
                Ok(AnalysisResult::Ambiguous {
                    candidates: ranked_update_candidates(description, &tasks),
                    next_progress: progress,
                    analysis_source: "local".to_string(),
                    confidence: Some(0),
                })
            } else {
                Ok(AnalysisResult::Temporary {
                    suggested_title: suggested_title(description),
                    suggested_detail: suggested_detail(description),
                    suggested_quadrant: None,
                    suggested_phrase: None,
                    next_progress: progress,
                    analysis_source: "local".to_string(),
                    confidence: Some(0),
                })
            }
        } else {
            Ok(AnalysisResult::Ambiguous {
                candidates,
                next_progress: progress,
                analysis_source: "local".to_string(),
                confidence: Some(50),
            })
        }
    }

    pub fn list_task_candidates(&self, limit: usize) -> Result<Vec<Task>, String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT id,title,detail,progress,quadrant,status,today,planned_date,updated_at
                 FROM tasks WHERE status != '已完成'
                 ORDER BY CASE
                     WHEN planned_date IS NOT NULL AND planned_date < date('now','localtime') THEN 0
                     WHEN planned_date IS NOT NULL AND planned_date <= date('now','localtime','+3 day') THEN 1
                     WHEN planned_date IS NOT NULL AND planned_date <= date('now','localtime','+7 day') THEN 2
                     ELSE 3 END,
                   today DESC,
                   CASE quadrant WHEN 'iu' THEN 0 WHEN 'un' THEN 1 WHEN 'in' THEN 2 ELSE 3 END,
                   updated_at DESC LIMIT ?1",
            )
            .map_err(|error| error.to_string())?;
        let tasks = statement
            .query_map([limit as i64], row_to_task)
            .map_err(|error| error.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        Ok(tasks)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn confirm(&mut self, input: ConfirmInput) -> Result<Task, String> {
        if input.description.trim().is_empty() || input.progress > 100 {
            return Err("写入内容无效".to_string());
        }
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let timestamp = now();
        let task = if let Some(task_id) = input.task_id {
            let previous = transaction
                .query_row(
                    "SELECT id,title,detail,progress,quadrant,status,today,planned_date,updated_at FROM tasks WHERE id=?1",
                    [&task_id],
                    row_to_task,
                )
                .map_err(|error| error.to_string())?;
            let status = match input.progress {
                0 => "未开始",
                100 => "已完成",
                _ => "进行中",
            };
            transaction
                .execute(
                    "UPDATE tasks SET progress=?1,status=?2,
                       planned_date=CASE WHEN ?3 THEN ?4 ELSE planned_date END,
                       updated_at=?5 WHERE id=?6",
                    params![
                        input.progress,
                        status,
                        input.change_planned_date,
                        input.planned_date,
                        timestamp,
                        task_id
                    ],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO progress_logs
                     (task_id,task_title,description,previous_progress,next_progress,previous_status,created_task,sync_state,
                      previous_planned_date,next_planned_date,planned_date_changed,created_at)
                     VALUES (?1,?2,?3,?4,?5,?6,0,'pending',?7,?8,?9,?10)",
                    params![
                        previous.id,
                        previous.title,
                        input.description,
                        previous.progress,
                        input.progress,
                        previous.status,
                        previous.planned_date,
                        input.planned_date,
                        input.change_planned_date as i64,
                        timestamp
                    ],
                )
                .map_err(|error| error.to_string())?;
            Task {
                progress: input.progress,
                status: status.to_string(),
                planned_date: if input.change_planned_date {
                    input.planned_date.clone()
                } else {
                    previous.planned_date.clone()
                },
                updated_at: timestamp,
                ..previous
            }
        } else {
            let title = input
                .temporary_title
                .unwrap_or_else(|| suggested_title(&input.description));
            let quadrant = input
                .quadrant
                .ok_or_else(|| "新增临时任务前请选择象限".to_string())?;
            if !["iu", "un", "in", "nn"].contains(&quadrant.as_str()) {
                return Err("任务象限无效".to_string());
            }
            let id = format!("temp-{timestamp}");
            let status = match input.progress {
                0 => "未开始",
                100 => "已完成",
                _ => "进行中",
            };
            let planned_date = input
                .change_planned_date
                .then(|| input.planned_date.clone())
                .flatten();
            transaction
                .execute(
                    "INSERT INTO tasks (id,title,detail,progress,quadrant,status,today,planned_date,updated_at)
                     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                    params![
                        id,
                        title,
                        input.description,
                        input.progress,
                        quadrant,
                        status,
                        input.today as i64,
                        planned_date,
                        timestamp
                    ],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO task_search (task_id,title,detail) VALUES (?1,?2,?3)",
                    params![id, title, input.description],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO task_search_v2 (task_id,title,detail) VALUES (?1,?2,?3)",
                    params![id, title, input.description],
                )
                .map_err(|error| error.to_string())?;
            transaction
                .execute(
                    "INSERT INTO progress_logs
                     (task_id,task_title,description,previous_progress,next_progress,previous_status,created_task,sync_state,
                      previous_planned_date,next_planned_date,planned_date_changed,created_at)
                     VALUES (?1,?2,?3,NULL,?4,NULL,1,'pending',NULL,?5,?6,?7)",
                    params![
                        id,
                        title,
                        input.description,
                        input.progress,
                        planned_date,
                        input.change_planned_date as i64,
                        timestamp
                    ],
                )
                .map_err(|error| error.to_string())?;
            Task {
                id,
                title,
                detail: input.description,
                progress: input.progress,
                quadrant,
                status: status.to_string(),
                today: input.today,
                planned_date,
                updated_at: timestamp,
            }
        };
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(task)
    }

    #[cfg(test)]
    pub fn undo_latest(&mut self) -> Result<UndoResult, String> {
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| error.to_string())?;
        let entry = transaction
            .query_row(
                "SELECT id,task_id,task_title,previous_progress,previous_status,created_task,
                        previous_planned_date,planned_date_changed
                 FROM progress_logs WHERE reverted_at IS NULL AND sync_state='pending'
                 ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, i64>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| error.to_string())?;
        let Some((
            log_id,
            task_id,
            title,
            previous_progress,
            previous_status,
            created_task,
            previous_planned_date,
            planned_date_changed,
        )) = entry
        else {
            return Ok(UndoResult {
                undone: false,
                task_title: None,
            });
        };
        if created_task != 0 {
            transaction
                .execute("DELETE FROM task_search WHERE task_id=?1", [&task_id])
                .map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM task_search_v2 WHERE task_id=?1", [&task_id])
                .map_err(|error| error.to_string())?;
            transaction
                .execute("DELETE FROM tasks WHERE id=?1", [&task_id])
                .map_err(|error| error.to_string())?;
        } else {
            transaction
                .execute(
                    "UPDATE tasks SET progress=?1,status=?2,
                     planned_date=CASE WHEN ?3 THEN ?4 ELSE planned_date END,
                     updated_at=?5 WHERE id=?6",
                    params![
                        previous_progress.unwrap_or_default(),
                        previous_status.unwrap_or_else(|| "进行中".to_string()),
                        planned_date_changed != 0,
                        previous_planned_date,
                        now(),
                        task_id
                    ],
                )
                .map_err(|error| error.to_string())?;
        }
        transaction
            .execute(
                "UPDATE progress_logs SET reverted_at=?1,sync_state='reverted' WHERE id=?2",
                params![now(), log_id],
            )
            .map_err(|error| error.to_string())?;
        transaction.commit().map_err(|error| error.to_string())?;
        Ok(UndoResult {
            undone: true,
            task_title: Some(title),
        })
    }
}

fn suggested_title(description: &str) -> String {
    let cleaned = strip_create_command(description)
        .replace("这是临时任务：", "")
        .replace("这是临时任务:", "")
        .replace("临时任务：", "")
        .replace("临时任务:", "");
    let title = cleaned
        .split(['，', '。', '；', ';'])
        .next()
        .unwrap_or("临时任务")
        .trim();
    title.chars().take(24).collect::<String>()
}

fn strip_create_command(value: &str) -> String {
    let mut cleaned = value.trim().to_string();
    for prefix in [
        "请帮我新建一个",
        "请新建一个",
        "帮我新建一个",
        "新建一个",
        "新增一个",
        "创建一个",
        "建立一个",
        "请帮我新建",
        "请新建",
        "帮我新建",
        "新建",
        "新增",
        "创建",
        "建立",
    ] {
        if let Some(rest) = cleaned.strip_prefix(prefix) {
            cleaned = rest.trim_start_matches(['：', ':', ' ', '，']).to_string();
            break;
        }
    }
    cleaned
}

fn normalized_task_title(value: &str) -> String {
    normalize_habit_key(&suggested_title(value))
}

fn suggested_detail(description: &str) -> String {
    let cleaned = strip_create_command(description);
    let detail = cleaned
        .split(['，', '。', '；', ';'])
        .filter(|part| {
            let part = part.trim();
            !part.starts_with("计划在") && !part.starts_with("预计在") && !part.starts_with("截止")
        })
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("；");
    let detail = detail.chars().take(120).collect::<String>();
    if normalize_habit_key(&detail) == normalized_task_title(description)
        && description != strip_create_command(description)
    {
        let subject = detail.trim_end_matches("任务").trim();
        if !subject.is_empty() {
            return format!("推进{subject}并记录结果");
        }
    }
    detail
}

fn overlap_score(left: &str, right: &str) -> usize {
    let left = normalize_habit_key(left).chars().collect::<Vec<_>>();
    let right = normalize_habit_key(right).chars().collect::<Vec<_>>();
    if left.len() < 2 || right.len() < 2 {
        return 0;
    }
    right
        .windows(2)
        .filter(|pair| left.windows(2).any(|candidate| candidate == *pair))
        .count()
}

fn ranked_update_candidates(description: &str, tasks: &[Task]) -> Vec<Task> {
    let mut ranked = tasks
        .iter()
        .map(|task| {
            let title = suggested_title(&task.title);
            let score =
                overlap_score(description, &title) * 4 + overlap_score(description, &task.detail);
            (score, task.updated_at, task.clone())
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));
    ranked
        .into_iter()
        .take(5)
        .map(|(_, _, task)| task)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release_table_snapshot(
        connection: &Connection,
        table: &str,
    ) -> Vec<Vec<rusqlite::types::Value>> {
        assert!(["tasks", "app_settings", "progress_logs", "pending_writes", "habits"].contains(&table));
        let mut statement = connection
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
            .expect("read fixed release table");
        let columns = statement.column_count();
        statement
            .query_map([], |row| (0..columns).map(|index| row.get(index)).collect())
            .expect("read release rows")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect release rows")
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "opens only a temporary copy of the private pre-release backup, never the formal database"]
    fn release_private_backup_reopen_preserves_existing_connection_and_24_tasks() {
        let backup = PathBuf::from(std::env::var_os("APPDATA").expect("Windows profile"))
            .join("app.snaptask.desktop/release-backups/0.1.59-20261002/project-track.sqlite3");
        assert!(backup.is_file(), "private release backup must already exist");
        let original = Connection::open_with_flags(
            &backup,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        ).expect("read backup without modifying it");
        let tables = ["tasks", "app_settings", "progress_logs", "pending_writes", "habits"];
        let before = tables.iter().map(|table| release_table_snapshot(&original, table))
            .collect::<Vec<_>>();
        assert_eq!(before[0].len(), 24, "expected release baseline task count");
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
        let temporary = backup.parent().expect("private backup directory")
            .join(format!("compatibility-check-{}-{unique}", std::process::id()));
        std::fs::create_dir(&temporary).expect("create unique private test directory");
        let copied_database = temporary.join("project-track.sqlite3");
        std::fs::copy(&backup, &copied_database).expect("copy private backup, never overwrite it");
        for _ in 0..2 {
            let reopened = Storage::open(copied_database.clone()).expect("new storage opens copied data");
            for (table, expected) in tables.iter().zip(&before) {
                // Do not use assert_eq!: a failure must not dump private rows or settings.
                assert!(release_table_snapshot(&reopened.connection, table) == *expected,
                    "release reopen changed protected table {table}");
            }
            assert!(reopened.get_setting("feishu.app_token").expect("read saved token")
                .as_deref() == Some("Example00000000000000000002"));
            assert!(reopened.get_setting("feishu.table_id").expect("read saved table")
                .as_deref() == Some("tbl0000000000006"));
            let integrity: String = reopened.connection.query_row(
                "PRAGMA integrity_check", [], |row| row.get(0)).expect("integrity check");
            assert!(integrity == "ok", "copied database integrity failed");
        }
        for (table, expected) in tables.iter().zip(&before) {
            assert!(release_table_snapshot(&original, table) == *expected,
                "read-only original backup changed");
        }
        drop(original);
        assert!(temporary.canonicalize().expect("resolve temporary path").parent()
            == backup.parent().map(|parent| parent.canonicalize().expect("resolve private directory")).as_deref());
        for filename in ["project-track.sqlite3", "project-track.sqlite3-wal", "project-track.sqlite3-shm"] {
            let file = temporary.join(filename);
            if file.exists() { std::fs::remove_file(file).expect("remove only generated test file"); }
        }
        std::fs::remove_dir(&temporary).expect("remove empty generated directory");
        println!("RELEASE_BACKUP_COMPATIBILITY {{\"passed\":true,\"reopens\":2,\"protectedTables\":5,\"taskCount\":24,\"privateRowsPrinted\":false,\"formalDatabaseOpenedForWrite\":false}}");
    }

    #[test]
    fn fresh_local_profile_has_no_saved_platform_connection() {
        let fresh = Storage::open(PathBuf::from(":memory:")).expect("fresh local profile");
        for key in ["feishu.app_id", "feishu.app_token", "feishu.table_id", "feishu.workspace_url",
            "wps.webhook_url", "workspace.provisioning.checkpoint.v1"] {
            assert!(fresh.get_setting(key).expect("fresh setting").is_none(),
                "fresh local profile must not inherit platform setting {key}");
        }
        let private_task_count: i64 = fresh.connection.query_row(
            "SELECT COUNT(*) FROM tasks WHERE source != 'local-demo'", [], |row| row.get(0))
            .expect("count user records");
        assert_eq!(private_task_count, 0, "fresh profile may seed local examples but not user cloud records");
    }

    fn storage() -> Storage {
        Storage::open(PathBuf::from(":memory:")).expect("create in-memory storage")
    }

    #[test]
    fn floating_hiding_is_scoped_survives_refresh_and_fills_remaining_slots() {
        let mut s=storage();
        let tasks=(0..7).map(|n|Task{id:format!("rec{n}"),title:format!("示例 {n}"),detail:String::new(),progress:0,quadrant:"iu".into(),status:"未开始".into(),today:false,planned_date:None,updated_at:n}).collect::<Vec<_>>();
        s.replace_remote_tasks(&tasks,"feishu").unwrap();
        let first=s.list_tasks_for_floating("target-a").unwrap();assert_eq!(first.len(),5);
        for t in &first[..2] {s.hide_floating_task("target-a",&t.id).unwrap();}
        assert_eq!(s.floating_hidden_count("target-a").unwrap(),2);
        let visible=s.list_tasks_for_floating("target-a").unwrap();assert_eq!(visible.len(),5);
        assert!(visible.iter().all(|t|first[..2].iter().all(|h|h.id!=t.id)));
        assert_eq!(s.floating_hidden_count("target-b").unwrap(),0);
        assert!(s.list_tasks_for_floating("target-b").unwrap().iter().any(|t|t.id==first[0].id));
        s.replace_remote_tasks(&tasks,"feishu").unwrap();
        assert_eq!(s.floating_hidden_count("target-a").unwrap(),2);
        s.show_hidden_floating_tasks("target-a").unwrap();
        assert_eq!(s.floating_hidden_count("target-a").unwrap(),0);
        assert!(s.list_tasks_for_floating("target-a").unwrap().iter().any(|t|t.id==first[0].id));
        assert_eq!(s.connection.query_row("SELECT count(*) FROM tasks",[],|r|r.get::<_,i64>(0)).unwrap(),7);
    }

    #[test]
    fn exact_alias_matches_without_model() {
        let storage = storage();
        let result = storage
            .analyze("完成了 C技术 方案初版", 55)
            .expect("analyze");
        match result {
            AnalysisResult::Match {
                task,
                next_progress,
                ..
            } => {
                assert_eq!(task.id, "task-c");
                assert_eq!(next_progress, 55);
            }
            _ => panic!("expected exact local match"),
        }
    }

    #[test]
    fn update_and_undo_are_transactional() {
        let mut storage = storage();
        let updated = storage
            .confirm(ConfirmInput {
                task_id: Some("task-b".to_string()),
                temporary_title: None,
                description: "完成B技术关键资料核验".to_string(),
                progress: 92,
                quadrant: None,
                today: false,
                expected_title: None,
                expected_progress: None,
                expected_status: None,
                change_planned_date: false,
                planned_date: None,
                expected_planned_date_known: false,
                expected_planned_date: None,
            })
            .expect("confirm update");
        assert_eq!(updated.progress, 92);

        let undo = storage.undo_latest().expect("undo");
        assert!(undo.undone);
        let restored: i64 = storage
            .connection
            .query_row(
                "SELECT progress FROM tasks WHERE id='task-b'",
                [],
                |row| row.get(0),
            )
            .expect("read restored task");
        assert_eq!(restored, 88);
    }

    #[test]
    fn temporary_task_requires_quadrant_and_can_be_removed() {
        let mut storage = storage();
        let created = storage
            .confirm(ConfirmInput {
                task_id: None,
                temporary_title: Some("准备项目汇报".to_string()),
                description: "这是临时任务：准备项目汇报".to_string(),
                progress: 20,
                quadrant: Some("in".to_string()),
                today: false,
                expected_title: None,
                expected_progress: None,
                expected_status: None,
                change_planned_date: false,
                planned_date: None,
                expected_planned_date_known: false,
                expected_planned_date: None,
            })
            .expect("create temporary task");
        assert_eq!(created.title, "准备项目汇报");
        assert!(storage.undo_latest().expect("undo create").undone);
        let count: i64 = storage
            .connection
            .query_row(
                "SELECT COUNT(*) FROM tasks WHERE id=?1",
                [&created.id],
                |row| row.get(0),
            )
            .expect("count tasks");
        assert_eq!(count, 0);
    }

    #[test]
    fn local_date_change_is_reverted_with_the_progress_update() {
        let mut storage = storage();
        let updated = storage
            .confirm(ConfirmInput {
                task_id: Some("task-e".to_string()),
                temporary_title: None,
                description: "E技术报告延期到2026-10-20".to_string(),
                progress: 60,
                quadrant: None,
                today: false,
                change_planned_date: true,
                planned_date: Some("2026-10-20".to_string()),
                expected_title: Some("E技术报告".to_string()),
                expected_progress: Some(40),
                expected_status: Some("进行中".to_string()),
                expected_planned_date_known: true,
                expected_planned_date: None,
            })
            .expect("confirm reschedule");
        assert_eq!(updated.planned_date.as_deref(), Some("2026-10-20"));

        assert!(storage.undo_latest().expect("undo reschedule").undone);
        let restored: Option<String> = storage
            .connection
            .query_row(
                "SELECT planned_date FROM tasks WHERE id='task-e'",
                [],
                |row| row.get(0),
            )
            .expect("read restored date");
        assert_eq!(restored, None);
    }

    #[test]
    fn follow_up_matches_a_task_whose_old_title_kept_the_create_command() {
        let storage = storage();
        storage
            .connection
            .execute(
                "INSERT INTO tasks
                 (id,title,detail,progress,quadrant,status,today,planned_date,updated_at,source)
                 VALUES ('rec-date-test','新建一个计划日期验收任务','计划在2026年10月15日完成',0,'in','未开始',0,'2026-10-15',1,'wps')",
                [],
            )
            .expect("insert reproduced WPS task");
        storage
            .connection
            .execute(
                "INSERT INTO task_search_v2(task_id,title,detail)
                 VALUES ('rec-date-test','新建一个计划日期验收任务','计划在2026年10月15日完成')",
                [],
            )
            .expect("index reproduced WPS task");

        let result = storage
            .analyze("计划日期验收任务已完成资料准备", 40)
            .expect("analyze follow-up");
        match result {
            AnalysisResult::Match { task, .. } => assert_eq!(task.id, "rec-date-test"),
            _ => panic!("expected existing task match instead of temporary task"),
        }
    }

    #[test]
    fn suggested_title_removes_create_commands_and_schedule_text() {
        assert_eq!(
            suggested_title("新建一个计划日期验收任务，计划在2026年10月15日完成"),
            "计划日期验收任务"
        );
        assert_eq!(
            suggested_detail("新建一个计划日期验收任务，计划在2026年10月15日完成"),
            "推进计划日期验收并记录结果"
        );
    }

    #[test]
    fn explicit_intent_never_crosses_between_update_and_create() {
        let storage = storage();
        let update = storage
            .analyze_with_intent("完全陌生的工作描述", 30, "update")
            .expect("analyze update intent");
        assert!(matches!(update, AnalysisResult::Ambiguous { .. }));

        let create = storage
            .analyze_with_intent("E技术报告完成资料准备", 30, "create")
            .expect("analyze create intent");
        assert!(matches!(create, AnalysisResult::Temporary { .. }));
    }

    #[test]
    fn explicit_update_can_reopen_a_named_completed_task() {
        let storage = storage();
        storage.connection.execute(
            "UPDATE tasks SET title='【验收】飞书写入测试',progress=100,status='已完成' WHERE id='task-e'", [],
        ).unwrap();
        let result = storage
            .analyze_with_intent("【验收】飞书写入测试延期到2026年10月20日", 71, "update")
            .unwrap();
        match result {
            AnalysisResult::Match {
                task,
                next_progress,
                ..
            } => {
                assert_eq!(task.id, "task-e");
                assert_eq!(task.status, "已完成"); // Keep the actual snapshot for conflict detection.
                assert_eq!(next_progress, 71);
            }
            _ => panic!("explicitly named completed task must remain selectable"),
        }
        assert!(!storage
            .list_task_candidates(50)
            .unwrap()
            .iter()
            .any(|task| task.id == "task-e"));
        let generic = storage
            .analyze_with_intent("其他工作延期", 71, "update")
            .unwrap();
        match generic {
            AnalysisResult::Ambiguous { candidates, .. } => {
                assert!(!candidates.iter().any(|task| task.id == "task-e"))
            }
            _ => panic!("unnamed completed tasks must not be suggested"),
        }
    }

    #[test]
    fn pending_writes_are_persistent_and_stateful() {
        let storage = storage();
        let input = ConfirmInput {
            task_id: Some("task-e".to_string()),
            temporary_title: None,
            description: "继续整理 E技术 报告".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        let id = storage
            .enqueue_pending_write(
                &input,
                Some(r#"{"phrase":"衍射报告","kind":"alias"}"#),
                "断网",
            )
            .expect("enqueue");
        let queued = storage.list_pending_writes().expect("pending writes");
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, id);
        assert_eq!(queued[0].input.expected_progress, Some(40));
        assert!(queued[0].habit_json.is_some());
        assert_eq!(storage.pending_write_summary().expect("summary").pending, 1);

        let prepared = storage.prepare_pending_write(id).expect("prepare");
        assert_eq!(prepared.description, input.description);
        assert_eq!(storage.pending_write_summary().expect("summary").editing, 1);

        storage
            .set_pending_write_state(id, "conflict", "远端已变化")
            .expect("mark conflict");
        let summary = storage.pending_write_summary().expect("summary");
        assert_eq!(summary.pending, 0);
        assert_eq!(summary.conflict, 1);

        storage.delete_pending_write(id).expect("delete");
        assert_eq!(
            storage.pending_write_summary().expect("summary").conflict,
            0
        );
    }

    #[test]
    fn synced_write_log_can_be_marked_reverted_after_cloud_verification() {
        let storage = storage();
        let previous = Task {
            id: "rec-1".to_string(),
            title: "E技术报告".to_string(),
            detail: String::new(),
            progress: 40,
            quadrant: "in".to_string(),
            status: "进行中".to_string(),
            today: false,
            planned_date: None,
            updated_at: now(),
        };
        let updated = Task {
            progress: 60,
            ..previous.clone()
        };
        let input = ConfirmInput {
            task_id: Some(previous.id.clone()),
            temporary_title: None,
            description: "更新测试".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        storage
            .record_synced_wps_write(&input, Some(&previous), &updated)
            .expect("record synced write");
        let log = storage
            .latest_synced_write()
            .expect("latest write")
            .expect("write log");
        assert_eq!(log.previous_progress, Some(40));
        assert_eq!(log.next_progress, 60);
        assert!(!log.created_task);
        storage
            .mark_synced_write_reverted(log.id)
            .expect("mark reverted");
        assert!(storage.latest_synced_write().expect("latest").is_none());
    }

    #[test]
    fn uncertain_write_preserves_observed_remote_state_for_safe_undo() {
        let storage = storage();
        let previous = Task {
            id: "rec-uncertain".to_string(),
            title: "D项目调研与规划".to_string(),
            detail: String::new(),
            progress: 40,
            quadrant: "in".to_string(),
            status: String::new(),
            today: false,
            planned_date: None,
            updated_at: now(),
        };
        let observed = Task {
            progress: 25,
            status: String::new(),
            ..previous.clone()
        };
        let input = ConfirmInput {
            task_id: Some(previous.id.clone()),
            temporary_title: None,
            description: "项目推进遇到困难，先做技术分析".to_string(),
            progress: 25,
            quadrant: None,
            today: false,
            expected_title: Some(previous.title.clone()),
            expected_progress: Some(40),
            expected_status: Some(String::new()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        storage
            .record_uncertain_wps_write(&input, &previous, Some(&observed))
            .expect("record uncertain write");
        let pending_id = storage
            .enqueue_uncertain_write(&input, None, "回读不一致")
            .expect("enqueue uncertain write");
        let duplicate_id = storage
            .enqueue_uncertain_write(&input, None, "仍待核对")
            .expect("deduplicate uncertain write");
        assert_eq!(pending_id, duplicate_id);
        let log = storage
            .latest_synced_write()
            .expect("latest uncertain write")
            .expect("uncertain write log");
        assert_eq!(log.previous_progress, Some(40));
        assert_eq!(log.observed_progress, Some(25));
        assert_eq!(log.observed_status.as_deref(), Some(""));
        assert_eq!(
            storage.pending_write_summary().expect("summary").uncertain,
            1
        );
        storage
            .resolve_uncertain_write(&log.task_id, log.next_progress)
            .expect("resolve uncertain write");
        assert_eq!(
            storage.pending_write_summary().expect("summary").uncertain,
            0
        );
    }

    #[test]
    fn pending_write_survives_database_reopen() {
        let path = std::env::temp_dir().join(format!(
            "project-track-queue-{}-{}.sqlite3",
            std::process::id(),
            now()
        ));
        let input = ConfirmInput {
            task_id: Some("task-e".to_string()),
            temporary_title: None,
            description: "离线继续整理 E技术 报告".to_string(),
            progress: 65,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        {
            let storage = Storage::open(path.clone()).expect("open database");
            storage
                .enqueue_pending_write(&input, None, "断网")
                .expect("enqueue");
        }
        {
            let reopened = Storage::open(path.clone()).expect("reopen database");
            let writes = reopened.list_pending_writes().expect("read after reopen");
            assert_eq!(writes.len(), 1);
            assert_eq!(writes[0].input.description, input.description);
        }
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{}", path.display(), suffix));
        }
    }

    #[test]
    fn saved_habit_matches_before_ai_fallback() {
        let storage = storage();
        let task = storage
            .list_task_candidates(50)
            .expect("tasks")
            .into_iter()
            .find(|task| task.id == "task-c")
            .expect("C技术 task");
        storage
            .save_habit("示例别名方案", &task, "alias")
            .expect("save habit");
        let result = storage
            .analyze("今天继续整理示例别名方案", 65)
            .expect("analyze");
        match result {
            AnalysisResult::Match {
                task,
                analysis_source,
                ..
            } => {
                assert_eq!(task.id, "task-c");
                assert_eq!(analysis_source, "habit");
            }
            _ => panic!("expected habit match"),
        }
        assert_eq!(storage.list_habits().expect("habits")[0].hit_count, 1);
    }

    #[test]
    fn specific_project_alias_wins_over_a_generic_technology_alias() {
        let storage = storage();
        let result = storage
            .analyze("D项目的E技术调研遇到困难，先要做技术分析", 25)
            .expect("analyze");
        match result {
            AnalysisResult::Match {
                task,
                analysis_source,
                ..
            } => {
                assert_eq!(task.id, "task-d");
                assert_eq!(analysis_source, "local");
            }
            _ => panic!("expected the specific project alias to win"),
        }
    }

    #[test]
    fn learned_correction_matches_a_close_rephrasing_before_local_aliases() {
        let storage = storage();
        let task = storage
            .list_task_candidates(50)
            .expect("tasks")
            .into_iter()
            .find(|task| task.id == "task-d")
            .expect("D项目 task");
        storage
            .save_habit(
                "D项目的E技术调研遇到困难，先要做技术分析",
                &task,
                "correction",
            )
            .expect("save correction");

        let result = storage
            .analyze_with_intent(
                "D项目的E技术调研遇到困难，先做技术分析并整理下一步方案",
                42,
                "update",
            )
            .expect("analyze rephrasing");

        match result {
            AnalysisResult::Match {
                task,
                analysis_source,
                ..
            } => {
                assert_eq!(task.id, "task-d");
                assert_eq!(analysis_source, "habit");
            }
            _ => panic!("expected learned correction to match the rephrasing"),
        }
        assert_eq!(storage.list_habits().expect("habits")[0].hit_count, 1);
    }

    #[test]
    fn correction_follows_the_unique_task_title_when_wps_record_id_changes() {
        let storage = storage();
        let original = storage
            .list_task_candidates(50)
            .expect("tasks")
            .into_iter()
            .find(|task| task.id == "task-c")
            .expect("C技术 task");
        storage
            .save_habit("示例别名返工", &original, "correction")
            .expect("save correction");
        storage
            .connection
            .execute("UPDATE tasks SET id='wps-new-id' WHERE id='task-c'", [])
            .expect("change simulated WPS record id");
        let result = storage
            .analyze("示例别名返工，需要把进度回退", 39)
            .expect("analyze");
        match result {
            AnalysisResult::Match {
                task,
                analysis_source,
                ..
            } => {
                assert_eq!(task.id, "wps-new-id");
                assert_eq!(analysis_source, "habit");
            }
            _ => panic!("expected habit match after record id change"),
        }
    }

    #[test]
    fn learned_task_type_creates_a_new_named_task_instead_of_matching_old_records() {
        let storage = storage();
        storage
            .save_task_type_habit("资料核对", "资料核对", "in")
            .expect("save task type");
        let result = storage
            .analyze("完成了2026-ABC1234资料核对，并提交给项目组", 100)
            .expect("analyze");
        match result {
            AnalysisResult::Temporary {
                suggested_title,
                suggested_quadrant,
                suggested_phrase,
                analysis_source,
                ..
            } => {
                assert_eq!(suggested_title, "资料核对");
                assert_eq!(suggested_quadrant.as_deref(), Some("in"));
                assert_eq!(suggested_phrase.as_deref(), Some("资料核对"));
                assert_eq!(analysis_source, "habit");
            }
            _ => panic!("expected learned task type"),
        }
        let habits = storage.list_habits().expect("habits");
        let learned = habits
            .iter()
            .find(|habit| habit.kind == "task_type")
            .expect("task type habit");
        assert!(learned.task_id.is_empty());
        assert_eq!(learned.hit_count, 1);
    }

    #[test]
    fn correction_deactivates_conflicting_habit() {
        let storage = storage();
        let tasks = storage.list_task_candidates(50).expect("tasks");
        let example_e = tasks
            .iter()
            .find(|task| task.id == "task-e")
            .expect("E技术 task");
        let example_c = tasks
            .iter()
            .find(|task| task.id == "task-c")
            .expect("C技术 task");
        storage
            .save_habit("示例别名", example_e, "alias")
            .expect("save old habit");
        storage
            .save_habit("示例别名", example_c, "correction")
            .expect("save correction");
        let active = storage.list_habits().expect("habits");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].task_id, "task-c");
        assert_eq!(active[0].kind, "correction");
    }

    #[test]
    fn retrieval_limits_context_and_keeps_relevant_task_and_habit() {
        let storage = storage();
        for index in 0..20 {
            let id = format!("extra-{index}");
            let title = format!("无关任务{index}");
            storage
                .connection
                .execute(
                    "INSERT INTO tasks
                     (id,title,detail,progress,quadrant,status,today,updated_at,source)
                     VALUES (?1,?2,'常规说明',0,'nn','进行中',0,?3,'local')",
                    params![id, title, index],
                )
                .expect("insert extra task");
            storage
                .connection
                .execute(
                    "INSERT INTO task_search_v2(task_id,title,detail) VALUES (?1,?2,'常规说明')",
                    params![id, title],
                )
                .expect("index extra task");
        }
        let target = storage
            .list_task_candidates(50)
            .expect("tasks")
            .into_iter()
            .find(|task| task.id == "task-e")
            .expect("E技术 task");
        storage
            .save_habit("衍射报告", &target, "alias")
            .expect("save habit");

        let context = storage
            .retrieve_ai_context("继续整理衍射报告数据")
            .expect("retrieve context");
        assert!(context.tasks.len() <= 10);
        assert!(context.habits.len() <= 5);
        assert!(context.tasks.iter().any(|task| task.id == "task-e"));
        assert!(context
            .habits
            .iter()
            .any(|habit| habit.task_id == "task-e"));
        assert_eq!(context.stats.total_tasks, 25);
        assert!(
            context.stats.estimated_context_tokens < context.stats.estimated_full_context_tokens
        );
    }
}
