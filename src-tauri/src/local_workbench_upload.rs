//! Durable intent and readback reconciliation; no network or credentials here.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    id: String,
    scope: Scope,
    before_cloud: Content,
    before_status: String,
    before_cloud_modified_at: Option<i64>,
    desired: LocalTask,
    desired_status: String,
    restore_of: Option<String>,
    verified_cloud_modified_at: Option<i64>,
    created_at: i64,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadSummary {
    pub id: String,
    pub state: String,
    pub local_id: String,
    pub cloud_id: String,
    pub can_restore: bool,
    pub restore_content: serde_json::Value,
    pub restore_status: String,
    pub is_restore: bool,
}
#[derive(Debug)]
pub struct WorkOperation {
    pub id: String,
    pub record_id: String,
    pub content: serde_json::Value,
    pub status: String,
}

pub fn initialize(c: &Connection) -> Result<(), String> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS local_sync_operations(id TEXT PRIMARY KEY,scope TEXT NOT NULL,local_id TEXT NOT NULL,cloud_id TEXT NOT NULL,data TEXT NOT NULL,state TEXT NOT NULL);").map_err(error)
}
fn load(c: &Connection, id: &str) -> Result<(Operation, String, String), String> {
    let (data, state, cloud): (String, String, String) = c
        .query_row(
            "SELECT data,state,cloud_id FROM local_sync_operations WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .map_err(error)?;
    Ok((serde_json::from_str(&data).map_err(error)?, state, cloud))
}
pub(crate) fn verify_scope(c:&Connection,id:&str,inspection:&crate::feishu::SyncInspection)->Result<(),String> {
    let (op,_,_)=load(c,id)?;
    if !inspection.complete||!inspection.read_only||op.scope!=scope_from_inspection(inspection) {return Err("操作记录与当前用户或目标不一致".into());}
    Ok(())
}
pub(crate) fn verify_restore_preview(c:&Connection,id:&str,content:&serde_json::Value,status:&str)->Result<(),String> {
    let (op,_,_)=load(c,id)?;
    if serde_json::to_value(&op.before_cloud).map_err(error)?!=*content||op.before_status!=status {
        return Err("恢复内容与已预览的操作记录不一致，请重新检查".into());
    }
    Ok(())
}
pub(crate) fn verify_unsent(c:&Connection,id:&str)->Result<(),String> {
    let (op,state,cloud)=load(c,id)?;
    let task=get_task(c,&op.desired.id)?.ok_or("本地任务不存在")?;
    if state!="pending"||task.trashed||task.revision!=op.desired.revision||task.content_modified_at!=op.desired.content_modified_at
        ||!existing_links(c,&op.scope)?.iter().any(|l|l.local_id==op.desired.id&&l.cloud_id==cloud) {
        return Err("提交前本地版本或关联已变化，请重新检查".into());
    }
    Ok(())
}
pub(super) fn verify_batch_operation(
    c: &Connection,
    id: &str,
    scope: &Scope,
    input: &DownloadInput,
    desired: &Content,
) -> Result<String, String> {
    let (op, state, cloud) = load(c, id)?;
    if &op.scope != scope
        || cloud != input.cloud_id
        || op.desired.id != input.local_id
        || op.restore_of.is_some()
        || op.desired.revision != input.expected_local_revision
        || op.desired.content_modified_at != input.expected_local_modified_at
        || Content::from(&op.desired) != *desired
        || op.before_cloud_modified_at != input.expected_cloud_modified_at
        || serde_json::to_value(&op.before_cloud).map_err(error)? != input.expected_cloud_fields
    {
        return Err("写入日志与冻结的批次条目不一致，禁止放行".into());
    }
    Ok(state)
}
fn summary(op: &Operation, state: String, cloud: String) -> Result<UploadSummary, String> {
    Ok(UploadSummary {
        id: op.id.clone(),
        can_restore: state == "completed" && op.restore_of.is_none(),
        state,
        local_id: op.desired.id.clone(),
        cloud_id: cloud,
        restore_content: serde_json::to_value(&op.before_cloud).map_err(error)?,
        restore_status: op.before_status.clone(),
        is_restore: op.restore_of.is_some(),
    })
}
pub fn list(c: &Connection, scope: &Scope) -> Result<Vec<UploadSummary>, String> {
    let key = serde_json::to_string(scope).map_err(error)?;
    let mut stmt = c
        .prepare("SELECT id FROM local_sync_operations WHERE scope=?1 AND (state IN ('pending','conflict') OR id IN (SELECT id FROM local_sync_operations WHERE scope=?1 AND state NOT IN ('pending','conflict') ORDER BY rowid DESC LIMIT 10)) ORDER BY CASE WHEN state IN ('pending','conflict') THEN 0 ELSE 1 END,rowid DESC")
        .map_err(error)?;
    let ids = stmt
        .query_map([key], |r| r.get::<_, String>(0))
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    ids.into_iter()
        .map(|id| {
            let (o, s, cid) = load(c, &id)?;
            summary(&o, s, cid)
        })
        .collect()
}
pub fn check_pending(
    c: &Connection,
    scope: &Scope,
    local_id: &str,
    cloud_id: &str,
) -> Result<(), String> {
    let pending:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM local_sync_operations WHERE scope=?1 AND (local_id=?2 OR cloud_id=?3) AND state IN ('pending','conflict'))",params![serde_json::to_string(scope).map_err(error)?,local_id,cloud_id],|r|r.get(0)).map_err(error)?;
    if pending {
        Err("此前写入结果待核实，请先只读核实，不能重发或覆盖".into())
    } else {
        Ok(())
    }
}
// Only callable before invoking the cloud mutation, never on a send/response error.
pub fn cancel_unsent(c: &Connection, id: &str) -> Result<(), String> {
    c.execute(
        "UPDATE local_sync_operations SET state='notApplied' WHERE id=?1 AND state='pending'",
        [id],
    )
    .map_err(error)?;
    Ok(())
}
fn insert(c: &mut Connection, op: Operation, cloud_id: &str) -> Result<WorkOperation, String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    check_pending(&tx, &op.scope, &op.desired.id, cloud_id)?;
    let live = get_task(&tx, &op.desired.id)?.ok_or("本地任务不存在")?;
    // For restoration the task content intentionally differs, but its current version is still checked.
    if live.trashed
        || live.revision != op.desired.revision
        || live.content_modified_at != op.desired.content_modified_at
    {
        return Err("本地版本已变化，请重新检查".into());
    }
    if !existing_links(&tx, &op.scope)?
        .iter()
        .any(|l| l.local_id == op.desired.id && l.cloud_id == cloud_id)
    {
        return Err("对应关系已变化".into());
    }
    tx.execute("INSERT INTO local_sync_operations(id,scope,local_id,cloud_id,data,state) VALUES(?1,?2,?3,?4,?5,'pending')",params![op.id,serde_json::to_string(&op.scope).map_err(error)?,op.desired.id,cloud_id,serde_json::to_string(&op).map_err(error)?]).map_err(error)?;
    tx.commit().map_err(error)?;
    Ok(WorkOperation {
        id: op.id,
        record_id: cloud_id.into(),
        content: serde_json::to_value(Content::from(&op.desired)).map_err(error)?,
        status: op.desired_status,
    })
}
pub fn begin(
    c: &mut Connection,
    inspection: &crate::feishu::SyncInspection,
    input: &DownloadInput,
) -> Result<WorkOperation, String> {
    if !inspection.read_only || !inspection.complete || inspection.subject_id != input.subject_id {
        return Err("授权身份或读取结果变化".into());
    }
    let cloud = inspection
        .records
        .iter()
        .find(|r| r.record_id == input.cloud_id)
        .ok_or("飞书记录不可见")?;
    if cloud.modified_at.is_none()
        || cloud.modified_at != input.expected_cloud_modified_at
        || cloud.shared_fields != input.expected_cloud_fields
    {
        return Err("飞书版本已变化，请重新检查".into());
    }
    let before_cloud: Content =
        serde_json::from_value(cloud.shared_fields.clone()).map_err(|_| "共有字段未核实完整")?;
    if serde_json::to_value(&before_cloud).map_err(error)? != cloud.shared_fields {
        return Err("共有字段未核实完整，不能把缺失日期当作空值".into());
    }
    let desired = get_task(c, &input.local_id)?.ok_or("本地任务不存在")?;
    if desired.revision != input.expected_local_revision
        || desired.content_modified_at != input.expected_local_modified_at
    {
        return Err("本地版本已变化，请重新检查".into());
    }
    let id: String = c
        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
        .map_err(error)?;
    let status = match desired.progress {
        0 => "未开始",
        100 => "已完成",
        _ => "进行中",
    }
    .into();
    insert(
        c,
        Operation {
            id,
            scope: scope_from_inspection(inspection),
            before_cloud,
            before_status: cloud.status.clone(),
            before_cloud_modified_at: cloud.modified_at,
            desired,
            desired_status: status,
            restore_of: None,
            verified_cloud_modified_at: None,
            created_at: content_now()?,
        },
        &cloud.record_id,
    )
}
pub fn begin_restore(
    c: &mut Connection,
    inspection: &crate::feishu::SyncInspection,
    id: &str,
) -> Result<WorkOperation, String> {
    let (original, state, cloud_id) = load(c, id)?;
    if state != "completed"
        || original.restore_of.is_some()
        || original.scope != scope_from_inspection(inspection)
        || !inspection.complete
        || !inspection.read_only
    {
        return Err("该操作不能恢复或授权目标已变化".into());
    }
    let cloud = inspection
        .records
        .iter()
        .find(|r| r.record_id == cloud_id)
        .ok_or("飞书记录不可见")?;
    if cloud.modified_at != original.verified_cloud_modified_at
        || cloud.shared_fields
            != serde_json::to_value(Content::from(&original.desired)).map_err(error)?
        || cloud.status != original.desired_status
    {
        return Err("上传后飞书记录又发生变化，不能覆盖恢复；请重新对照".into());
    }
    let mut desired = get_task(c, &original.desired.id)?.ok_or("本地任务不存在")?;
    let before = original.before_cloud;
    desired.title = before.title;
    desired.detail = before.detail;
    desired.progress = before.progress;
    desired.quadrant = before.quadrant;
    desired.today = before.today;
    desired.start_date = before.start_date;
    desired.planned_date = before.planned_date;
    let next: String = c
        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
        .map_err(error)?;
    insert(
        c,
        Operation {
            id: next,
            scope: original.scope,
            before_cloud: Content::from(&original.desired),
            before_status: cloud.status.clone(),
            before_cloud_modified_at: cloud.modified_at,
            desired,
            desired_status: original.before_status,
            restore_of: Some(id.into()),
            verified_cloud_modified_at: None,
            created_at: content_now()?,
        },
        &cloud_id,
    )
}
pub fn reconcile(
    c: &mut Connection,
    id: &str,
    inspection: Option<&crate::feishu::SyncInspection>,
) -> Result<UploadSummary, String> {
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let (mut op, state, cloud_id) = load(&tx, id)?;
    if state != "pending" && state != "conflict" {
        return summary(&op, state, cloud_id);
    }
    let Some(read) =
        inspection.filter(|r| r.complete && r.read_only && scope_from_inspection(r) == op.scope)
    else {
        return summary(&op, state, cloud_id);
    };
    let Some(cloud) = read.records.iter().find(|r| r.record_id == cloud_id) else {
        return summary(&op, state, cloud_id);
    };
    let content = Content::from(&op.desired);
    let next = if cloud.modified_at.is_some()
        && cloud.shared_fields == serde_json::to_value(&content).map_err(error)?
        && cloud.status == op.desired_status
    {
        op.verified_cloud_modified_at = cloud.modified_at;
        if let Some(original) = &op.restore_of {
            tx.execute("UPDATE local_sync_operations SET state='restored' WHERE id=?1 AND state='completed'",[original]).map_err(error)?;
        } else {
            let baseline = Baseline {
                content,
                local_evidence: Evidence {
                    modified_at: op.desired.content_modified_at,
                    source: TimeSource::LocalClock,
                    version: Some(op.desired.revision.to_string()),
                },
                cloud_evidence: Evidence {
                    modified_at: cloud.modified_at,
                    source: TimeSource::PlatformSystem,
                    version: cloud.modified_at.map(|v| v.to_string()),
                },
            };
            let link = Link {
                local_id: op.desired.id.clone(),
                cloud_id: cloud_id.clone(),
                baseline: Some(baseline),
                last_successful_sync_at: Some(content_now()?),
            };
            let connection = register(&tx, &op.scope)?;
            let count=tx.execute("UPDATE local_sync_links SET data=?1 WHERE connection_id=?2 AND local_id=?3 AND cloud_id=?4",params![serde_json::to_string(&link).map_err(error)?,connection,link.local_id,link.cloud_id]).map_err(error)?;
            if count != 1 {
                return Err("关联已变化，结果仍待核实".into());
            }
        }
        "completed"
    } else if cloud.shared_fields == serde_json::to_value(&op.before_cloud).map_err(error)?
        && cloud.modified_at == op.before_cloud_modified_at
        && cloud.status == op.before_status
    {
        // A read of the old version cannot prove an interrupted request will never apply.
        // Keep the journal locked; future checks are read-only, never automatic retries.
        "pending"
    } else {
        "conflict"
    };
    tx.execute(
        "UPDATE local_sync_operations SET data=?1,state=?2 WHERE id=?3",
        params![serde_json::to_string(&op).map_err(error)?, next, id],
    )
    .map_err(error)?;
    tx.commit().map_err(error)?;
    summary(&op, next.into(), cloud_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (LocalWorkspace, crate::feishu::SyncInspection, DownloadInput) {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        let local = db
            .save(super::super::super::tests::input(None, 42))
            .unwrap()
            .tasks
            .remove(0);
        let mut content = Content::from(&local);
        content.progress = 60;
        let read = crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "test".into(),
            subject_id: "cli:open".into(),
            app_id: "cli".into(),
            app_token: "base".into(),
            table_id: "table".into(),
            checked_at: 1791684437000,
            records: vec![crate::feishu::SyncRecordTime {
                shared_fields: serde_json::to_value(content).unwrap(),
                record_id: "cloud".into(),
                title: local.title.clone(),
                progress: 60,
                status: "进行中".into(),
                planned_date: local.planned_date.clone(),
                modified_at: Some(1791684436000),
                time_source: "platformSystem".into(),
            }],
            conditional_write_verified: false,
            warnings: vec![],
        };
        save_pairing(
            &mut db.connection,
            &local,
            &read.records[0],
            &read,
            &SavePairingInput {
                local_id: local.id.clone(),
                cloud_id: "cloud".into(),
                expected_local_revision: local.revision,
                expected_local_modified_at: local.content_modified_at,
                expected_cloud_modified_at: read.records[0].modified_at,
            },
        )
        .unwrap();
        let input = DownloadInput {
            local_id: local.id,
            cloud_id: "cloud".into(),
            subject_id: read.subject_id.clone(),
            expected_local_revision: local.revision,
            expected_local_modified_at: local.content_modified_at,
            expected_cloud_modified_at: read.records[0].modified_at,
            expected_cloud_fields: read.records[0].shared_fields.clone(),
        };
        (db, read, input)
    }
    fn applied(read: &mut crate::feishu::SyncInspection, op: &WorkOperation) {
        read.records[0].shared_fields = op.content.clone();
        read.records[0].status = op.status.clone();
        read.records[0].modified_at = Some(read.records[0].modified_at.unwrap() + 1000);
    }
    #[test]
    fn pending_record_guard_keeps_unrelated_connections_and_records_independent() {
        let (mut db, read, input) = fixture();
        begin(&mut db.connection, &read, &input).unwrap();
        let scope = scope_from_inspection(&read);
        assert!(check_pending(&db.connection, &scope, "restored-new-id", &input.cloud_id).is_err());
        assert!(check_pending(&db.connection, &scope, &input.local_id, "other-cloud").is_err());
        assert!(check_pending(&db.connection, &scope, "other-local", "other-cloud").is_ok());
        for field in ["subject", "space", "table", "platform"] {
            let mut other = scope.clone();
            match field {
                "subject" => other.subject_id = "another-user".into(),
                "space" => other.space_id = "another-space".into(),
                "table" => other.table_id = "another-table".into(),
                _ => other.platform = Platform::Wps,
            }
            assert!(
                check_pending(&db.connection, &other, &input.local_id, &input.cloud_id).is_ok()
            );
        }
    }
    #[test]
    fn pending_survives_read_failure_and_old_read_never_proves_safe_retry() {
        let (mut db, read, input) = fixture();
        let op = begin(&mut db.connection, &read, &input).unwrap();
        assert_eq!(
            reconcile(&mut db.connection, &op.id, None).unwrap().state,
            "pending"
        );
        assert_eq!(
            reconcile(&mut db.connection, &op.id, Some(&read))
                .unwrap()
                .state,
            "pending"
        );
        assert_eq!(
            list(&db.connection, &scope_from_inspection(&read)).unwrap()[0].state,
            "pending"
        );
        assert!(begin(&mut db.connection, &read, &input).is_err());
        assert!(download(&mut db.connection, &read, &input).is_err());
        assert!(
            !existing_links(&db.connection, &scope_from_inspection(&read)).unwrap()[0]
                .baseline_established
        );
    }
    #[test]
    fn stale_identity_version_and_conflicting_read_do_not_advance_baseline() {
        let (mut db, mut read, input) = fixture();
        read.subject_id = "other".into();
        assert!(begin(&mut db.connection, &read, &input).is_err());
        read.subject_id = input.subject_id.clone();
        read.records[0].modified_at = Some(1);
        assert!(begin(&mut db.connection, &read, &input).is_err());
        read.records[0].modified_at = input.expected_cloud_modified_at;
        let op = begin(&mut db.connection, &read, &input).unwrap();
        read.records[0].shared_fields["progress"] = serde_json::json!(71);
        assert_eq!(
            reconcile(&mut db.connection, &op.id, Some(&read))
                .unwrap()
                .state,
            "conflict"
        );
        assert!(check_pending(
            &db.connection,
            &scope_from_inspection(&read),
            &input.local_id,
            &input.cloud_id
        )
        .is_err());
        assert!(download(&mut db.connection, &read, &input).is_err());
        assert!(
            !existing_links(&db.connection, &scope_from_inspection(&read)).unwrap()[0]
                .baseline_established
        );
        applied(&mut read, &op);
        assert_eq!(
            reconcile(&mut db.connection, &op.id, Some(&read))
                .unwrap()
                .state,
            "completed"
        );
    }
    #[test]
    fn successful_readback_preserves_new_local_edits_and_restore_guards_then_preserves_baseline() {
        let (mut db, mut read, input) = fixture();
        let original = get_task(&db.connection, &input.local_id).unwrap().unwrap();
        let op = begin(&mut db.connection, &read, &input).unwrap();
        let mut later = super::super::super::tests::input(Some(&original), 71);
        later.title = original.title.clone();
        db.save(later).unwrap();
        applied(&mut read, &op);
        let done = reconcile(&mut db.connection, &op.id, Some(&read)).unwrap();
        assert!(done.can_restore);
        assert_eq!(
            get_task(&db.connection, &input.local_id)
                .unwrap()
                .unwrap()
                .progress,
            71
        );
        let before_link: String = db
            .connection
            .query_row("SELECT data FROM local_sync_links", [], |r| r.get(0))
            .unwrap();
        let link: Link = serde_json::from_str(&before_link).unwrap();
        assert_eq!(link.baseline.unwrap().content.progress, 42);
        read.records[0].modified_at = Some(read.records[0].modified_at.unwrap() + 1);
        assert!(begin_restore(&mut db.connection, &read, &op.id).is_err());
        read.records[0].modified_at = Some(read.records[0].modified_at.unwrap() - 1);
        let restore = begin_restore(&mut db.connection, &read, &op.id).unwrap();
        assert_eq!(restore.content["progress"], 60);
        applied(&mut read, &restore);
        assert_eq!(
            reconcile(&mut db.connection, &restore.id, Some(&read))
                .unwrap()
                .state,
            "completed"
        );
        assert_eq!(load(&db.connection, &op.id).unwrap().1, "restored");
        let after_link: String = db
            .connection
            .query_row("SELECT data FROM local_sync_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before_link, after_link);
        assert_eq!(
            get_task(&db.connection, &input.local_id)
                .unwrap()
                .unwrap()
                .progress,
            71
        );
        assert!(begin_restore(&mut db.connection, &read, &op.id).is_err());
    }
    #[test]
    fn failed_baseline_commit_retains_pending_for_safe_reconciliation() {
        let (mut db, mut read, input) = fixture();
        let op = begin(&mut db.connection, &read, &input).unwrap();
        applied(&mut read, &op);
        db.connection.execute_batch("CREATE TRIGGER fail_baseline BEFORE UPDATE ON local_sync_links BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(reconcile(&mut db.connection, &op.id, Some(&read)).is_err());
        assert_eq!(load(&db.connection, &op.id).unwrap().1, "pending");
        db.connection
            .execute_batch("DROP TRIGGER fail_baseline")
            .unwrap();
        assert_eq!(
            reconcile(&mut db.connection, &op.id, Some(&read))
                .unwrap()
                .state,
            "completed"
        );
    }
    #[test]
    fn only_known_unsent_preparation_failure_releases_pending_guard() {
        let (mut db, read, input) = fixture();
        let op = begin(&mut db.connection, &read, &input).unwrap();
        cancel_unsent(&db.connection, &op.id).unwrap();
        assert_eq!(load(&db.connection, &op.id).unwrap().1, "notApplied");
        assert!(
            !existing_links(&db.connection, &scope_from_inspection(&read)).unwrap()[0]
                .baseline_established
        );
        assert!(begin(&mut db.connection, &read, &input).is_ok());
    }
    #[test]
    fn journal_survives_database_reopen() {
        let (mut db, read, input) = fixture();
        let op = begin(&mut db.connection, &read, &input).unwrap();
        let path =
            std::env::temp_dir().join(format!("project-track-journal-test-{}.sqlite3", op.id));
        db.connection
            .execute("VACUUM INTO ?1", [path.to_str().unwrap()])
            .unwrap();
        drop(db);
        let reopened = LocalWorkspace::open(&path).unwrap();
        assert_eq!(
            list(&reopened.connection, &scope_from_inspection(&read)).unwrap()[0].state,
            "pending"
        );
        assert!(check_pending(
            &reopened.connection,
            &scope_from_inspection(&read),
            &input.local_id,
            &input.cloud_id
        )
        .is_err());
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn absent_optional_date_is_unknown_not_a_verified_empty_value() {
        let (mut db, mut read, mut input) = fixture();
        read.records[0]
            .shared_fields
            .as_object_mut()
            .unwrap()
            .remove("startDate");
        input.expected_cloud_fields = read.records[0].shared_fields.clone();
        assert!(begin(&mut db.connection, &read, &input).is_err());
        assert!(download(&mut db.connection, &read, &input).is_err());
        assert!(list(&db.connection, &scope_from_inspection(&read))
            .unwrap()
            .is_empty());
    }
}
