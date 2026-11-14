//! Versioned local-only backup. Export uses a native picker; restore accepts selected file bytes, never a frontend-supplied filesystem path.
use super::*;
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};
const FORMAT: &str = "project-track-local-workspace";
const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct History {
    id: i64,
    task_id: String,
    before: Option<LocalTask>,
    after: LocalTask,
    label: String,
    undone: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Activity {
    id:String, task_id:String, operation_id:String, description:String,
    original_text:String, created_at:i64, history_id:Option<i64>, undone:bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImeOperation {
    idem_key:String, id:String, text:String, mode:String, status:String, reason:String,
    task_id:Option<String>, activity_description:String, progress_after:Option<i64>,
    result:String, created_at:i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Backup {
    format: String,
    version: u32,
    created_at: i64,
    tasks: Vec<LocalTask>,
    history: Vec<History>,
    #[serde(default)]
    activities: Vec<Activity>,
    #[serde(default)]
    ime_operations: Vec<ImeOperation>,
}
pub(super) struct PreparedRestore {
    request_id: String,
    expected_revision: i64,
    backup: Backup,
    prepared_at: Instant,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    path: String,
    active_count: usize,
    trash_count: usize,
    history_count: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePreview {
    request_id: String,
    source: String,
    created_at: i64,
    active_count: usize,
    trash_count: usize,
    history_count: usize,
    current_count: usize,
    sample_titles: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreResult {
    snapshot: Snapshot,
    protection_path: String,
}
fn now() -> Result<i64, String> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(error)?
        .as_secs() as i64)
}
fn same(a: &LocalTask, b: &LocalTask) -> bool {
    same_content(a, b)
}
fn validate_task(t: &LocalTask) -> Result<(), String> {
    if t.id.is_empty()
        || t.id.len() > 128
        || !t
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        || t.revision <= 0
        || t.title.trim().is_empty()
        || t.title.chars().count() > 200
        || t.detail.chars().count() > 10000
        || t.progress > 100
        || t.content_modified_at
            .is_some_and(|v| !(1..=253402300799999).contains(&v))
        || !["iu", "in", "un", "nn"].contains(&t.quadrant.as_str())
    {
        return Err("备份含无效的任务字段".into());
    }
    for d in [&t.start_date, &t.planned_date].into_iter().flatten() {
        if !date_valid(d) {
            return Err("备份含无效日期".into());
        }
    }
    if t.start_date
        .as_ref()
        .zip(t.planned_date.as_ref())
        .is_some_and(|(s, d)| s > d)
    {
        return Err("备份的计划完成日期早于开始日期".into());
    }
    Ok(())
}
impl Backup {
    fn validate(&self) -> Result<(), String> {
        if self.format != FORMAT || ![1, 2, 3].contains(&self.version) {
            return Err("不是本地工作台备份，或备份版本暂不支持".into());
        }
        if !(0..=253402300799).contains(&self.created_at)
            || self.tasks.len() > 10000
            || self.history.len() > 50000 || self.activities.len()>50000 || self.ime_operations.len()>50000
        {
            return Err("备份大小或记录数量超出支持范围".into());
        }
        let mut tasks = HashMap::new();
        for t in &self.tasks {
            validate_task(t)?;
            if tasks.insert(t.id.clone(), t.clone()).is_some() {
                return Err("备份含重复任务编号".into());
            }
        }
        let mut last_id = 0;
        for h in &self.history {
            if h.id <= last_id
                || h.id == i64::MAX
                || h.task_id != h.after.id
                || !tasks.contains_key(&h.task_id)
                || h.label.trim().is_empty()
                || h.label.chars().count() > 240
            {
                return Err("备份的撤销历史不完整或重复".into());
            }
            last_id = h.id;
            validate_task(&h.after)?;
            if let Some(before) = &h.before {
                validate_task(before)?;
                if before.id != h.task_id {
                    return Err("备份的历史任务编号不一致".into());
                }
            }
        }
        // Verify the complete remaining undo chain before any replacement.
        for h in self.history.iter().rev().filter(|h| !h.undone) {
            let current = tasks.get(&h.task_id).ok_or("备份缺少历史对应任务")?;
            if !same(current, &h.after) {
                return Err("备份的撤销历史与任务内容不一致".into());
            }
            let before = h.before.clone().unwrap_or_else(|| {
                let mut t = current.clone();
                t.trashed = true;
                t
            });
            tasks.insert(h.task_id.clone(), before);
        }
        let history_ids:std::collections::HashSet<_>=self.history.iter().map(|h|h.id).collect();
        let mut activity_ids=std::collections::HashSet::new();
        for a in &self.activities {
            if !activity_ids.insert(&a.id)||!tasks.contains_key(&a.task_id)
                ||a.description.trim().is_empty()||a.description.chars().count()>500
                ||a.original_text.chars().count()>500||a.history_id.is_some_and(|id|!history_ids.contains(&id))
            {return Err("备份含无效的输入法完成记录".into());}
        }
        let mut operation_ids=std::collections::HashSet::new();
        let mut keys=std::collections::HashSet::new();
        for o in &self.ime_operations {
            if !operation_ids.insert(&o.id)||!keys.insert(&o.idem_key)||o.text.chars().count()>500
                ||o.task_id.as_ref().is_some_and(|id|!tasks.contains_key(id))
                ||o.progress_after.is_some_and(|p|!(0..=100).contains(&p))
                ||serde_json::from_str::<serde_json::Value>(&o.result).is_err()
            {return Err("备份含无效的输入法操作记录".into());}
        }
        Ok(())
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(error)?;
        if bytes.len() > MAX_BYTES {
            return Err("完整备份超过 16 MiB，未导出或截断数据".into());
        }
        Ok(bytes)
    }
    fn parse(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BYTES {
            return Err("备份不能超过 16 MiB".into());
        }
        let backup: Self = serde_json::from_slice(bytes)
            .map_err(|_| "备份不是有效的工作台 JSON 文件".to_string())?;
        backup.validate()?;
        Ok(backup)
    }
}
fn capture(c: &Connection) -> Result<Backup, String> {
    let mut statement = c
        .prepare("SELECT data FROM local_tasks ORDER BY id")
        .map_err(error)?;
    let rows = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(error)?;
    let tasks = rows
        .map(|r| serde_json::from_str::<LocalTask>(&r.map_err(error)?).map_err(error))
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = c
        .prepare(
            "SELECT id,task_id,before_data,after_data,label,undone FROM local_history ORDER BY id",
        )
        .map_err(error)?;
    let rows = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
            ))
        })
        .map_err(error)?;
    let history = rows
        .map(|r| {
            let (id, task_id, before, after, label, undone) = r.map_err(error)?;
            Ok(History {
                id,
                task_id,
                before: serde_json::from_str(&before).map_err(error)?,
                after: serde_json::from_str(&after).map_err(error)?,
                label,
                undone,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let has_table=|name:&str| -> Result<bool,String>{
        c.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",[name],|r|r.get(0)).map_err(error)
    };
    let activities=if has_table("local_task_activity")? {
        let mut statement=c.prepare("SELECT id,task_id,operation_id,description,original_text,created_at,history_id,undone FROM local_task_activity ORDER BY created_at,id").map_err(error)?;
        let rows=statement.query_map([],|r|Ok(Activity{id:r.get(0)?,task_id:r.get(1)?,operation_id:r.get(2)?,description:r.get(3)?,original_text:r.get(4)?,created_at:r.get(5)?,history_id:r.get(6)?,undone:r.get(7)?})).map_err(error)?
            .map(|r|r.map_err(error)).collect::<Result<Vec<_>,_>>()?;
        rows
    } else {Vec::new()};
    let ime_operations=if has_table("local_ime_operations")? {
        let mut statement=c.prepare("SELECT idem_key,id,text,mode,status,reason,task_id,activity_description,progress_after,result,created_at FROM local_ime_operations ORDER BY created_at,id").map_err(error)?;
        let rows=statement.query_map([],|r|Ok(ImeOperation{idem_key:r.get(0)?,id:r.get(1)?,text:r.get(2)?,mode:r.get(3)?,status:r.get(4)?,reason:r.get(5)?,task_id:r.get(6)?,activity_description:r.get(7)?,progress_after:r.get(8)?,result:r.get(9)?,created_at:r.get(10)?})).map_err(error)?
            .map(|r|r.map_err(error)).collect::<Result<Vec<_>,_>>()?;
        rows
    } else {Vec::new()};
    let backup = Backup {
        format: FORMAT.into(),
        version: 3,
        created_at: now()?,
        tasks,
        history,
        activities,
        ime_operations,
    };
    backup.validate()?;
    Ok(backup)
}
fn read_file(path: &Path) -> Result<Backup, String> {
    let file = File::open(path).map_err(error)?;
    if !file.metadata().map_err(error)?.is_file() {
        return Err("请选择普通备份文件".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    Backup::parse(&bytes)
}
fn write_new(path: &Path, backup: &Backup) -> Result<(), String> {
    let bytes = backup.bytes()?;
    // Never overwrite an existing file, including a prior protection backup.
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                "文件已存在，请选择新的备份文件名；不会覆盖旧备份".into()
            } else {
                error(e)
            }
        })?;
    let result = file.write_all(&bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(path);
        return Err(format!("备份未完成，当前任务未被替换：{e}"));
    }
    // Read back persisted content; protection failures must abort restoration.
    let checked = read_file(path)?;
    if checked.bytes()? != bytes {
        return Err("备份读回核验失败，当前任务未被替换".into());
    }
    Ok(())
}
fn revision(c: &Connection) -> Result<i64, String> {
    c.query_row("SELECT revision FROM local_meta WHERE id=1", [], |r| {
        r.get(0)
    })
    .map_err(error)
}
impl LocalWorkspace {
    fn export_document(&mut self) -> Result<Backup, String> {
        let tx = self.connection.transaction().map_err(error)?;
        let backup = capture(&tx)?;
        tx.commit().map_err(error)?;
        Ok(backup)
    }
    fn prepare_document(
        &mut self,
        backup: Backup,
        source: String,
    ) -> Result<(PreparedRestore, RestorePreview), String> {
        backup.validate()?;
        let tx = self.connection.transaction().map_err(error)?;
        let current = capture(&tx)?;
        let expected_revision = revision(&tx)?;
        let request_id: String = tx
            .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
            .map_err(error)?;
        tx.commit().map_err(error)?;
        let active_count = backup.tasks.iter().filter(|t| !t.trashed).count();
        let preview = RestorePreview {
            request_id: request_id.clone(),
            source,
            created_at: backup.created_at,
            active_count,
            trash_count: backup.tasks.len() - active_count,
            history_count: backup.history.len(),
            current_count: current.tasks.len(),
            sample_titles: backup
                .tasks
                .iter()
                .take(5)
                .map(|t| format!("{}{}", t.title, if t.trashed { "（回收站）" } else { "" }))
                .collect(),
        };
        Ok((
            PreparedRestore {
                request_id,
                expected_revision,
                backup,
                prepared_at: Instant::now(),
            },
            preview,
        ))
    }
    fn replace_document(
        &mut self,
        prepared: &PreparedRestore,
        protection_dir: &Path,
    ) -> Result<RestoreResult, String> {
        prepared.backup.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        if revision(&tx)? != prepared.expected_revision {
            return Err("预览后任务已变化，请重新选择备份并预览；没有替换任务".into());
        }
        let current = capture(&tx)?;
        std::fs::create_dir_all(protection_dir).map_err(error)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(error)?
            .as_nanos();
        let protection_path = protection_dir.join(format!("before-restore-{nonce}.json"));
        write_new(&protection_path, &current)?;
        sync_preview::invalidate_batches_for_restore(&tx)?;
        // One transaction replaces only this local workspace; cloud DB is never opened.
        tx.execute("DELETE FROM local_history", []).map_err(error)?;
        tx.execute("DELETE FROM local_tasks", []).map_err(error)?;
        tx.execute("DELETE FROM local_task_activity", []).map_err(error)?;
        tx.execute("DELETE FROM local_ime_operations", []).map_err(error)?;
        // Links/baselines are not in task backups. A restore cannot reuse old approvals.
        tx.execute("DELETE FROM local_sync_connections", [])
            .map_err(error)?;
        next_revision(&tx)?; // Invalidates stale editors even when restoring an empty file.
        for task in &prepared.backup.tasks {
            let mut task = task.clone();
            task.revision = next_revision(&tx)?;
            put(&tx, &task)?;
        }
        for h in &prepared.backup.history {
            tx.execute("INSERT INTO local_history(id,task_id,before_data,after_data,label,undone) VALUES(?1,?2,?3,?4,?5,?6)",params![h.id,h.task_id,serde_json::to_string(&h.before).map_err(error)?,serde_json::to_string(&h.after).map_err(error)?,h.label,h.undone]).map_err(error)?;
        }
        for a in &prepared.backup.activities {
            tx.execute("INSERT INTO local_task_activity(id,task_id,operation_id,description,original_text,created_at,history_id,undone) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![a.id,a.task_id,a.operation_id,a.description,a.original_text,a.created_at,a.history_id,a.undone]).map_err(error)?;
        }
        for o in &prepared.backup.ime_operations {
            tx.execute("INSERT INTO local_ime_operations(idem_key,id,text,mode,status,reason,task_id,activity_description,progress_after,result,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![o.idem_key,o.id,o.text,o.mode,o.status,o.reason,o.task_id,o.activity_description,o.progress_after,o.result,o.created_at]).map_err(error)?;
        }
        // Acceptance-only proof of a legitimate disconnected state, in the same
        // transaction as replacement. Never restore connections or approvals.
        #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
        {
            tx.execute_batch("CREATE TABLE IF NOT EXISTS local_direct_restore_marker(identifier TEXT PRIMARY KEY,restored_revision INTEGER NOT NULL);").map_err(error)?;
            tx.execute("INSERT INTO local_direct_restore_marker(identifier,restored_revision) VALUES(?1,?2) ON CONFLICT(identifier) DO UPDATE SET restored_revision=excluded.restored_revision",
                params![crate::direct_account_scope::SYNC_IDENTIFIER,revision(&tx)?]).map_err(error)?;
        }
        tx.commit().map_err(error)?;
        Ok(RestoreResult {
            snapshot: self.snapshot()?,
            protection_path: protection_path.display().to_string(),
        })
    }
}
pub(super) fn export(state: &LocalWorkspaceState) -> Result<Option<ExportResult>, String> {
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("导出本地任务备份（不覆盖旧文件）")
        .set_file_name(format!("任务工作台备份-{}.json", now()?))
        .add_filter("工作台 JSON 备份", &["json"])
        .save_file()
    else {
        return Ok(None);
    };
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    {
        path.set_extension("json");
    }
    let backup = state.with(|db| db.export_document())?;
    write_new(&path, &backup)?;
    let active_count = backup.tasks.iter().filter(|t| !t.trashed).count();
    Ok(Some(ExportResult {
        path: path.display().to_string(),
        active_count,
        trash_count: backup.tasks.len() - active_count,
        history_count: backup.history.len(),
    }))
}
pub(super) fn preview(
    state: &LocalWorkspaceState,
    json: &str,
    source: &str,
) -> Result<Option<RestorePreview>, String> {
    *state.pending_restore.lock().map_err(error)? = None;
    if source.is_empty() || source.len() > 1024 {
        return Err("备份文件名称无效".into());
    }
    let backup = Backup::parse(json.as_bytes())?;
    let (prepared, preview) = state.with(|db| db.prepare_document(backup, source.to_string()))?;
    *state.pending_restore.lock().map_err(error)? = Some(prepared);
    Ok(Some(preview))
}
// Read a frozen task-only document from the known previous workspace. Never open
// it through LocalWorkspace::open: that routine performs schema migrations.
pub(super) fn preview_legacy(state: &LocalWorkspaceState, path: &Path) -> Result<Option<RestorePreview>, String> {
    *state.pending_restore.lock().map_err(error)? = None;
    if !path.is_file() { return Err("未找到旧工作台数据；可在旧程序导出完整JSON备份后选择预览。".into()); }
    let source = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(error)?;
    source.busy_timeout(Duration::from_secs(3)).map_err(error)?;
    source.execute_batch("BEGIN DEFERRED").map_err(error)?;
    let integrity: String = source.query_row("PRAGMA quick_check", [], |r| r.get(0)).map_err(error)?;
    if integrity != "ok" { return Err("旧数据完整性检查未通过，未迁移；请先保留原文件。".into()); }
    let document = capture(&source)?;
    let json = String::from_utf8(document.bytes()?).map_err(error)?;
    preview(state, &json, &format!("旧工作台只读快照：{}", path.display()))
}
pub(super) fn cancel(state: &LocalWorkspaceState, id: &str) -> Result<(), String> {
    let mut pending = state.pending_restore.lock().map_err(error)?;
    if pending.as_ref().is_some_and(|p| p.request_id == id) {
        *pending = None;
    }
    Ok(())
}
pub(super) fn confirm(
    state: &LocalWorkspaceState,
    id: &str,
    confirmed: bool,
) -> Result<RestoreResult, String> {
    if !confirmed {
        return Err("请明确确认整体替换当前本地任务".into());
    }
    let mut pending = state.pending_restore.lock().map_err(error)?;
    let prepared = pending
        .as_ref()
        .filter(|p| p.request_id == id)
        .ok_or("恢复预览不存在，请重新选择备份")?;
    if prepared.prepared_at.elapsed() > Duration::from_secs(300) {
        return Err("恢复预览已过期，请重新选择备份".into());
    }
    let dir = state
        .path
        .parent()
        .ok_or("本地存储位置无效")?
        .join("local-backups");
    let result = state.with(|db| db.replace_document(prepared, &dir))?;
    *pending = None;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    #[test]
    fn direct_native_restore_reopens_without_reusing_removed_connection() {
        let root = directory();
        let exe = root.join("acceptance.exe");
        let path = direct_acceptance_database_path(&exe).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let protection = root.join("protection");
        {
            let mut db = LocalWorkspace::open(&path).unwrap();
            for (name, progress) in [("A", 25), ("B", 75)] {
                let mut task = input(None, progress);
                task.title = format!("【同步验收{name}】恢复重开测试");
                task.detail = format!("DIRECT-20261020-{name}");
                db.save(task).unwrap();
            }
            db.connection.execute("INSERT INTO local_sync_connections VALUES('test','feishu',?1,?2,?3)",
                params![format!("{}:synthetic-test",crate::direct_account_scope::APP_ID),crate::direct_account_scope::APP_TOKEN,crate::direct_account_scope::TABLE_ID]).unwrap();
            let backup = db.export_document().unwrap();
            let before = db.snapshot().unwrap().tasks.remove(0);
            let mut edit = input(Some(&before), 60);
            edit.title = before.title;
            edit.detail = before.detail;
            db.save(edit).unwrap();
            let (prepared, _) = db.prepare_document(backup, "only-synthetic.json".into()).unwrap();
            db.replace_document(&prepared, &protection).unwrap();
        }
        let state = LocalWorkspaceState::prepared_direct_from_exe(&exe)
            .unwrap_or_else(|e| panic!("legitimate native restore must reopen, without auto-connecting: {e}"));
        let snapshot = state.with(|db| db.snapshot()).unwrap();
        let mut progress: Vec<_> = snapshot.tasks.iter().map(|t| t.progress).collect();
        progress.sort();
        assert_eq!(progress, [25, 75]);
        assert_eq!(state.with(|db| db.connection.query_row("SELECT count(*) FROM local_sync_connections", [], |r| r.get::<_,i64>(0)).map_err(error)).unwrap(), 0);
        state.with(|db| db.connection.execute("INSERT INTO local_sync_connections VALUES('wrong','feishu','wrong-account','wrong-base','wrong-table')", []).map_err(error)).unwrap();
        drop(state);
        assert!(matches!(LocalWorkspaceState::prepared_direct_from_exe(&exe), Err(ref e) if e.contains("连接范围不匹配")), "a restore marker cannot bypass a wrong connected scope");
        cleanup(protection);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    fn input(task: Option<&LocalTask>, p: i64) -> SaveTask {
        SaveTask {
            id: task.map(|t| t.id.clone()),
            expected_revision: task.map(|t| t.revision),
            title: "备份测试任务".into(),
            detail: "测试数据".into(),
            progress: p,
            quadrant: "in".into(),
            today: true,
            start_date: None,
            planned_date: Some("2026-10-20".into()),
        }
    }
    fn db() -> LocalWorkspace {
        LocalWorkspace::open(Path::new(":memory:")).unwrap()
    }
    #[test]
    fn ime_activity_and_pending_review_survive_full_backup_restore() {
        let mut source=db();
        let mut seed=input(None,60);seed.title="资料核对任务".into();source.save(seed).unwrap();
        let applied=source.ingest_utterance("backup-applied-key-01",super::super::ime::Utterance{
            text:"今日完成资料核对任务，达到百分之百".into(),mode:"applyIfExact".into()}).unwrap();
        source.ingest_utterance("backup-pending-key-01",super::super::ime::Utterance{
            text:"今日完成另一项任务".into(),mode:"applyIfExact".into()}).unwrap();
        let backup=source.export_document().unwrap();
        assert_eq!(backup.version,3);
        assert_eq!(backup.activities.len(),1);
        assert_eq!(backup.ime_operations.len(),2);
        let mut target=db();
        let (prepared,_)=target.prepare_document(backup,"ime-backup.json".into()).unwrap();
        let protection=directory();
        target.replace_document(&prepared,&protection).unwrap();
        assert_eq!(target.snapshot().unwrap().tasks[0].progress,100);
        assert_eq!(target.ime_reviews().unwrap().len(),1);
        assert_eq!(target.ime_activities(&target.snapshot().unwrap().tasks[0].id).unwrap().len(),1);
        assert_eq!(target.ime_operation(applied["operationId"].as_str().unwrap()).unwrap().unwrap()["status"],"applied");
        cleanup(protection);
    }
    #[test]
    fn legacy_preview_is_readonly_and_confirm_preserves_current_data() {
        let folder=directory();
        let source_path=folder.join("old.sqlite3");
        { let mut old=LocalWorkspace::open(&source_path).unwrap();old.save(input(None,71)).unwrap(); }
        let before=std::fs::read(&source_path).unwrap();
        let state=LocalWorkspaceState::new(folder.join("current.sqlite3"));
        state.with(|db|db.save(input(None,20))).unwrap();
        let preview=preview_legacy(&state,&source_path).unwrap().unwrap();
        assert_eq!(preview.active_count,1);
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks[0].progress,20);
        assert_eq!(before,std::fs::read(&source_path).unwrap());
        assert!(confirm(&state,&preview.request_id,false).is_err());
        let restored=confirm(&state,&preview.request_id,true).unwrap();
        assert_eq!(restored.snapshot.tasks[0].progress,71);
        assert_eq!(read_file(Path::new(&restored.protection_path)).unwrap().tasks[0].progress,20);
        assert_eq!(before,std::fs::read(&source_path).unwrap());
        assert!(confirm(&state,&preview.request_id,true).is_err());
        drop(state);
        cleanup(folder.join("local-backups"));
        for entry in std::fs::read_dir(&folder).unwrap(){std::fs::remove_file(entry.unwrap().path()).unwrap();}
        std::fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn legacy_preview_missing_or_wrong_schema_clears_old_confirmation_without_creating_source() {
        let folder=directory();let missing=folder.join("missing.sqlite3");
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let json=String::from_utf8(db().export_document().unwrap().bytes().unwrap()).unwrap();
        let old=preview(&state,&json,"fixture.json").unwrap().unwrap();
        assert!(preview_legacy(&state,&missing).is_err());assert!(!missing.exists());
        assert!(confirm(&state,&old.request_id,true).is_err());
        let wrong=folder.join("cloud-cache.sqlite3");
        {let c=Connection::open(&wrong).unwrap();c.execute_batch("CREATE TABLE tasks(id TEXT)").unwrap();}
        let before=std::fs::read(&wrong).unwrap();
        assert!(preview_legacy(&state,&wrong).is_err());
        assert_eq!(before,std::fs::read(&wrong).unwrap());
        cleanup(folder);
    }
    #[test]
    #[ignore = "generates a synthetic fixture for the isolated native backup UI test"]
    fn write_native_backup_fixture() {
        let mut db = db();
        db.seed_acceptance_examples().unwrap();
        let backup = db.export_document().unwrap();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("output/playwright");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join(format!("local-backup-fixture-{}.json", now().unwrap()));
        write_new(&file, &backup).unwrap();
        println!("fixture: {}", file.display());
    }
    // Opt-in local fixture only: persist intent with the production core, never send it.
    // A byte-identical existing rev30 binary can open this separate adjacent profile.
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    #[test]
    #[ignore = "creates a new synthetic pending-write native fixture; no network or credentials"]
    fn write_direct_pending_native_fixture() {
        use super::super::sync_preview as sync;
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf();
        let source = root.join("outputs/direct-sync-acceptance-20261020/local-workspace.sqlite3");
        let source_db = Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(source_db.query_row("SELECT count(*) FROM local_sync_connections", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
        let scope: String = source_db.query_row("SELECT scope FROM local_sync_operations ORDER BY rowid LIMIT 1", [], |r| r.get(0)).unwrap();
        let scope: serde_json::Value = serde_json::from_str(&scope).unwrap();
        assert_eq!(scope["platform"], "feishu");
        assert_eq!(scope["spaceId"], crate::direct_account_scope::APP_TOKEN);
        assert_eq!(scope["tableId"], crate::direct_account_scope::TABLE_ID);
        let subject = scope["subjectId"].as_str().unwrap().to_string();
        assert!(subject.starts_with(&format!("{}:", crate::direct_account_scope::APP_ID)));
        let original = Backup::parse(&std::fs::read(root.join("outputs/direct-restore-baseline-20261020.json")).unwrap()).unwrap();
        assert_eq!(original.tasks.len(), 2);
        assert!(original.tasks.iter().all(|t| t.title.starts_with("【同步验收") && t.detail.contains("DIRECT-20261020-")));
        let folder = root.join("outputs/pending-restore-native-20261020");
        // Refuse any existing target, including partial prior preparation; never overwrite.
        std::fs::create_dir(&folder).unwrap();
        let exe = folder.join("本地任务工作台_合成待核实恢复验收_rev30.exe");
        let path = direct_acceptance_database_path(&exe).unwrap();
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        let mut db = LocalWorkspace::open(&path).unwrap();
        let mut other = LocalWorkspace::open(Path::new(":memory:")).unwrap();
        let mut seed = original.tasks.clone();
        seed.sort_by_key(|t| t.progress);
        for t in &seed {
            let make = |p| SaveTask {
                id: None, expected_revision: None, title: t.title.clone(),
                detail: format!("{}\nSYNTHETIC-PENDING-NO-SEND：临时验收数据，未发送云端请求", t.detail),
                progress: p, quadrant: t.quadrant.clone(), today: t.today,
                start_date: t.start_date.clone(), planned_date: t.planned_date.clone(),
            };
            db.save(make(if t.progress == 25 { 40 } else { 30 })).unwrap();
            other.save(make(t.progress.into())).unwrap();
        }
        let local = db.snapshot().unwrap().tasks.into_iter().find(|t| t.progress == 40).unwrap();
        let cloud_task = &seed[0];
        let read = crate::feishu::SyncInspection {
            read_only: true, complete: true, account_name: "合成验收（非平台结果）".into(),
            subject_id: subject, app_id: crate::direct_account_scope::APP_ID.into(),
            app_token: crate::direct_account_scope::APP_TOKEN.into(), table_id: crate::direct_account_scope::TABLE_ID.into(),
            checked_at: content_now().unwrap(), conditional_write_verified: false, warnings: vec![],
            records: vec![crate::feishu::SyncRecordTime {
                record_id: crate::direct_account_scope::RECORD_IDS[0].into(), title: cloud_task.title.clone(),
                progress: cloud_task.progress, status: "进行中".into(), planned_date: cloud_task.planned_date.clone(),
                modified_at: Some(content_now().unwrap() - 1000), time_source: "platformSystem".into(),
                shared_fields: serde_json::json!({"title":cloud_task.title,"detail":cloud_task.detail,
                    "progress":cloud_task.progress,"quadrant":cloud_task.quadrant,"today":cloud_task.today,
                    "startDate":cloud_task.start_date,"plannedDate":cloud_task.planned_date}),
            }],
        };
        let cloud = &read.records[0];
        sync::save_pairing(&mut db.connection, &local, cloud, &read, &sync::SavePairingInput {
            local_id: local.id.clone(), cloud_id: cloud.record_id.clone(),
            expected_local_revision: local.revision, expected_local_modified_at: local.content_modified_at,
            expected_cloud_modified_at: cloud.modified_at,
        }).unwrap();
        let choice: sync::BatchInput = serde_json::from_value(serde_json::json!({"entries":[{
            "direction":"local", "input":{"localId":local.id,"cloudId":cloud.record_id,"subjectId":read.subject_id,
                "expectedLocalRevision":local.revision,"expectedLocalModifiedAt":local.content_modified_at,
                "expectedCloudModifiedAt":cloud.modified_at,"expectedCloudFields":cloud.shared_fields}
        }]})).unwrap();
        let job = sync::prepare_batch(&mut db.connection, &read, choice.clone()).unwrap();
        let claimed = sync::claim_batch(&mut db.connection, &job.id, &read.subject_id).unwrap().unwrap();
        let operation = sync::begin_upload(&mut db.connection, &read, &claimed.input).unwrap();
        sync::attach_batch_operation(&mut db.connection, &job.id, &operation.id).unwrap();
        assert_eq!(sync::finish_batch(&mut db.connection, &job.id, "uncertain").unwrap().state, "uncertain");
        // This is the hard boundary: no upload_send/Feishu client/vault call occurs here.
        assert!(sync::begin_upload(&mut db.connection, &read, &claimed.input).unwrap_err().contains("待核实"));
        let backup = other.export_document().unwrap();
        assert!(backup.tasks.iter().all(|t| t.id != local.id));
        write_new(&folder.join("synthetic-cross-id-backup.json"), &backup).unwrap();
        let manifest = serde_json::json!({"synthetic":true,"cloudRequestSent":false,"credentialsCopied":false,
            "beforeProgress":[40,30],"backupProgress":[25,75],"oldLocalId":local.id,
            "newLocalIds":backup.tasks.iter().map(|t|t.id.clone()).collect::<Vec<_>>(),
            "pendingOperationId":operation.id,"uncertainBatchId":job.id});
        let mut file = OpenOptions::new().write(true).create_new(true).open(folder.join("fixture-manifest.json")).unwrap();
        file.write_all(&serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
        file.sync_all().unwrap();
        drop(db);
        let state = LocalWorkspaceState::prepared_direct_from_exe(&exe).unwrap();
        assert_eq!(state.with(|db| db.connection.query_row("SELECT count(*) FROM local_sync_operations WHERE state='pending'", [], |r| r.get::<_,i64>(0)).map_err(error)).unwrap(), 1);
        println!("Synthetic native fixture ready; no send, no vault access: {}", folder.display());
    }
    #[test]
    fn invalid_preview_clears_pending_and_never_changes_tasks() {
        let state = LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let backup = state
            .with(|db| {
                db.save(input(None, 20))?;
                db.export_document()
            })
            .unwrap();
        let json = String::from_utf8(backup.bytes().unwrap()).unwrap();
        let prepared = preview(&state, &json, "fixture.json").unwrap().unwrap();
        assert!(state.pending_restore.lock().unwrap().is_some());
        assert!(preview(&state, "invalid", "bad.json").is_err());
        assert!(state.pending_restore.lock().unwrap().is_none());
        assert!(confirm(&state, &prepared.request_id, true).is_err());
        assert_eq!(
            state.with(|db| db.snapshot()).unwrap().tasks[0].progress,
            20
        );
    }
    #[test]
    fn restoring_empty_backup_is_explicit_and_preserves_original_in_protection() {
        let mut db = db();
        let empty = db.export_document().unwrap();
        db.save(input(None, 20)).unwrap();
        let (prepared, preview) = db.prepare_document(empty, "empty.json".into()).unwrap();
        assert_eq!(preview.active_count, 0);
        let dir = directory();
        let result = db.replace_document(&prepared, &dir).unwrap();
        assert!(result.snapshot.tasks.is_empty());
        assert!(result.snapshot.undo_label.is_none());
        assert_eq!(
            read_file(Path::new(&result.protection_path))
                .unwrap()
                .tasks
                .len(),
            1
        );
        cleanup(dir);
    }
    fn directory() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "local-backup-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
    fn cleanup(path: PathBuf) {
        for entry in std::fs::read_dir(&path).unwrap() {
            std::fs::remove_file(entry.unwrap().path()).unwrap();
        }
        std::fs::remove_dir(path).unwrap();
    }
    #[test]
    fn full_roundtrip_preserves_trash_and_undo() {
        let mut db = db();
        let t = db.save(input(None, 20)).unwrap().tasks.remove(0);
        let t = db.save(input(Some(&t), 71)).unwrap().tasks.remove(0);
        db.trash(&t.id, t.revision, true).unwrap();
        let backup = db.export_document().unwrap();
        let parsed = Backup::parse(&backup.bytes().unwrap()).unwrap();
        assert_eq!(parsed.tasks, backup.tasks);
        assert_eq!(parsed.history.len(), 3);
        assert!(parsed.tasks[0].trashed);
        let (prepared, _) = db.prepare_document(parsed, "fixture".into()).unwrap();
        let dir = directory();
        let result = db.replace_document(&prepared, &dir).unwrap();
        assert!(result.snapshot.tasks[0].trashed);
        assert!(!db.undo().unwrap().tasks[0].trashed);
        assert_eq!(db.undo().unwrap().tasks[0].progress, 20);
        cleanup(dir);
    }
    #[test]
    fn v1_backup_missing_time_remains_readable_and_never_fabricates_history_dates() {
        let mut db = db();
        let t = db.save(input(None, 25)).unwrap().tasks.remove(0);
        db.save(input(Some(&t), 71)).unwrap();
        let mut old = serde_json::to_value(db.export_document().unwrap()).unwrap();
        old["version"] = 1.into();
        for t in old["tasks"].as_array_mut().unwrap() {
            t.as_object_mut().unwrap().remove("contentModifiedAt");
        }
        for h in old["history"].as_array_mut().unwrap() {
            for name in ["before", "after"] {
                if let Some(task) = h[name].as_object_mut() {
                    task.remove("contentModifiedAt");
                }
            }
        }
        let old = Backup::parse(old.to_string().as_bytes()).unwrap();
        assert!(old.tasks[0].content_modified_at.is_none());
        let (prepared, _) = db.prepare_document(old, "v1".into()).unwrap();
        let dir = directory();
        let restored = db.replace_document(&prepared, &dir).unwrap();
        assert!(restored.snapshot.tasks[0].content_modified_at.is_none());
        let t = db.undo().unwrap().tasks.remove(0);
        assert_eq!(t.progress, 25);
        assert!(t.content_modified_at.is_some());
        cleanup(dir);
    }
    #[test]
    fn restore_invalidates_old_batches_but_preserves_uncertain_write_evidence() {
        use super::super::sync_preview as sync;
        for mode in ["ready", "stopped", "uncertain", "crossId"] {
            let uncertain = ["uncertain", "crossId"].contains(&mode);
            let dir = directory();
            let path = dir.join("workspace.sqlite3");
            let mut db = LocalWorkspace::open(&path).unwrap();
            let task = db.save(input(None, 25)).unwrap().tasks.remove(0);
            let read = crate::feishu::SyncInspection {
                read_only: true,
                complete: true,
                account_name: "test".into(),
                subject_id: "cli:open".into(),
                app_id: "cli".into(),
                app_token: "base".into(),
                table_id: "table".into(),
                checked_at: content_now().unwrap(),
                conditional_write_verified: false,
                warnings: vec![],
                records: vec![crate::feishu::SyncRecordTime {
                    record_id: "cloud".into(),
                    title: task.title.clone(),
                    progress: 60,
                    status: "进行中".into(),
                    planned_date: task.planned_date.clone(),
                    modified_at: Some(content_now().unwrap() - 1000),
                    time_source: "platformSystem".into(),
                    shared_fields: serde_json::json!({"title":task.title,"detail":task.detail,
                        "progress":60,"quadrant":task.quadrant,"today":task.today,
                        "startDate":task.start_date,"plannedDate":task.planned_date}),
                }],
            };
            let cloud = &read.records[0];
            sync::save_pairing(
                &mut db.connection,
                &task,
                cloud,
                &read,
                &sync::SavePairingInput {
                    local_id: task.id.clone(),
                    cloud_id: cloud.record_id.clone(),
                    expected_local_revision: task.revision,
                    expected_local_modified_at: task.content_modified_at,
                    expected_cloud_modified_at: cloud.modified_at,
                },
            )
            .unwrap();
            let choice: sync::BatchInput = serde_json::from_value(serde_json::json!({"entries":[{
                "direction":"local", "input":{
                    "localId":task.id,"cloudId":cloud.record_id,"subjectId":read.subject_id,
                    "expectedLocalRevision":task.revision,"expectedLocalModifiedAt":task.content_modified_at,
                    "expectedCloudModifiedAt":cloud.modified_at,"expectedCloudFields":cloud.shared_fields
                }
            }]})).unwrap();
            let job = sync::prepare_batch(&mut db.connection, &read, choice.clone()).unwrap();
            let mut operation_id = None;
            if mode == "stopped" {
                sync::control_batch(&mut db.connection, &job.id, "stop").unwrap();
            } else if uncertain {
                let claimed = sync::claim_batch(&mut db.connection, &job.id, &read.subject_id)
                    .unwrap()
                    .unwrap();
                let op = sync::begin_upload(&mut db.connection, &read, &claimed.input).unwrap();
                sync::attach_batch_operation(&mut db.connection, &job.id, &op.id).unwrap();
                sync::finish_batch(&mut db.connection, &job.id, "uncertain").unwrap();
                operation_id = Some(op.id);
            }
            let exported = if mode == "crossId" {
                let mut other = LocalWorkspace::open(Path::new(":memory:")).unwrap();
                let new_task = other.save(input(None, 25)).unwrap().tasks.remove(0);
                assert_ne!(new_task.id, task.id);
                other.export_document().unwrap().bytes().unwrap()
            } else {
                db.export_document().unwrap().bytes().unwrap()
            };
            let (prepared, _) = db
                .prepare_document(Backup::parse(&exported).unwrap(), "export.json".into())
                .unwrap();
            // A failed restore must roll back revocation together with the task replacement.
            db.connection.execute_batch("CREATE TRIGGER fail_restore BEFORE DELETE ON local_tasks BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
            assert!(db.replace_document(&prepared, &dir).is_err());
            assert_eq!(db.snapshot().unwrap().tasks[0], task);
            let stopped = sync::control_batch(&mut db.connection, &job.id, "stop").unwrap();
            assert_eq!(
                stopped.state,
                if uncertain { "uncertain" } else { "stopped" }
            );
            if mode == "ready" {
                assert_eq!(
                    sync::control_batch(&mut db.connection, &job.id, "resume")
                        .unwrap()
                        .state,
                    "ready"
                );
            }
            db.connection
                .execute_batch("DROP TRIGGER fail_restore")
                .unwrap();
            db.replace_document(&prepared, &dir).unwrap();
            drop(db);
            let mut db = LocalWorkspace::open(&path).unwrap();
            let local = db.snapshot().unwrap().tasks;
            assert!(sync::claim_batch(&mut db.connection, &job.id, &read.subject_id).is_err());
            assert!(sync::control_batch(&mut db.connection, &job.id, "resume").is_err());
            assert!(sync::reconcile_batch(&mut db.connection, &job.id, &read).is_err());
            assert!(sync::finish_batch(&mut db.connection, &job.id, "completed").is_err());
            // Explicitly re-pairing the same account/table cannot revive old approvals.
            sync::save_pairing(
                &mut db.connection,
                &local[0],
                &read.records[0],
                &read,
                &sync::SavePairingInput {
                    local_id: local[0].id.clone(),
                    cloud_id: "cloud".into(),
                    expected_local_revision: local[0].revision,
                    expected_local_modified_at: local[0].content_modified_at,
                    expected_cloud_modified_at: read.records[0].modified_at,
                },
            )
            .unwrap();
            assert!(sync::claim_batch(&mut db.connection, &job.id, &read.subject_id).is_err());
            assert!(sync::control_batch(&mut db.connection, &job.id, "resume").is_err());
            let mut fresh = choice;
            fresh.entries[0].input.local_id = local[0].id.clone();
            fresh.entries[0].input.expected_local_revision = local[0].revision;
            fresh.entries[0].input.expected_local_modified_at = local[0].content_modified_at;
            let new_plan = sync::prepare_batch(&mut db.connection, &read, fresh.clone());
            if uncertain {
                assert!(new_plan.unwrap_err().contains("待核实"));
                assert!(
                    sync::begin_upload(&mut db.connection, &read, &fresh.entries[0].input)
                        .unwrap_err()
                        .contains("待核实")
                );
            } else {
                assert_eq!(new_plan.unwrap().state, "ready");
            }
            let preview =
                serde_json::to_value(sync::pairing_preview(&db.connection, &local, read).unwrap())
                    .unwrap();
            let old_job = preview["batchJobs"]
                .as_array()
                .unwrap()
                .iter()
                .find(|j| j["id"] == job.id)
                .unwrap();
            assert_eq!(
                old_job["state"], "blocked",
                "restored {mode} batch must lose execution approval"
            );
            if let Some(id) = operation_id {
                assert_eq!(preview["uploadOperations"].as_array().unwrap().len(), 1);
                assert_eq!(preview["uploadOperations"][0]["id"], id);
                assert_eq!(preview["uploadOperations"][0]["state"], "pending");
            }
            assert_eq!(local[0].progress, 25);
            drop(db);
            cleanup(dir);
        }
    }
    #[test]
    fn v2_restore_preserves_source_time_and_detaches_sync_connections() {
        let mut db = db();
        let t = db.save(input(None, 25)).unwrap().tasks.remove(0);
        let backup = db.export_document().unwrap();
        assert_eq!(backup.version, 3);
        db.connection
            .execute(
                "INSERT INTO local_sync_connections VALUES('id','feishu','subject','app','table')",
                [],
            )
            .unwrap();
        db.connection
            .execute(
                "INSERT INTO local_sync_links VALUES('id',?1,'record','{}')",
                [&t.id],
            )
            .unwrap();
        let (prepared, _) = db.prepare_document(backup, "v2".into()).unwrap();
        let dir = directory();
        let restored = db.replace_document(&prepared, &dir).unwrap();
        assert_eq!(
            restored.snapshot.tasks[0].content_modified_at,
            t.content_modified_at
        );
        for table in ["local_sync_connections", "local_sync_links"] {
            assert_eq!(
                db.connection
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r
                        .get::<_, i64>(0))
                    .unwrap(),
                0
            );
        }
        cleanup(dir);
    }
    #[test]
    fn restore_creates_readable_protection_and_invalidates_old_edits() {
        let mut db = db();
        let t = db.save(input(None, 20)).unwrap().tasks.remove(0);
        let backup = db.export_document().unwrap();
        let current = db.save(input(Some(&t), 71)).unwrap().tasks.remove(0);
        let (prepared, _) = db.prepare_document(backup, "fixture".into()).unwrap();
        let dir = directory();
        let result = db.replace_document(&prepared, &dir).unwrap();
        assert_eq!(result.snapshot.tasks[0].progress, 20);
        assert_eq!(
            read_file(Path::new(&result.protection_path)).unwrap().tasks[0].progress,
            71
        );
        assert!(db.save(input(Some(&current), 99)).is_err());
        cleanup(dir);
    }
    #[test]
    fn preview_is_read_only_and_stale_confirmation_is_rejected() {
        let mut db = db();
        let t = db.save(input(None, 20)).unwrap().tasks.remove(0);
        let backup = db.export_document().unwrap();
        let (prepared, preview) = db.prepare_document(backup, "fixture".into()).unwrap();
        assert_eq!(db.snapshot().unwrap().tasks[0], t);
        assert_eq!(preview.active_count, 1);
        db.save(input(Some(&t), 71)).unwrap();
        let dir = directory();
        assert!(db.replace_document(&prepared, &dir).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        assert_eq!(db.snapshot().unwrap().tasks[0].progress, 71);
        cleanup(dir);
    }
    #[test]
    fn protection_write_failure_aborts_restore() {
        let mut db = db();
        db.save(input(None, 20)).unwrap();
        let backup = db.export_document().unwrap();
        let (prepared, _) = db.prepare_document(backup, "fixture".into()).unwrap();
        let dir = directory();
        let blocker = dir.join("not-a-directory");
        std::fs::write(&blocker, b"preserve").unwrap();
        assert!(db.replace_document(&prepared, &blocker).is_err());
        assert_eq!(db.snapshot().unwrap().tasks[0].progress, 20);
        cleanup(dir);
    }
    #[test]
    fn database_failure_rolls_back_after_protection() {
        let mut db = db();
        db.save(input(None, 20)).unwrap();
        let backup = db.export_document().unwrap();
        let (prepared, _) = db.prepare_document(backup, "fixture".into()).unwrap();
        db.connection.execute_batch("CREATE TRIGGER reject_restore BEFORE DELETE ON local_tasks BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
        let dir = directory();
        assert!(db.replace_document(&prepared, &dir).is_err());
        assert_eq!(db.snapshot().unwrap().tasks[0].progress, 20);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        assert_eq!(db.export_document().unwrap().history.len(), 1);
        cleanup(dir);
    }
    #[test]
    fn invalid_files_and_history_are_rejected() {
        let mut db = db();
        db.save(input(None, 20)).unwrap();
        let backup = db.export_document().unwrap();
        assert!(Backup::parse(b"not json").is_err());
        assert!(Backup::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
        let mut bad = backup.clone();
        bad.version = 4;
        assert!(bad.validate().is_err());
        let mut bad = backup.clone();
        bad.tasks.push(bad.tasks[0].clone());
        assert!(bad.validate().is_err());
        let mut bad = backup.clone();
        bad.tasks[0].quadrant = "evil".into();
        assert!(bad.validate().is_err());
        let mut bad = backup.clone();
        bad.history[0].after.progress = 99;
        assert!(bad.validate().is_err());
        let mut value = serde_json::to_value(&backup).unwrap();
        value["secret"] = serde_json::json!("not allowed");
        assert!(Backup::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
    #[test]
    fn backup_does_not_overwrite_existing_file() {
        let mut db = db();
        let backup = db.export_document().unwrap();
        let dir = directory();
        let path = dir.join("backup.json");
        write_new(&path, &backup).unwrap();
        let original = std::fs::read(&path).unwrap();
        assert!(write_new(&path, &backup).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        cleanup(dir);
    }
    #[test]
    fn remaining_history_with_undo_and_new_edits_roundtrips() {
        let mut db = db();
        let t = db.save(input(None, 20)).unwrap().tasks.remove(0);
        db.save(input(Some(&t), 71)).unwrap();
        let t = db.undo().unwrap().tasks.remove(0);
        db.save(input(Some(&t), 42)).unwrap();
        assert!(db.export_document().unwrap().validate().is_ok());
    }
    #[test]
    fn confirmation_requires_explicit_choice_and_matching_preview() {
        let state = LocalWorkspaceState::new(PathBuf::from(":memory:"));
        assert!(confirm(&state, "unknown", false).is_err());
        assert!(confirm(&state, "unknown", true).is_err());
        let backup = state.with(|db| db.export_document()).unwrap();
        let (prepared, preview) = state
            .with(|db| db.prepare_document(backup, "fixture".into()))
            .unwrap();
        *state.pending_restore.lock().unwrap() = Some(prepared);
        cancel(&state, &preview.request_id).unwrap();
        assert!(state.pending_restore.lock().unwrap().is_none());
    }
    #[test]
    fn confirmation_expires_and_success_is_single_use() {
        let dir = directory();
        let state = LocalWorkspaceState::new(dir.join("local-workspace.sqlite3"));
        state.with(|db| db.save(input(None, 20))).unwrap();
        let backup = state.with(|db| db.export_document()).unwrap();
        let (mut prepared, preview) = state
            .with(|db| db.prepare_document(backup.clone(), "fixture".into()))
            .unwrap();
        prepared.prepared_at = Instant::now() - Duration::from_secs(301);
        *state.pending_restore.lock().unwrap() = Some(prepared);
        assert!(confirm(&state, &preview.request_id, true)
            .unwrap_err()
            .contains("过期"));
        assert!(!dir.join("local-backups").exists());
        let (prepared, preview) = state
            .with(|db| db.prepare_document(backup, "fixture".into()))
            .unwrap();
        *state.pending_restore.lock().unwrap() = Some(prepared);
        assert!(confirm(&state, &preview.request_id, false).is_err());
        assert!(confirm(&state, "mismatched", true).is_err());
        assert_eq!(
            confirm(&state, &preview.request_id, true)
                .unwrap()
                .snapshot
                .tasks[0]
                .progress,
            20
        );
        assert!(confirm(&state, &preview.request_id, true).is_err());
        assert_eq!(
            std::fs::read_dir(dir.join("local-backups"))
                .unwrap()
                .count(),
            1
        );
        drop(state);
        cleanup(dir.join("local-backups"));
        cleanup(dir);
    }
}
