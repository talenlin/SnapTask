//! Durable sequential checkpoints. No network and no automatic restart/retry.
use super::*;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchInput {
    pub entries: Vec<BatchChoice>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatchChoice {
    pub direction: String,
    pub input: DownloadInput,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    choice: BatchChoice,
    state: String,
    frozen_local: Option<Content>,
    #[serde(default)]
    operation_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    id: String,
    scope: Scope,
    entries: Vec<Entry>,
    stopped: bool,
    #[serde(default)]
    invalidated_by_restore: bool,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchSummary {
    pub id: String,
    pub state: String,
    pub total: usize,
    pub completed: usize,
    pub queued: usize,
    pub message: String,
    pub entries: Vec<serde_json::Value>,
}
pub fn initialize(c: &Connection) -> Result<(), String> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS local_sync_batches(id TEXT PRIMARY KEY,scope TEXT NOT NULL,data TEXT NOT NULL);").map_err(error)
}
fn summary(j: &Job) -> BatchSummary {
    let completed = j.entries.iter().filter(|e| e.state == "completed").count();
    let state = if j.invalidated_by_restore {
        "blocked"
    } else if j
        .entries
        .iter()
        .any(|e| ["running", "uncertain"].contains(&e.state.as_str()))
    {
        "uncertain"
    } else if j.entries.iter().any(|e| e.state == "blocked") {
        "blocked"
    } else if completed == j.entries.len() {
        "completed"
    } else if j.stopped {
        "stopped"
    } else {
        "ready"
    };
    BatchSummary{id:j.id.clone(),state:state.into(),total:j.entries.len(),completed,
        entries:j.entries.iter().map(|e|serde_json::json!({"direction":e.choice.direction,"state":e.state,"input":e.choice.input,"localContent":e.frozen_local,"operationId":e.operation_id})).collect(),
        queued:j.entries.iter().filter(|e|e.state=="queued").count(),message:match state {
            "uncertain"=>"正在执行或结果待核实；禁止重发，请核对单条操作记录。",
            "blocked" if j.invalidated_by_restore=>"备份恢复已使旧批次许可失效；日志保留供核查，重新确认连接并制定新计划。",
            "blocked"=>"版本变化或检查失败，本批已停止；重新检查后制定新计划。",
            "stopped"=>"已停止，未开始的记录保留；继续需人工确认并重新复查。",
            "completed"=>"本批逐条读回核实完成。",_=>"计划已保存；重启不会自动执行，需人工启动。"}.into()}
}
fn load(c: &Connection, id: &str) -> Result<Job, String> {
    let data: String = c
        .query_row(
            "SELECT data FROM local_sync_batches WHERE id=?1",
            [id],
            |r| r.get(0),
        )
        .map_err(error)?;
    serde_json::from_str(&data).map_err(error)
}
fn save(c: &Connection, j: &Job) -> Result<(), String> {
    c.execute(
        "UPDATE local_sync_batches SET data=?1 WHERE id=?2",
        params![serde_json::to_string(j).map_err(error)?, j.id],
    )
    .map_err(error)?;
    Ok(())
}
fn require_active(j: &Job) -> Result<(), String> {
    if j.invalidated_by_restore {
        Err("备份恢复已使旧批次许可失效，请重新确认连接；旧日志仅供核查".into())
    } else {
        Ok(())
    }
}
// Called inside the restore transaction. Preserve evidence; revoke execution approvals.
pub fn invalidate_for_restore(c: &Connection) -> Result<(), String> {
    let mut statement = c
        .prepare("SELECT data FROM local_sync_batches")
        .map_err(error)?;
    let jobs = statement
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    drop(statement);
    for data in jobs {
        let mut job: Job = serde_json::from_str(&data).map_err(error)?;
        if summary(&job).state != "completed" {
            job.invalidated_by_restore = true;
            save(c, &job)?;
        }
    }
    Ok(())
}
pub fn list(c: &Connection, scope: &Scope) -> Result<Vec<BatchSummary>, String> {
    let mut q = c
        .prepare("SELECT data FROM local_sync_batches WHERE scope=?1 ORDER BY rowid DESC")
        .map_err(error)?;
    let rows = q
        .query_map([serde_json::to_string(scope).map_err(error)?], |r| {
            r.get::<_, String>(0)
        })
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    rows.into_iter()
        .map(|s| {
            serde_json::from_str::<Job>(&s)
                .map(|j| summary(&j))
                .map_err(error)
        })
        .collect()
}
pub fn prepare(
    c: &mut Connection,
    read: &crate::feishu::SyncInspection,
    input: BatchInput,
) -> Result<BatchSummary, String> {
    if !read.read_only || !read.complete || input.entries.is_empty() || input.entries.len() > 50 {
        return Err("批量计划须为完整检查后的1–50条已有对应记录".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let scope = scope_from_inspection(read);
    if list(&tx, &scope)?
        .iter()
        .any(|j| ["ready", "stopped", "uncertain"].contains(&j.state.as_str()))
    {
        return Err("已有未完成批次，请先处理；不能建立重复计划".into());
    }
    let links = existing_links(&tx, &scope)?;
    let mut local_ids = std::collections::HashSet::new();
    let mut cloud_ids = std::collections::HashSet::new();
    for e in &input.entries {
        let i = &e.input;
        if !["local", "cloud"].contains(&e.direction.as_str())
            || !local_ids.insert(&i.local_id)
            || !cloud_ids.insert(&i.cloud_id)
            || i.subject_id != read.subject_id
        {
            return Err("方向、账号或重复记录无效".into());
        }
        if !links
            .iter()
            .any(|l| l.local_id == i.local_id && l.cloud_id == i.cloud_id)
        {
            return Err("对应关系已变化".into());
        }
        upload::check_pending(&tx, &scope, &i.local_id, &i.cloud_id)?;
        let t = get_task(&tx, &i.local_id)?.ok_or("本地记录不存在")?;
        let cloud = read
            .records
            .iter()
            .find(|r| r.record_id == i.cloud_id)
            .ok_or("云端记录不可见")?;
        if t.trashed
            || t.revision != i.expected_local_revision
            || t.content_modified_at.is_none()
            || t.content_modified_at != i.expected_local_modified_at
            || cloud.modified_at.is_none()
            || cloud.modified_at != i.expected_cloud_modified_at
            || cloud.shared_fields != i.expected_cloud_fields
        {
            return Err("记录版本已变化，请重新预览".into());
        }
        let parsed: Content =
            serde_json::from_value(cloud.shared_fields.clone()).map_err(|_| "字段未核实完整")?;
        if serde_json::to_value(parsed).map_err(error)? != cloud.shared_fields {
            return Err("字段未核实完整".into());
        }
    }
    let id: String = tx
        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
        .map_err(error)?;
    let mut entries = Vec::new();
    for choice in input.entries {
        let local = get_task(&tx, &choice.input.local_id)?.ok_or("本地记录不存在")?;
        entries.push(Entry {
            choice,
            state: "queued".into(),
            frozen_local: Some(Content::from(&local)),
            operation_id: None,
        });
    }
    let job = Job {
        id,
        scope,
        stopped: false,
        invalidated_by_restore: false,
        entries,
    };
    tx.execute(
        "INSERT INTO local_sync_batches VALUES(?1,?2,?3)",
        params![
            job.id,
            serde_json::to_string(&job.scope).map_err(error)?,
            serde_json::to_string(&job).map_err(error)?
        ],
    )
    .map_err(error)?;
    tx.commit().map_err(error)?;
    Ok(summary(&job))
}
pub fn claim(c: &mut Connection, id: &str, subject: &str) -> Result<Option<BatchChoice>, String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let mut j = load(&tx, id)?;
    require_active(&j)?;
    if j.scope.subject_id != subject {
        return Err("批次账号变化".into());
    }
    if summary(&j).state == "completed" {
        return Ok(None);
    }
    if summary(&j).state != "ready" {
        return Err("批次已停止或结果待核实，不能继续发出写入".into());
    }
    let e = j
        .entries
        .iter_mut()
        .find(|e| e.state == "queued")
        .ok_or("没有未执行条目")?;
    e.state = "running".into();
    let choice = e.choice.clone();
    save(&tx, &j)?;
    tx.commit().map_err(error)?;
    Ok(Some(choice))
}
pub fn finish(c: &mut Connection, id: &str, outcome: &str) -> Result<BatchSummary, String> {
    if !["completed", "blocked", "uncertain"].contains(&outcome) {
        return Err("批次结果无效".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let mut j = load(&tx, id)?;
    require_active(&j)?;
    let e = j
        .entries
        .iter_mut()
        .find(|e| e.state == "running")
        .ok_or("没有正在执行的条目")?;
    e.state = outcome.into();
    if outcome != "completed" {
        j.stopped = true;
    }
    save(&tx, &j)?;
    tx.commit().map_err(error)?;
    Ok(summary(&j))
}
// Associate the durable single-write journal before any network mutation.
pub fn attach_operation(c: &mut Connection, id: &str, operation: &str) -> Result<(), String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let mut j = load(&tx, id)?;
    require_active(&j)?;
    let e = j
        .entries
        .iter_mut()
        .find(|e| e.state == "running")
        .ok_or("没有正在执行的条目")?;
    if e.choice.direction != "local" || e.operation_id.is_some() {
        return Err("条目不能重复关联写入日志".into());
    }
    upload::verify_batch_operation(
        &tx,
        operation,
        &j.scope,
        &e.choice.input,
        e.frozen_local.as_ref().ok_or("缺少冻结内容")?,
    )?;
    e.operation_id = Some(operation.into());
    save(&tx, &j)?;
    tx.commit().map_err(error)
}
fn verify_frozen_local(c: &Connection, scope: &Scope, e: &Entry) -> Result<(), String> {
    let i = &e.choice.input;
    let local = get_task(c, &i.local_id)?.ok_or("本地记录不可见")?;
    if local.trashed
        || local.revision != i.expected_local_revision
        || local.content_modified_at != i.expected_local_modified_at
        || Some(Content::from(&local)) != e.frozen_local
        || !existing_links(c, scope)?
            .iter()
            .any(|l| l.local_id == i.local_id && l.cloud_id == i.cloud_id)
    {
        return Err("本地版本或对应关系已变化，批次仍锁定，请人工核对".into());
    }
    Ok(())
}
// Only reads the remote inspection. Never sends or retries an update.
pub fn reconcile(
    c: &mut Connection,
    id: &str,
    read: &crate::feishu::SyncInspection,
) -> Result<BatchSummary, String> {
    verify_scope(c, id, read)?;
    let mut j = load(c, id)?;
    if j.entries.iter().any(|e| e.state == "running") {
        return Err("条目仍标记为执行中，不能假定已退出；先核对运行实例，禁止重发".into());
    }
    let index = j
        .entries
        .iter()
        .position(|e| e.state == "uncertain")
        .ok_or("没有可核实的不明条目")?;
    let e = &j.entries[index];
    if e.choice.direction != "local" {
        return Err("该条目暂不支持核实恢复，不能重发".into());
    }
    let operation = e
        .operation_id
        .as_deref()
        .ok_or("旧条目没有对应写入日志，不能自动放行")?;
    verify_frozen_local(c, &j.scope, e)?;
    let logged = upload::verify_batch_operation(
        c,
        operation,
        &j.scope,
        &e.choice.input,
        e.frozen_local.as_ref().ok_or("缺少冻结内容")?,
    )?;
    let state = if logged == "notApplied" {
        "blocked"
    } else {
        let cloud = read
            .records
            .iter()
            .find(|r| r.record_id == e.choice.input.cloud_id)
            .ok_or("飞书记录不可见，结果仍待核实")?;
        let desired = e.frozen_local.as_ref().ok_or("缺少冻结内容")?;
        let status = match desired.progress {
            0 => "未开始",
            100 => "已完成",
            _ => "进行中",
        };
        if cloud.modified_at.is_none()
            || cloud.shared_fields != serde_json::to_value(desired).map_err(error)?
            || cloud.status != status
        {
            return Err("飞书未与冻结提交内容及状态一致，结果仍待核实，不重发".into());
        }
        if upload::reconcile(c, operation, Some(read))?.state != "completed" {
            return Err("写入日志尚未核实完成，不能放行".into());
        }
        "completed"
    };
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let live = load(&tx, id)?;
    if serde_json::to_string(&live).map_err(error)? != serde_json::to_string(&j).map_err(error)? {
        return Err("核实期间批次变化，请重新检查".into());
    }
    verify_frozen_local(&tx, &j.scope, e)?;
    j.entries[index].state = state.into();
    j.stopped = true;
    save(&tx, &j)?;
    tx.commit().map_err(error)?;
    Ok(summary(&j))
}
pub fn control(c: &mut Connection, id: &str, action: &str) -> Result<BatchSummary, String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let mut j = load(&tx, id)?;
    require_active(&j)?;
    match action {
        "stop" => j.stopped = true,
        "start" => {
            if !["ready","stopped"].contains(&summary(&j).state.as_str()) {
                return Err("仅可人工启动未执行或已停止且无未核实条目的批次".into());
            }
            j.stopped = false;
        }
        "resume" => {
            if summary(&j).state != "stopped" {
                return Err("仅可继续已停止且无未核实条目的批次".into());
            }
            j.stopped = false;
        }
        _ => return Err("批次控制无效".into()),
    }
    save(&tx, &j)?;
    tx.commit().map_err(error)?;
    Ok(summary(&j))
}
pub fn verify_scope(
    c: &Connection,
    id: &str,
    read: &crate::feishu::SyncInspection,
) -> Result<(), String> {
    require_active(&load(c, id)?)?;
    if !read.read_only || !read.complete || load(c, id)?.scope != scope_from_inspection(read) {
        return Err("批次账号、表格或检查结果变化".into());
    }
    Ok(())
}
pub fn verify_target(c: &Connection, id: &str, space: &str, table: &str) -> Result<(), String> {
    let j = load(c, id)?;
    require_active(&j)?;
    if j.scope.platform != Platform::Feishu
        || j.scope.space_id != space
        || j.scope.table_id != table
    {
        return Err("批次目标不属于当前隔离表".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prepared_fixture(
        path: &std::path::Path,
        directions: [&str; 2],
    ) -> (LocalWorkspace, crate::feishu::SyncInspection, String) {
        let mut db = LocalWorkspace::open(path).unwrap();
        let mut tasks = Vec::new();
        for n in 0..2 {
            let mut input = super::super::super::tests::input(None, 42);
            input.title = format!("fixture-{n}");
            let s = db.save(input).unwrap();
            tasks.push(
                s.tasks
                    .into_iter()
                    .find(|t| t.title == format!("fixture-{n}"))
                    .unwrap(),
            );
        }
        let mut read = crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "test".into(),
            subject_id: "cli:open".into(),
            app_id: "cli".into(),
            app_token: "base".into(),
            table_id: "table".into(),
            checked_at: content_now().unwrap(),
            records: vec![],
            conditional_write_verified: false,
            warnings: vec![],
        };
        let mut choices = Vec::new();
        for (n, local) in tasks.iter().enumerate() {
            let mut fields = Content::from(local);
            fields.progress = 60;
            let cloud = crate::feishu::SyncRecordTime {
                shared_fields: serde_json::to_value(fields).unwrap(),
                record_id: format!("cloud-{n}"),
                title: local.title.clone(),
                progress: 60,
                status: "进行中".into(),
                planned_date: local.planned_date.clone(),
                modified_at: Some(read.checked_at - 1000),
                time_source: "platformSystem".into(),
            };
            save_pairing(
                &mut db.connection,
                local,
                &cloud,
                &read,
                &SavePairingInput {
                    local_id: local.id.clone(),
                    cloud_id: cloud.record_id.clone(),
                    expected_local_revision: local.revision,
                    expected_local_modified_at: local.content_modified_at,
                    expected_cloud_modified_at: cloud.modified_at,
                },
            )
            .unwrap();
            choices.push(BatchChoice {
                direction: directions[n].into(),
                input: DownloadInput {
                    local_id: local.id.clone(),
                    cloud_id: cloud.record_id.clone(),
                    subject_id: read.subject_id.clone(),
                    expected_local_revision: local.revision,
                    expected_local_modified_at: local.content_modified_at,
                    expected_cloud_modified_at: cloud.modified_at,
                    expected_cloud_fields: cloud.shared_fields.clone(),
                },
            });
            read.records.push(cloud);
        }
        let job = prepare(&mut db.connection, &read, BatchInput { entries: choices }).unwrap();
        (db, read, job.id)
    }
    fn fixture() -> (
        LocalWorkspace,
        crate::feishu::SyncInspection,
        String,
        upload::WorkOperation,
    ) {
        let (mut db, read, id) =
            prepared_fixture(std::path::Path::new(":memory:"), ["local", "cloud"]);
        let first = claim(&mut db.connection, &id, &read.subject_id)
            .unwrap()
            .unwrap();
        let op = upload::begin(&mut db.connection, &read, &first.input).unwrap();
        attach_operation(&mut db.connection, &id, &op.id).unwrap();
        (db, read, id, op)
    }
    #[test]
    fn two_downloads_stop_reopen_manual_resume_and_no_replay() {
        let path = std::env::temp_dir().join(format!(
            "local-batch-two-downloads-{}-{}.sqlite3",
            std::process::id(),
            content_now().unwrap()
        ));
        let (mut db, read, id) = prepared_fixture(&path, ["cloud", "cloud"]);
        let first = claim(&mut db.connection, &id, &read.subject_id)
            .unwrap()
            .unwrap();
        download(&mut db.connection, &read, &first.input).unwrap();
        // The current write is settled; stop prevents the next claim, not its readback.
        control(&mut db.connection, &id, "stop").unwrap();
        let paused = finish(&mut db.connection, &id, "completed").unwrap();
        assert_eq!(
            (paused.state.as_str(), paused.completed, paused.queued),
            ("stopped", 1, 1)
        );
        let tasks = db.snapshot().unwrap().tasks;
        assert_eq!(tasks.iter().filter(|t| t.progress == 60).count(), 1);
        assert_eq!(tasks.iter().filter(|t| t.progress == 42).count(), 1);
        drop(db);

        let mut db = LocalWorkspace::open(&path).unwrap();
        assert_eq!(db.snapshot().unwrap().tasks, tasks);
        for _ in 0..3 {
            let paused = summary(&load(&db.connection, &id).unwrap());
            assert_eq!(
                (paused.state.as_str(), paused.completed, paused.queued),
                ("stopped", 1, 1)
            );
            assert!(claim(&mut db.connection, &id, &read.subject_id).is_err());
        }
        // Explicit human continuation is represented by the same production control command.
        control(&mut db.connection, &id, "resume").unwrap();
        let next = claim(&mut db.connection, &id, &read.subject_id)
            .unwrap()
            .unwrap();
        assert_ne!(next.input.local_id, first.input.local_id);
        download(&mut db.connection, &read, &next.input).unwrap();
        assert_eq!(
            finish(&mut db.connection, &id, "completed")
                .unwrap()
                .completed,
            2
        );
        let tasks = db.snapshot().unwrap().tasks;
        assert!(tasks.iter().all(|t| t.progress == 60));
        let history: i64 = db
            .connection
            .query_row("SELECT COUNT(*) FROM local_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(history, 4); // Two initial saves and exactly two downloads.
        drop(db);

        let mut db = LocalWorkspace::open(&path).unwrap();
        assert_eq!(
            summary(&load(&db.connection, &id).unwrap()).state,
            "completed"
        );
        assert_eq!(db.snapshot().unwrap().tasks, tasks);
        assert!(claim(&mut db.connection, &id, &read.subject_id)
            .unwrap()
            .is_none());
        assert!(control(&mut db.connection, &id, "resume").is_err());
        let after: i64 = db
            .connection
            .query_row("SELECT COUNT(*) FROM local_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(after, history);
        drop(db);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn paused_download_rechecks_remaining_local_and_cloud_versions() {
        for change_local in [false, true] {
            let (mut db, mut read, id) =
                prepared_fixture(std::path::Path::new(":memory:"), ["cloud", "cloud"]);
            let first = claim(&mut db.connection, &id, &read.subject_id)
                .unwrap()
                .unwrap();
            download(&mut db.connection, &read, &first.input).unwrap();
            control(&mut db.connection, &id, "stop").unwrap();
            finish(&mut db.connection, &id, "completed").unwrap();
            let remaining = load(&db.connection, &id).unwrap().entries[1]
                .choice
                .input
                .local_id
                .clone();
            if change_local {
                let task = get_task(&db.connection, &remaining).unwrap().unwrap();
                db.save(super::super::super::tests::input(Some(&task), 25))
                    .unwrap();
            } else {
                read.records[1].modified_at = read.records[1].modified_at.map(|t| t + 1000);
            }
            let before = db.snapshot().unwrap().tasks;
            control(&mut db.connection, &id, "resume").unwrap();
            let next = claim(&mut db.connection, &id, &read.subject_id)
                .unwrap()
                .unwrap();
            assert!(download(&mut db.connection, &read, &next.input).is_err());
            let blocked = finish(&mut db.connection, &id, "blocked").unwrap();
            assert_eq!((blocked.state.as_str(), blocked.completed), ("blocked", 1));
            assert_eq!(db.snapshot().unwrap().tasks, before);
            assert!(claim(&mut db.connection, &id, &read.subject_id).is_err());
            assert!(control(&mut db.connection, &id, "resume").is_err());
        }
    }
    #[test]
    fn uncertain_upload_reconciles_without_resend_and_keeps_remaining_stopped() {
        let (mut db, mut read, id, op) = fixture();
        assert!(reconcile(&mut db.connection, &id, &read).is_err()); // still running
        finish(&mut db.connection, &id, "uncertain").unwrap();
        assert!(reconcile(&mut db.connection, &id, &read).is_err()); // old read proves nothing
        read.records[0].shared_fields = op.content.clone();
        read.records[0].status = op.status.clone();
        read.records[0].modified_at = Some(read.checked_at);
        let before = db.snapshot().unwrap().tasks;
        let checked = reconcile(&mut db.connection, &id, &read).unwrap();
        assert_eq!(checked.state, "stopped");
        assert_eq!(checked.completed, 1);
        assert_eq!(checked.queued, 1);
        assert_eq!(db.snapshot().unwrap().tasks, before);
        assert_eq!(
            db.connection
                .query_row("SELECT COUNT(*) FROM local_sync_operations", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(claim(&mut db.connection, &id, &read.subject_id).is_err());
        assert!(reconcile(&mut db.connection, &id, &read).is_err());
        control(&mut db.connection, &id, "resume").unwrap();
        let next = claim(&mut db.connection, &id, &read.subject_id)
            .unwrap()
            .unwrap();
        assert_eq!(next.direction, "cloud");
        download(&mut db.connection, &read, &next.input).unwrap();
        assert_eq!(
            finish(&mut db.connection, &id, "completed").unwrap().state,
            "completed"
        );
        assert!(claim(&mut db.connection, &id, &read.subject_id)
            .unwrap()
            .is_none());
    }
    #[test]
    fn missing_log_changed_local_wrong_account_or_remote_status_never_unlocks() {
        for case in 0..6 {
            let (mut db, mut read, id, op) = fixture();
            finish(&mut db.connection, &id, "uncertain").unwrap();
            read.records[0].shared_fields = op.content;
            read.records[0].modified_at = Some(read.checked_at);
            match case {
                0 => read.subject_id = "other".into(),
                1 => read.records[0].status = "已完成".into(),
                2 => read.records[0].modified_at = None,
                3 => {
                    let mut j = load(&db.connection, &id).unwrap();
                    j.entries[0].operation_id = None;
                    save(&db.connection, &j).unwrap();
                }
                4 => {
                    let task = db
                        .snapshot()
                        .unwrap()
                        .tasks
                        .into_iter()
                        .find(|t| t.title == "fixture-0")
                        .unwrap();
                    db.save(super::super::super::tests::input(Some(&task), 71))
                        .unwrap();
                }
                _ => read.complete = false,
            }
            assert!(reconcile(&mut db.connection, &id, &read).is_err());
            assert_eq!(
                summary(&load(&db.connection, &id).unwrap()).state,
                "uncertain"
            );
            assert!(control(&mut db.connection, &id, "resume").is_err());
        }
    }
    #[test]
    fn sync_boundary_scope_switches_never_reuse_old_batch_or_pending_evidence() {
        fn contents(c: &Connection) -> Vec<(String, Vec<Vec<String>>)> {
            ["local_tasks", "local_history", "local_meta", "local_sync_connections",
                "local_sync_links", "local_sync_operations", "local_sync_batches"].iter().map(|table| {
                let mut q = c.prepare(&format!("SELECT * FROM {table} ORDER BY rowid")).unwrap();
                let columns = q.column_count();
                let rows = q.query_map([], |r| Ok((0..columns).map(|i| format!("{:?}",r.get_ref(i).unwrap())).collect()))
                    .unwrap().collect::<Result<Vec<_>,_>>().unwrap();
                ((*table).to_string(), rows)
            }).collect()
        }
        for change in ["subject", "base", "table", "partial", "not-readonly"] {
            let (mut db, mut read, id, op) = fixture();
            finish(&mut db.connection, &id, "uncertain").unwrap();
            let input = load(&db.connection, &id).unwrap().entries[0].choice.input.clone();
            // A matching desired cloud value must not unlock an operation from another scope.
            read.records[0].shared_fields = op.content.clone();
            read.records[0].status = op.status.clone();
            read.records[0].modified_at = Some(read.checked_at);
            match change {
                "subject" => read.subject_id = "cli:other-user".into(),
                "base" => read.app_token = "other-base".into(),
                "table" => read.table_id = "other-table".into(),
                "partial" => read.complete = false,
                _ => read.read_only = false,
            }
            let before = contents(&db.connection);
            assert!(verify_scope(&db.connection, &id, &read).is_err(), "{change}");
            assert!(reconcile(&mut db.connection, &id, &read).is_err(), "{change}");
            assert_eq!(upload::reconcile(&mut db.connection, &op.id, Some(&read)).unwrap().state, "pending", "{change}");
            assert!(upload::begin(&mut db.connection, &read, &input).is_err(), "{change}");
            assert!(download(&mut db.connection, &read, &input).is_err(), "{change}");
            assert!(prepare(&mut db.connection, &read, BatchInput {entries:vec![BatchChoice {direction:"local".into(),input}]}).is_err(), "{change}");
            assert!(control(&mut db.connection, &id, "resume").is_err());
            if ["subject","base","table"].contains(&change) {
                let other = scope_from_inspection(&read);
                assert!(list(&db.connection,&other).unwrap().is_empty());
                assert!(upload::list(&db.connection,&other).unwrap().is_empty());
                assert!(existing_links(&db.connection,&other).unwrap().is_empty());
            }
            assert!(verify_target(&db.connection,&id,"other-base",&read.table_id).is_err());
            assert!(verify_target(&db.connection,&id,&read.app_token,"other-table").is_err());
            assert_eq!(contents(&db.connection),before,"{change}: no task, history, baseline, intent or queue mutation");
        }
    }
    #[test]
    fn definitely_unsent_log_blocks_old_batch_instead_of_replaying() {
        let (mut db, read, id, op) = fixture();
        upload::cancel_unsent(&db.connection, &op.id).unwrap();
        finish(&mut db.connection, &id, "uncertain").unwrap();
        assert_eq!(
            reconcile(&mut db.connection, &id, &read).unwrap().state,
            "blocked"
        );
        assert!(claim(&mut db.connection, &id, &read.subject_id).is_err());
    }
    fn seeded(c: &Connection) -> String {
        let choice = BatchChoice {
            direction: "local".into(),
            input: DownloadInput {
                local_id: "l".into(),
                cloud_id: "c".into(),
                subject_id: "s".into(),
                expected_local_revision: 1,
                expected_local_modified_at: Some(1),
                expected_cloud_modified_at: Some(1),
                expected_cloud_fields: serde_json::json!({}),
            },
        };
        let j = Job {
            id: "test".into(),
            scope: Scope {
                platform: Platform::Feishu,
                subject_id: "s".into(),
                space_id: "b".into(),
                table_id: "t".into(),
            },
            entries: vec![
                Entry {
                    choice: choice.clone(),
                    state: "queued".into(),
                    frozen_local: None,
                    operation_id: None,
                },
                Entry {
                    choice,
                    state: "queued".into(),
                    frozen_local: None,
                    operation_id: None,
                },
            ],
            stopped: false,
            invalidated_by_restore: false,
        };
        c.execute(
            "INSERT INTO local_sync_batches VALUES('test','scope',?1)",
            [serde_json::to_string(&j).unwrap()],
        )
        .unwrap();
        j.id
    }
    #[test]
    fn stop_preserves_queue_and_completion_never_replays() {
        let mut c = Connection::open_in_memory().unwrap();
        initialize(&c).unwrap();
        let id = seeded(&c);
        assert!(claim(&mut c, &id, "wrong").is_err());
        claim(&mut c, &id, "s").unwrap();
        assert!(claim(&mut c, &id, "s").is_err());
        control(&mut c, &id, "stop").unwrap();
        assert_eq!(finish(&mut c, &id, "completed").unwrap().state, "stopped");
        assert!(claim(&mut c, &id, "s").is_err());
        control(&mut c, &id, "resume").unwrap();
        claim(&mut c, &id, "s").unwrap();
        assert_eq!(finish(&mut c, &id, "completed").unwrap().completed, 2);
        assert!(claim(&mut c, &id, "s").unwrap().is_none());
    }
    #[test]
    fn interrupted_and_uncertain_intents_never_resume() {
        let mut c = Connection::open_in_memory().unwrap();
        initialize(&c).unwrap();
        let id = seeded(&c);
        claim(&mut c, &id, "s").unwrap();
        let persisted = load(&c, &id).unwrap();
        assert_eq!(summary(&persisted).state, "uncertain");
        assert!(control(&mut c, &id, "resume").is_err());
        finish(&mut c, &id, "uncertain").unwrap();
        assert!(claim(&mut c, &id, "s").is_err());
        assert_eq!(summary(&load(&c, &id).unwrap()).queued, 1);
    }
    #[test]
    fn durable_running_checkpoint_survives_database_reopen() {
        let path = std::env::temp_dir().join(format!(
            "local-batch-checkpoint-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let mut c = Connection::open(&path).unwrap();
            initialize(&c).unwrap();
            let id = seeded(&c);
            claim(&mut c, &id, "s").unwrap();
        }
        {
            let mut c = Connection::open(&path).unwrap();
            let job = summary(&load(&c, "test").unwrap());
            assert_eq!(job.state, "uncertain");
            assert_eq!(job.queued, 1);
            assert!(control(&mut c, "test", "resume").is_err());
            assert!(claim(&mut c, "test", "s").is_err());
            assert!(verify_target(&c, "test", "wrong", "t").is_err());
            assert!(verify_target(&c, "test", "b", "t").is_ok());
        }
        std::fs::remove_file(path).unwrap();
    }
}
