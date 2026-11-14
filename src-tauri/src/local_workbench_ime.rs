//! Local, explicitly enabled input-method ingestion. This module never opens cloud connections.
use super::*;
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{Emitter, Manager};

pub(super) fn initialize(c: &Connection) -> Result<(), String> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS local_ime_operations (
        idem_key TEXT PRIMARY KEY, id TEXT NOT NULL UNIQUE, text TEXT NOT NULL,
        mode TEXT NOT NULL, status TEXT NOT NULL, reason TEXT NOT NULL,
        task_id TEXT, activity_description TEXT NOT NULL, progress_after INTEGER,
        result TEXT NOT NULL, created_at INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS local_task_activity (
        id TEXT PRIMARY KEY, task_id TEXT NOT NULL, operation_id TEXT NOT NULL UNIQUE,
        description TEXT NOT NULL, original_text TEXT NOT NULL,
        created_at INTEGER NOT NULL, history_id INTEGER, undone INTEGER NOT NULL DEFAULT 0
    );
    CREATE INDEX IF NOT EXISTS local_ime_review_status ON local_ime_operations(status,created_at);",
    )
    .map_err(error)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Utterance {
    pub text: String,
    #[serde(default = "default_mode")]
    pub mode: String,
}
fn default_mode() -> String {
    "applyIfExact".into()
}

fn random_id(c: &Connection, prefix: &str) -> Result<String, String> {
    let id: String = c
        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
        .map_err(error)?;
    Ok(format!("{prefix}{id}"))
}

fn chinese_digit(c: char) -> Option<u16> {
    match c {
        '零' | '〇' => Some(0),
        '一' => Some(1),
        '二' | '两' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    }
}
fn chinese_number(s: &str) -> Option<u8> {
    if s == "百" || s == "一百" {
        return Some(100);
    }
    if let Ok(n) = s.parse::<u8>() {
        return (n <= 100).then_some(n);
    }
    if s.chars().count() == 1 {
        return chinese_digit(s.chars().next()?).map(|n| n as u8);
    }
    if let Some((head, tail)) = s.split_once('十') {
        let tens = if head.is_empty() {
            1
        } else {
            chinese_digit(head.chars().next()?)?
        };
        if head.chars().count() > 1 || tail.chars().count() > 1 {
            return None;
        }
        let ones = if tail.is_empty() {
            0
        } else {
            chinese_digit(tail.chars().next()?)?
        };
        let n = tens * 10 + ones;
        return (n <= 99).then_some(n as u8);
    }
    None
}
fn parse_progress(text: &str) -> (Option<u8>, Option<usize>, bool) {
    let mut matches = Vec::new();
    let mut invalid = false;
    for (idx, _) in text.match_indices(['%', '％']) {
        let head = &text[..idx];
        let digits: String = head
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        if digits.is_empty() {
            invalid = true;
            continue;
        }
        match digits.parse::<u16>() {
            Ok(v) if v <= 100 => matches.push((idx - digits.len(), v as u8)),
            _ => invalid = true,
        }
    }
    for (idx, _) in text.match_indices("百分之") {
        let tail = &text[idx + "百分之".len()..];
        let number: String = tail
            .chars()
            .take_while(|c| c.is_ascii_digit() || "零〇一二两三四五六七八九十百".contains(*c))
            .collect();
        match chinese_number(&number) {
            Some(v) => matches.push((idx, v)),
            None => invalid = true,
        }
    }
    if matches.is_empty() {
        return (None, None, invalid);
    }
    let first = matches[0].1;
    if invalid || matches.iter().any(|(_, v)| *v != first) {
        return (None, None, true);
    }
    (Some(first), matches.iter().map(|(i, _)| *i).min(), false)
}
fn activity_description(text: &str, progress_start: Option<usize>) -> String {
    let prefix = progress_start.map_or(text, |i| &text[..i]);
    prefix
        .trim_end_matches(|c: char| c.is_whitespace() || "，,；;：:".contains(c))
        .trim_end_matches("达到")
        .trim_end_matches("进度")
        .trim_end_matches("到")
        .trim_end_matches(|c: char| c.is_whitespace() || "，,；;：:".contains(c))
        .trim()
        .to_string()
}

fn all_tasks(c: &Connection) -> Result<Vec<LocalTask>, String> {
    let mut statement = c
        .prepare("SELECT data FROM local_tasks ORDER BY id")
        .map_err(error)?;
    let tasks = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(error)?
        .map(|row| serde_json::from_str(&row.map_err(error)?).map_err(error))
        .collect();
    tasks
}
fn save_operation(
    c: &Connection,
    key: &str,
    id: &str,
    text: &str,
    mode: &str,
    status: &str,
    reason: &str,
    task_id: Option<&str>,
    description: &str,
    progress: Option<u8>,
    result: &Value,
) -> Result<(), String> {
    c.execute("INSERT INTO local_ime_operations(idem_key,id,text,mode,status,reason,task_id,activity_description,progress_after,result,created_at)
        VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
        params![key,id,text,mode,status,reason,task_id,description,progress.map(i64::from),result.to_string(),content_now()?]).map_err(error)?;
    Ok(())
}
fn apply(
    c: &Connection,
    before: LocalTask,
    progress: u8,
    description: &str,
    original: &str,
    op_id: &str,
) -> Result<Value, String> {
    let recorded = before.progress == progress;
    let mut after = before.clone();
    let history_id;
    if recorded {
        next_revision(c)?; // An activity still invalidates an earlier whole-workspace restore preview.
        log(c, &Some(before.clone()), &after, "记录任务完成情况")?;
        history_id = c.last_insert_rowid();
    } else {
        after.progress = progress;
        after.revision = next_revision(c)?;
        after.content_modified_at = Some(content_now()?);
        put(c, &after)?;
        log(c, &Some(before.clone()), &after, "输入法更新任务进度")?;
        history_id = c.last_insert_rowid();
    }
    let activity_id = random_id(c, "act_")?;
    c.execute("INSERT INTO local_task_activity(id,task_id,operation_id,description,original_text,created_at,history_id)
        VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![activity_id,after.id,op_id,description,original,content_now()?,history_id]).map_err(error)?;
    Ok(
        json!({"status":if recorded {"recorded"} else {"applied"},"operationId":op_id,
        "task":{"id":after.id,"title":after.title,"progressBefore":before.progress,
            "progressAfter":after.progress,"statusAfter":if after.progress==100 {"已完成"} else if after.progress==0 {"未开始"} else {"进行中"},"revision":after.revision},
        "recognized":{"activityDescription":description,"progressAfter":progress},
        "activityId":activity_id,"cloudSync":"notScheduled"}),
    )
}

impl LocalWorkspace {
    pub(super) fn ingest_utterance(
        &mut self,
        key: &str,
        utterance: Utterance,
    ) -> Result<Value, String> {
        if key.len() < 16
            || key.len() > 128
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("Idempotency-Key 无效".into());
        }
        let text = utterance.text.trim();
        if text.is_empty()
            || text.chars().count() > 500
            || !["applyIfExact", "reviewOnly"].contains(&utterance.mode.as_str())
        {
            return Err("输入内容或模式无效".into());
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT text,mode,result FROM local_ime_operations WHERE idem_key=?1",
                [key],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(error)?;
        if let Some((old_text, old_mode, result)) = existing {
            if old_text != text || old_mode != utterance.mode {
                return Err("idempotency_conflict".into());
            }
            return serde_json::from_str(&result).map_err(error);
        }
        let (progress, progress_start, invalid) = parse_progress(text);
        let description = activity_description(text, progress_start);
        let candidates: Vec<_> = all_tasks(&tx)?
            .into_iter()
            .filter(|t| !t.trashed && text.contains(t.title.trim()))
            .collect();
        let reason = if utterance.mode == "reviewOnly" {
            "reviewOnly"
        } else if invalid {
            "invalidProgress"
        } else if progress.is_none() {
            "missingProgress"
        } else if description.is_empty() {
            "missingDescription"
        } else if candidates.is_empty() {
            "taskNotFound"
        } else if candidates.len() != 1 {
            "ambiguousTask"
        } else {
            ""
        };
        let op_id = random_id(&tx, "op_")?;
        let result = if reason.is_empty() {
            apply(
                &tx,
                candidates.into_iter().next().unwrap(),
                progress.unwrap(),
                &description,
                text,
                &op_id,
            )?
        } else {
            next_revision(&tx)?;
            json!({"status":"needsReview","operationId":op_id,"reason":reason,
                "message":"已送到任务工作台待确认，尚未修改任务","cloudSync":"notScheduled"})
        };
        let task_id = result.pointer("/task/id").and_then(Value::as_str);
        save_operation(
            &tx,
            key,
            &op_id,
            text,
            &utterance.mode,
            result["status"].as_str().unwrap_or("needsReview"),
            reason,
            task_id,
            &description,
            progress,
            &result,
        )?;
        tx.commit().map_err(error)?;
        Ok(result)
    }
    pub(super) fn ime_operation(&self, id: &str) -> Result<Option<Value>, String> {
        let value: Option<String> = self
            .connection
            .query_row(
                "SELECT result FROM local_ime_operations WHERE id=?1",
                [id],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        value
            .map(|v| serde_json::from_str(&v).map_err(error))
            .transpose()
    }
    pub(super) fn ime_reviews(&self) -> Result<Vec<Value>, String> {
        let cutoff = content_now()? - 7 * 24 * 60 * 60 * 1000;
        let expired=self.connection.execute("UPDATE local_ime_operations SET status='cancelled',reason='expired',result=json_set(result,'$.status','cancelled','$.reason','expired') WHERE status='needsReview' AND created_at<?1",[cutoff]).map_err(error)?;
        if expired > 0 {
            next_revision(&self.connection)?;
        }
        let mut statement=self.connection.prepare("SELECT id,text,reason,progress_after,created_at FROM local_ime_operations WHERE status='needsReview' ORDER BY created_at DESC LIMIT 100").map_err(error)?;
        let reviews=statement.query_map([],|r|Ok(json!({"operationId":r.get::<_,String>(0)?,"text":r.get::<_,String>(1)?,"reason":r.get::<_,String>(2)?,"progressAfter":r.get::<_,Option<i64>>(3)?,"createdAt":r.get::<_,i64>(4)?}))).map_err(error)?
            .map(|r|r.map_err(error)).collect();
        reviews
    }
    pub(super) fn ime_activities(&self, task_id: &str) -> Result<Vec<Value>, String> {
        let mut statement=self.connection.prepare("SELECT description,original_text,created_at,undone FROM local_task_activity WHERE task_id=?1 ORDER BY created_at DESC LIMIT 100").map_err(error)?;
        let activities=statement.query_map([task_id],|r|Ok(json!({"description":r.get::<_,String>(0)?,"originalText":r.get::<_,String>(1)?,"createdAt":r.get::<_,i64>(2)?,"undone":r.get::<_,bool>(3)?}))).map_err(error)?
            .map(|r|r.map_err(error)).collect();
        activities
    }
    pub(super) fn confirm_ime_review(
        &mut self,
        op_id: &str,
        task_id: &str,
        progress: u8,
    ) -> Result<Value, String> {
        if progress > 100 {
            return Err("进度须为 0–100 的整数".into());
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let row:Option<(String,String,String,i64)>=tx.query_row("SELECT text,status,activity_description,created_at FROM local_ime_operations WHERE id=?1",[op_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(error)?;
        let (text, status, description, created_at) = row.ok_or("待确认输入不存在")?;
        if status != "needsReview" {
            return Err("该输入已处理，请刷新".into());
        }
        if created_at < content_now()? - 7 * 24 * 60 * 60 * 1000 {
            return Err("待确认输入已过期，请重新提交".into());
        }
        let task = get_task(&tx, task_id)?
            .filter(|t| !t.trashed)
            .ok_or("任务不存在或已移入回收站")?;
        let description = if description.is_empty() {
            text.as_str()
        } else {
            description.as_str()
        };
        let result = apply(&tx, task, progress, description, &text, op_id)?;
        tx.execute("UPDATE local_ime_operations SET status=?2,reason='',task_id=?3,progress_after=?4,activity_description=?5,result=?6 WHERE id=?1",
            params![op_id,result["status"].as_str(),task_id,progress,description,result.to_string()]).map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(result)
    }
    pub(super) fn cancel_ime_review(&mut self, op_id: &str) -> Result<(), String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let changed=tx.execute("UPDATE local_ime_operations SET status='cancelled',result=json_set(result,'$.status','cancelled') WHERE id=?1 AND status='needsReview'",[op_id]).map_err(error)?;
        if changed != 1 {
            return Err("该输入已处理或不存在，请刷新".into());
        }
        next_revision(&tx)?;
        tx.commit().map_err(error)
    }
}

pub(super) fn undo_activity(c: &Connection, history_id: i64) -> Result<(), String> {
    c.execute(
        "UPDATE local_task_activity SET undone=1 WHERE history_id=?1",
        [history_id],
    )
    .map_err(error)?;
    c.execute("UPDATE local_ime_operations SET status='cancelled',result=json_set(result,'$.status','cancelled')
        WHERE id IN (SELECT operation_id FROM local_task_activity WHERE history_id=?1)",[history_id]).map_err(error)?;
    Ok(())
}

#[tauri::command]
pub fn local_ime_reviews(state: State<'_, LocalWorkspaceState>) -> Result<Vec<Value>, String> {
    state.with(|db| db.ime_reviews())
}
#[tauri::command]
pub fn local_ime_activities(
    state: State<'_, LocalWorkspaceState>,
    task_id: String,
) -> Result<Vec<Value>, String> {
    state.with(|db| db.ime_activities(&task_id))
}
#[tauri::command]
pub fn local_ime_confirm_review(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    operation_id: String,
    task_id: String,
    progress: u8,
) -> Result<Value, String> {
    if window.label() != "local-workbench" {
        return Err("请从本地任务工作台确认输入".into());
    }
    state.with(|db| db.confirm_ime_review(&operation_id, &task_id, progress))
}
#[tauri::command]
pub fn local_ime_cancel_review(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    operation_id: String,
) -> Result<(), String> {
    if window.label() != "local-workbench" {
        return Err("请从本地任务工作台取消输入".into());
    }
    state.with(|db| db.cancel_ime_review(&operation_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(title: &str, progress: i64) -> SaveTask {
        SaveTask {
            id: None,
            expected_revision: None,
            title: title.into(),
            detail: String::new(),
            progress,
            quadrant: "in".into(),
            today: false,
            start_date: None,
            planned_date: None,
        }
    }
    #[test]
    fn chinese_and_ascii_percentages() {
        for text in [
            "今日完成资料核对任务，达到100%",
            "今日完成资料核对任务，达到百分之百",
            "今日完成资料核对任务，达到百分之一百",
        ] {
            assert_eq!(parse_progress(text).0, Some(100));
            assert_eq!(
                activity_description(text, parse_progress(text).1),
                "今日完成资料核对任务"
            );
        }
        assert_eq!(parse_progress("达到百分之六十").0, Some(60));
        assert!(parse_progress("达到40%和百分之百").2);
    }
    #[test]
    fn unique_task_updates_once_and_undoes_activity() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        db.save(task("资料核对任务", 60)).unwrap();
        let input = || Utterance {
            text: "今日完成资料核对任务，达到百分之一百".into(),
            mode: default_mode(),
        };
        let result = db
            .ingest_utterance("test-unique-key-0001", input())
            .unwrap();
        assert_eq!(result["status"], "applied");
        assert_eq!(
            result["recognized"]["activityDescription"],
            "今日完成资料核对任务"
        );
        assert_eq!(db.snapshot().unwrap().tasks[0].progress, 100);
        assert_eq!(
            db.ingest_utterance("test-unique-key-0001", input())
                .unwrap(),
            result
        );
        assert_eq!(
            db.ime_activities(&db.snapshot().unwrap().tasks[0].id)
                .unwrap()
                .len(),
            1
        );
        db.undo().unwrap();
        let current = db.snapshot().unwrap().tasks.remove(0);
        assert_eq!(current.progress, 60);
        assert_eq!(db.ime_activities(&current.id).unwrap()[0]["undone"], true);
        assert_eq!(
            db.ime_operation(result["operationId"].as_str().unwrap())
                .unwrap()
                .unwrap()["status"],
            "cancelled"
        );
    }
    #[test]
    fn ambiguous_and_missing_progress_never_write() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        db.save(task("资料核对任务", 40)).unwrap();
        db.save(task("资料核对任务", 80)).unwrap();
        let result = db
            .ingest_utterance(
                "test-ambiguous-key-01",
                Utterance {
                    text: "今日完成资料核对任务，达到100%".into(),
                    mode: default_mode(),
                },
            )
            .unwrap();
        assert_eq!(result["status"], "needsReview");
        assert_eq!(result["reason"], "ambiguousTask");
        let mut progress = db
            .snapshot()
            .unwrap()
            .tasks
            .iter()
            .map(|t| t.progress)
            .collect::<Vec<_>>();
        progress.sort();
        assert_eq!(progress, vec![40, 80]);
        let result = db
            .ingest_utterance(
                "test-missing-key-0001",
                Utterance {
                    text: "今日完成资料核对任务".into(),
                    mode: default_mode(),
                },
            )
            .unwrap();
        assert_eq!(result["reason"], "missingProgress");
        assert!(db
            .ingest_utterance(
                "test-ambiguous-key-01",
                Utterance {
                    text: "另一个句子".into(),
                    mode: default_mode()
                }
            )
            .is_err());
    }
    #[test]
    fn activity_only_does_not_change_task_revision_or_time() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        db.save(task("资料核对任务", 100)).unwrap();
        let before = db.snapshot().unwrap().tasks.remove(0);
        let result = db
            .ingest_utterance(
                "test-recorded-key-01",
                Utterance {
                    text: "今日完成资料核对任务，达到100%".into(),
                    mode: default_mode(),
                },
            )
            .unwrap();
        assert_eq!(result["status"], "recorded");
        assert_eq!(db.snapshot().unwrap().tasks[0], before);
        db.undo().unwrap();
        assert_eq!(db.snapshot().unwrap().tasks[0], before);
        assert_eq!(db.ime_activities(&before.id).unwrap()[0]["undone"], true);
    }
    #[test]
    fn http_request_is_bounded_and_preserves_utf8_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let writer = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            let body = r#"{"text":"今日完成资料核对任务，达到百分之百"}"#;
            let request=format!("POST /v1/utterances HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len());
            stream.write_all(request.as_bytes()).unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();
        let (method, path, _, body) = read_request(&mut stream).unwrap();
        writer.join().unwrap();
        assert_eq!(method, "POST");
        assert_eq!(path, "/v1/utterances");
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["text"], "今日完成资料核对任务，达到百分之百");
        assert!(equal_secret("abc", "abc"));
        assert!(!equal_secret("abc", "abd"));
    }
}

const CREDENTIAL_SERVICE: &str = "app.snaptask.desktop.ime-api";
const CREDENTIAL_USER_PREFIX: &str = "local-input-method:";

#[cfg(windows)]
fn load_token(identifier: &str) -> Result<String, String> {
    keyring::Entry::new(
        CREDENTIAL_SERVICE,
        &format!("{CREDENTIAL_USER_PREFIX}{identifier}"),
    )
    .and_then(|e| e.get_password())
    .map_err(|_| "输入法接口令牌未配置".into())
}
#[cfg(windows)]
fn save_token(identifier: &str, token: &str) -> Result<(), String> {
    keyring::Entry::new(
        CREDENTIAL_SERVICE,
        &format!("{CREDENTIAL_USER_PREFIX}{identifier}"),
    )
    .and_then(|e| e.set_password(token))
    .map_err(|e| format!("无法保存输入法接口令牌：{e}"))
}
#[cfg(not(windows))]
fn load_token(_identifier: &str) -> Result<String, String> {
    Err("输入法接口目前仅支持 Windows".into())
}
#[cfg(not(windows))]
fn save_token(_identifier: &str, _token: &str) -> Result<(), String> {
    Err("输入法接口目前仅支持 Windows".into())
}

#[derive(Default)]
struct ApiRuntime {
    port: Option<u16>,
    stop: Option<Arc<AtomicBool>>,
    token: Option<Arc<Mutex<String>>>,
    error: Option<String>,
}
pub struct ImeApiState {
    folder: PathBuf,
    identifier: String,
    runtime: Mutex<ApiRuntime>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiStatus {
    enabled: bool,
    port: Option<u16>,
    address: Option<String>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}
impl ImeApiState {
    pub fn new(folder: PathBuf, identifier: String) -> Self {
        let _ = std::fs::remove_file(folder.join("ime-api.json"));
        Self {
            folder,
            identifier,
            runtime: Mutex::new(ApiRuntime::default()),
        }
    }
    fn enabled_path(&self) -> PathBuf {
        self.folder.join("ime-api-enabled.json")
    }
    fn discovery_path(&self) -> PathBuf {
        self.folder.join("ime-api.json")
    }
    fn status(&self, token: Option<String>) -> Result<ApiStatus, String> {
        let runtime = self.runtime.lock().map_err(|_| "输入法接口状态繁忙")?;
        Ok(ApiStatus {
            enabled: runtime.port.is_some(),
            port: runtime.port,
            address: runtime.port.map(|p| format!("http://127.0.0.1:{p}")),
            error: runtime.error.clone(),
            token,
        })
    }
    fn start(&self, app: tauri::AppHandle, token: String) -> Result<ApiStatus, String> {
        let mut runtime = self.runtime.lock().map_err(|_| "输入法接口状态繁忙")?;
        if runtime.port.is_some() {
            return drop_and_status(self, runtime, None);
        }
        let listener = TcpListener::bind("127.0.0.1:0").map_err(error)?;
        listener.set_nonblocking(true).map_err(error)?;
        let port = listener.local_addr().map_err(error)?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let shared_token = Arc::new(Mutex::new(token));
        let thread_stop = stop.clone();
        let thread_token = shared_token.clone();
        let payload = json!({"apiVersion":1,"port":port,"pid":std::process::id()}).to_string();
        let discovery = self.discovery_path();
        let temporary = self.folder.join("ime-api.json.tmp");
        std::fs::write(&temporary, payload).map_err(error)?;
        let _ = std::fs::remove_file(&discovery);
        std::fs::rename(&temporary, &discovery).map_err(error)?;
        std::thread::spawn(move || serve(listener, app, thread_stop, thread_token));
        runtime.port = Some(port);
        runtime.stop = Some(stop);
        runtime.token = Some(shared_token);
        runtime.error = None;
        drop_and_status(self, runtime, None)
    }
    fn stop(&self) -> Result<ApiStatus, String> {
        let mut runtime = self.runtime.lock().map_err(|_| "输入法接口状态繁忙")?;
        if let Some(flag) = runtime.stop.take() {
            flag.store(true, Ordering::SeqCst);
        }
        runtime.port = None;
        runtime.token = None;
        runtime.error = None;
        let _ = std::fs::remove_file(self.discovery_path());
        drop_and_status(self, runtime, None)
    }
    pub fn startup(&self, app: tauri::AppHandle) {
        if std::fs::read_to_string(self.enabled_path()).ok().as_deref() != Some("true") {
            return;
        }
        match load_token(&self.identifier).and_then(|token| self.start(app, token).map(|_| ())) {
            Ok(()) => {}
            Err(e) => {
                if let Ok(mut runtime) = self.runtime.lock() {
                    runtime.error = Some(e);
                }
            }
        }
    }
}
fn drop_and_status(
    state: &ImeApiState,
    guard: std::sync::MutexGuard<'_, ApiRuntime>,
    token: Option<String>,
) -> Result<ApiStatus, String> {
    drop(guard);
    state.status(token)
}
#[tauri::command]
pub fn local_ime_status(state: State<'_, ImeApiState>) -> Result<ApiStatus, String> {
    state.status(None)
}
#[tauri::command]
pub fn local_ime_set_enabled(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, ImeApiState>,
    workspace: State<'_, LocalWorkspaceState>,
    enabled: bool,
) -> Result<ApiStatus, String> {
    if window.label() != "local-workbench" {
        return Err("请从本地任务工作台设置输入法接口".into());
    }
    if !enabled {
        std::fs::write(state.enabled_path(), "false").map_err(error)?;
        return state.stop();
    }
    let mut new_token = None;
    let token = match load_token(&state.identifier) {
        Ok(t) => t,
        Err(_) => {
            let t = workspace.with(|db| {
                db.connection
                    .query_row("SELECT lower(hex(randomblob(32)))", [], |r| {
                        r.get::<_, String>(0)
                    })
                    .map_err(error)
            })?;
            save_token(&state.identifier, &t)?;
            new_token = Some(t.clone());
            t
        }
    };
    let mut status = state.start(app, token)?;
    if let Err(e) = std::fs::write(state.enabled_path(), "true") {
        let _ = state.stop();
        return Err(error(e));
    }
    status.token = new_token;
    Ok(status)
}
impl Drop for ImeApiState {
    fn drop(&mut self) {
        if let Ok(mut runtime) = self.runtime.lock() {
            if let Some(stop) = runtime.stop.take() {
                stop.store(true, Ordering::SeqCst);
            }
        }
        let _ = std::fs::remove_file(self.discovery_path());
    }
}
#[tauri::command]
pub fn local_ime_rotate_token(
    window: tauri::WebviewWindow,
    state: State<'_, ImeApiState>,
    workspace: State<'_, LocalWorkspaceState>,
) -> Result<ApiStatus, String> {
    if window.label() != "local-workbench" {
        return Err("请从本地任务工作台设置输入法接口".into());
    }
    let token: String = workspace.with(|db| {
        db.connection
            .query_row("SELECT lower(hex(randomblob(32)))", [], |r| r.get(0))
            .map_err(error)
    })?;
    save_token(&state.identifier, &token)?;
    if let Ok(runtime) = state.runtime.lock() {
        if let Some(shared) = &runtime.token {
            *shared.lock().map_err(|_| "输入法接口状态繁忙")? = token.clone();
        }
    }
    state.status(Some(token))
}
fn equal_secret(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes().zip(b.bytes()).fold(0u8, |v, (x, y)| v | (x ^ y)) == 0
}
fn send(stream: &mut TcpStream, code: u16, value: Value) {
    let body = value.to_string();
    let label = match code {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        409 => "Conflict",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        _ => "Service Unavailable",
    };
    let header=format!("HTTP/1.1 {code} {label}\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",body.len());
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}
fn read_request(
    stream: &mut TcpStream,
) -> Result<(String, String, Vec<(String, String)>, Vec<u8>), u16> {
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|_| 400u16)?;
    let mut bytes = Vec::new();
    let mut buffer = [0u8; 1024];
    let end;
    loop {
        if bytes.len() > 8192 {
            return Err(413);
        }
        let n = stream.read(&mut buffer).map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        bytes.extend_from_slice(&buffer[..n]);
        if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            end = i + 4;
            break;
        }
    }
    let head = std::str::from_utf8(&bytes[..end]).map_err(|_| 400u16)?;
    let mut lines = head.split("\r\n");
    let request = lines.next().ok_or(400u16)?;
    let mut parts = request.split_whitespace();
    let method = parts.next().ok_or(400u16)?.to_string();
    let path = parts.next().ok_or(400u16)?.to_string();
    if parts.next() != Some("HTTP/1.1") || parts.next().is_some() {
        return Err(400);
    }
    let mut headers = Vec::new();
    for line in lines.filter(|s| !s.is_empty()) {
        let (k, v) = line.split_once(':').ok_or(400u16)?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let get = |name: &str| {
        headers
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
    };
    if !get("transfer-encoding").is_empty() {
        return Err(400);
    }
    let lengths = get("content-length");
    if lengths.len() > 1 {
        return Err(400);
    }
    let length = if lengths.is_empty() {
        0
    } else {
        lengths[0].parse::<usize>().map_err(|_| 400u16)?
    };
    if length > 4096 {
        return Err(413);
    }
    let mut body = bytes[end..].to_vec();
    if body.len() > length {
        return Err(400);
    }
    while body.len() < length {
        let n = stream.read(&mut buffer).map_err(|_| 400u16)?;
        if n == 0 {
            return Err(400);
        }
        body.extend_from_slice(&buffer[..n]);
        if body.len() > length {
            return Err(400);
        }
    }
    Ok((method, path, headers, body))
}
fn handle<R: tauri::Runtime>(
    mut stream: TcpStream,
    app: &tauri::AppHandle<R>,
    token: &Arc<Mutex<String>>,
    port: u16,
    rate: &mut (Instant, u32),
) {
    let (method, path, headers, body) = match read_request(&mut stream) {
        Ok(v) => v,
        Err(code) => {
            send(&mut stream, code, json!({"code":if code==413 {"payload_too_large"} else {"invalid_request"}}));
            return;
        }
    };
    let get = |name: &str| {
        headers
            .iter()
            .filter(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
    };
    if get("host") != [format!("127.0.0.1:{port}").as_str()] || !get("origin").is_empty() {
        send(&mut stream, 400, json!({"code":"invalid_origin"}));
        return;
    }
    if method == "GET" && path == "/v1/health" {
        send(&mut stream, 200, json!({"apiVersion":1,"status":"ready"}));
        return;
    }
    let auth = get("authorization");
    let secret = token.lock().map(|v| v.clone()).unwrap_or_default();
    if auth.len() != 1
        || !auth[0]
            .strip_prefix("Bearer ")
            .is_some_and(|v| equal_secret(v, &secret))
    {
        send(&mut stream, 401, json!({"code":"unauthorized"}));
        return;
    }
    if method == "GET" && path.starts_with("/v1/operations/") {
        let id = &path["/v1/operations/".len()..];
        let result = app
            .state::<LocalWorkspaceState>()
            .with(|db| db.ime_operation(id));
        match result {
            Ok(Some(v)) => send(&mut stream, 200, v),
            Ok(None) => send(&mut stream, 404, json!({"code":"not_found"})),
            Err(_) => send(&mut stream, 503, json!({"code":"workspace_unavailable"})),
        }
        return;
    }
    if method != "POST" || path != "/v1/utterances" {
        send(&mut stream, 404, json!({"code":"not_found"}));
        return;
    }
    if rate.0.elapsed() > Duration::from_secs(60) {
        *rate = (Instant::now(), 0);
    }
    rate.1 += 1;
    if rate.1 > 60 {
        send(&mut stream, 429, json!({"code":"rate_limited"}));
        return;
    }
    if get("content-type").len() != 1 || !get("content-type")[0].starts_with("application/json") {
        send(&mut stream, 400, json!({"code":"invalid_request"}));
        return;
    }
    let keys = get("idempotency-key");
    if keys.len() != 1 {
        send(&mut stream, 400, json!({"code":"invalid_request"}));
        return;
    }
    let request: Utterance = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => {
            send(&mut stream, 400, json!({"code":"invalid_request"}));
            return;
        }
    };
    let result = app
        .state::<LocalWorkspaceState>()
        .with(|db| db.ingest_utterance(keys[0], request));
    match result {
        Ok(value) => {
            let code = if value["status"] == "needsReview" {
                202
            } else {
                200
            };
            let _ = app.emit_to("local-workbench", "local-ime-changed", ());
            send(&mut stream, code, value);
        }
        Err(e) if e == "idempotency_conflict" => {
            send(&mut stream, 409, json!({"code":"idempotency_conflict"}))
        }
        Err(e) if e.contains("无效") => send(
            &mut stream,
            400,
            json!({"code":"invalid_request","message":e}),
        ),
        Err(_) => send(&mut stream, 503, json!({"code":"workspace_unavailable"})),
    }
}
fn serve<R: tauri::Runtime>(
    listener: TcpListener,
    app: tauri::AppHandle<R>,
    stop: Arc<AtomicBool>,
    token: Arc<Mutex<String>>,
) {
    let port = listener.local_addr().map(|v| v.port()).unwrap_or(0);
    let mut rate = (Instant::now(), 0u32);
    while !stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((stream, address)) if address.ip().is_loopback() => {
                handle(stream, &app, &token, port, &mut rate)
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => break,
        }
    }
}
