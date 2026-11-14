//! Read-only comparison seam. Deliberately has no cloud client or write command.
#[cfg(all(test,windows,feature="feishu-direct-account-acceptance"))]
#[path="direct_account_sync_acceptance.rs"]
mod direct_acceptance;
use super::*;
#[path = "local_workbench_batch.rs"]
mod batch;
pub(super) use batch::invalidate_for_restore as invalidate_batches_for_restore;
pub(super) use batch::verify_scope as verify_batch_scope;
pub(super) use batch::verify_target as verify_batch_target;
pub(super) use batch::{
    attach_operation as attach_batch_operation, claim as claim_batch, control as control_batch,
    finish as finish_batch, prepare as prepare_batch, reconcile as reconcile_batch,
};
pub use batch::{BatchInput, BatchSummary};
#[cfg(test)]
pub(super) use batch::BatchChoice;
#[path = "local_workbench_upload.rs"]
mod upload;
pub use upload::UploadSummary;
pub(crate) use upload::WorkOperation;
pub(super) use upload::{
    verify_scope as verify_upload_scope, verify_unsent as verify_unsent_upload, verify_restore_preview,
    begin as begin_upload, begin_restore as begin_upload_restore,
    cancel_unsent as cancel_unsent_upload, reconcile as reconcile_upload,
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    platform: Platform,
    subject_id: String,
    space_id: String,
    table_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Platform {
    Feishu,
    Wps,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Content {
    title: String,
    detail: String,
    progress: u8,
    quadrant: String,
    today: bool,
    start_date: Option<String>,
    planned_date: Option<String>,
}
impl From<&LocalTask> for Content {
    fn from(t: &LocalTask) -> Self {
        Self {
            title: t.title.clone(),
            detail: t.detail.clone(),
            progress: t.progress,
            quadrant: t.quadrant.clone(),
            today: t.today,
            start_date: t.start_date.clone(),
            planned_date: t.planned_date.clone(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
enum TimeSource {
    LocalClock,
    PlatformSystem,
    ApplicationMetadata,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Evidence {
    modified_at: Option<i64>,
    source: TimeSource,
    version: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Baseline {
    content: Content,
    local_evidence: Evidence,
    cloud_evidence: Evidence,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Record {
    id: String,
    content: Content,
    evidence: Evidence,
    trashed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Link {
    local_id: String,
    cloud_id: String,
    baseline: Option<Baseline>,
    last_successful_sync_at: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CloudRead {
    scope: Scope,
    complete: bool,
    records: Vec<Record>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Difference {
    field: String,
    local: serde_json::Value,
    cloud: serde_json::Value,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    local_id: String,
    cloud_id: Option<String>,
    title: String,
    status: String,
    message: String,
    recommendation: Option<String>,
    local_evidence: Evidence,
    cloud_evidence: Option<Evidence>,
    last_successful_sync_at: Option<i64>,
    differences: Vec<Difference>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview {
    scope: Scope,
    checked_at: i64,
    read_only: bool,
    items: Vec<Item>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncDemo {
    pub simulated: bool,
    pub label: String,
    preview: Preview,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingLocalCandidate {
    id: String,
    title: String,
    progress: u8,
    planned_date: Option<String>,
    revision: i64,
    modified_at: Option<i64>,
    time_source: TimeSource,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PairingSavedLink {
    #[serde(skip)]
    baseline: Option<Baseline>,
    local_id: String,
    cloud_id: String,
    baseline_established: bool,
    last_successful_sync_at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingPreview {
    batch_jobs: Vec<BatchSummary>,
    upload_operations: Vec<UploadSummary>,
    comparisons: Vec<LinkedComparison>,
    kind: &'static str,
    read_only: bool,
    requires_explicit_selection: bool,
    account_name: String,
    subject_id: String,
    app_id: String,
    app_token: String,
    table_id: String,
    checked_at: i64,
    local_candidates: Vec<PairingLocalCandidate>,
    cloud_candidates: Vec<crate::feishu::SyncRecordTime>,
    existing_links: Vec<PairingSavedLink>,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedComparison {
    status: String,
    message: String,
    recommendation: Option<String>,
    baseline_established: bool,
    last_successful_sync_at: Option<i64>,
    local_revision: i64,
    expected_cloud_fields: serde_json::Value,
    subject_id: String,
    local_id: String,
    cloud_id: String,
    local_title: String,
    cloud_title: String,
    local_modified_at: Option<i64>,
    cloud_modified_at: Option<i64>,
    fields: Vec<ComparedField>,
    baseline_candidate_ready: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ComparedField {
    field: String,
    local: serde_json::Value,
    cloud: serde_json::Value,
    available: bool,
    equal: bool,
}

fn linked_comparisons(
    local: &[LocalTask],
    inspection: &crate::feishu::SyncInspection,
    links: &[PairingSavedLink],
) -> Result<Vec<LinkedComparison>, String> {
    let mut result = Vec::new();
    for link in links {
        let Some(task) = local.iter().find(|t| t.id == link.local_id && !t.trashed) else {
            continue;
        };
        let Some(cloud) = inspection
            .records
            .iter()
            .find(|r| r.record_id == link.cloud_id)
        else {
            continue;
        };
        let content = serde_json::to_value(Content::from(task)).map_err(error)?;
        let fields: Vec<_> = [
            "title",
            "detail",
            "progress",
            "quadrant",
            "today",
            "startDate",
            "plannedDate",
        ]
        .into_iter()
        .map(|key| {
            let available = cloud.shared_fields.get(key).is_some();
            let value = cloud
                .shared_fields
                .get(key)
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            ComparedField {
                field: key.into(),
                local: content[key].clone(),
                equal: available && content[key] == value,
                cloud: value,
                available,
            }
        })
        .collect();
        let local_record = Record {
            id: task.id.clone(),
            content: Content::from(task),
            trashed: false,
            evidence: Evidence {
                modified_at: task.content_modified_at,
                source: TimeSource::LocalClock,
                version: Some(task.revision.to_string()),
            },
        };
        // Never interpret a missing shared field as a verified empty value.
        let classification = if fields.iter().all(|f| f.available) {
            let cloud_content: Content =
                serde_json::from_value(cloud.shared_fields.clone()).map_err(error)?;
            let cloud_record = Record {
                id: cloud.record_id.clone(),
                content: cloud_content,
                trashed: false,
                evidence: Evidence {
                    modified_at: cloud.modified_at,
                    source: if cloud.time_source == "platformSystem" {
                        TimeSource::PlatformSystem
                    } else {
                        TimeSource::Unknown
                    },
                    version: None,
                },
            };
            let full_link = Link {
                local_id: link.local_id.clone(),
                cloud_id: link.cloud_id.clone(),
                baseline: link.baseline.clone(),
                last_successful_sync_at: link.last_successful_sync_at,
            };
            Some(compare(
                &local_record,
                Some(&cloud_record),
                Some(&full_link),
                inspection.checked_at,
            )?)
        } else {
            None
        };
        result.push(LinkedComparison {
            status: classification
                .as_ref()
                .map(|item| item.status.clone())
                .unwrap_or_else(|| "unverified".into()),
            message: classification
                .as_ref()
                .map(|item| item.message.clone())
                .unwrap_or_else(|| "共有字段尚未核实完整，不能判断哪一侧修改；禁止覆盖。".into()),
            recommendation: classification.and_then(|item| item.recommendation),
            baseline_established: link.baseline_established,
            last_successful_sync_at: link.last_successful_sync_at,
            local_revision: task.revision,
            expected_cloud_fields: cloud.shared_fields.clone(),
            subject_id: inspection.subject_id.clone(),
            local_id: task.id.clone(),
            cloud_id: cloud.record_id.clone(),
            local_title: task.title.clone(),
            cloud_title: cloud.title.clone(),
            local_modified_at: task.content_modified_at,
            cloud_modified_at: cloud.modified_at,
            baseline_candidate_ready: fields.iter().all(|f| f.available && f.equal),
            fields,
        });
    }
    Ok(result)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DownloadInput {
    pub local_id: String,
    pub cloud_id: String,
    pub subject_id: String,
    pub expected_local_revision: i64,
    pub expected_local_modified_at: Option<i64>,
    pub expected_cloud_modified_at: Option<i64>,
    pub expected_cloud_fields: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportInput {
    pub cloud_id: String,
    pub subject_id: String,
    pub expected_cloud_modified_at: Option<i64>,
    pub expected_cloud_fields: serde_json::Value,
    pub confirm_import: bool,
}

fn validated_cloud_content(cloud: &crate::feishu::SyncRecordTime) -> Result<Content, String> {
    let content: Content = serde_json::from_value(cloud.shared_fields.clone())
        .map_err(|_| "共有字段不完整或类型不支持，不能同步")?;
    if serde_json::to_value(&content).map_err(error)? != cloud.shared_fields {
        return Err("共有字段未核实完整，不能把缺失日期当作空值".into());
    }
    if content.title.trim().is_empty()
        || content.title.chars().count() > 200
        || content.detail.chars().count() > 10000
        || !["iu", "in", "un", "nn"].contains(&content.quadrant.as_str())
        || content.progress > 100
    {
        return Err("飞书任务内容不符合本地字段要求，不能同步".into());
    }
    let start_date = normalize_date(content.start_date.clone())?;
    let planned_date = normalize_date(content.planned_date.clone())?;
    if start_date != content.start_date || planned_date != content.planned_date {
        return Err("飞书日期字段格式不符合本地要求，不能同步".into());
    }
    if start_date
        .as_ref()
        .zip(planned_date.as_ref())
        .is_some_and(|(s, d)| s > d)
    {
        return Err("计划完成日期早于开始日期，不能同步".into());
    }
    Ok(content)
}

pub(super) fn import_cloud_task(
    c: &mut Connection,
    inspection: &crate::feishu::SyncInspection,
    input: &ImportInput,
) -> Result<(), String> {
    import_cloud_tasks(c, inspection, std::slice::from_ref(input))
}

pub(super) fn import_cloud_tasks(
    c: &mut Connection,
    inspection: &crate::feishu::SyncInspection,
    inputs: &[ImportInput],
) -> Result<(), String> {
    if inputs.is_empty() || inputs.len() > 100 {
        return Err("每次请选择 1–100 条飞书任务".into());
    }
    if !inspection.read_only || !inspection.complete {
        return Err("授权账号或读取结果已变化，请重新检查".into());
    }
    let mut seen=std::collections::HashSet::new();
    for input in inputs {
        if !input.confirm_import || input.subject_id != inspection.subject_id {
            return Err("所选任务的确认或飞书账号已变化，请重新检查".into());
        }
        if !seen.insert(&input.cloud_id) {
            return Err("所选飞书任务重复，请重新选择".into());
        }
        let cloud = inspection.records.iter().find(|r| r.record_id == input.cloud_id)
            .ok_or("飞书记录不存在或当前不可见")?;
        if cloud.modified_at.is_none()
            || cloud.modified_at != input.expected_cloud_modified_at
            || cloud.shared_fields != input.expected_cloud_fields
        {
            return Err("飞书内容或版本已变化，请重新检查后确认".into());
        }
        validated_cloud_content(cloud)?;
    }
    let scope = scope_from_inspection(inspection);
    let tx = c.transaction_with_behavior(TransactionBehavior::Immediate).map_err(error)?;
    let connection_id = register(&tx, &scope)?;
    for input in inputs {
        let cloud = inspection.records.iter().find(|r| r.record_id == input.cloud_id)
            .ok_or("飞书记录不存在或当前不可见")?;
        let content = validated_cloud_content(cloud)?;
        let already_linked: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_sync_links WHERE connection_id=?1 AND cloud_id=?2)",
            params![connection_id, input.cloud_id], |row| row.get(0),
        ).map_err(error)?;
        if already_linked {
            return Err("所选任务中有飞书记录已关联本地；整批未导入，请重新检查".into());
        }
        upload::check_pending(&tx, &scope, "", &input.cloud_id)?;
        let id: String = tx.query_row("SELECT lower(hex(randomblob(16)))", [], |row| row.get(0)).map_err(error)?;
        let task = LocalTask {
            id: id.clone(), title: content.title.clone(), detail: content.detail.clone(),
            progress: content.progress, quadrant: content.quadrant.clone(), today: content.today,
            start_date: content.start_date.clone(), planned_date: content.planned_date.clone(),
            trashed: false, revision: next_revision(&tx)?, content_modified_at: Some(content_now()?),
        };
        put(&tx, &task)?;
        log(&tx, &None, &task, "从飞书导入任务（可撤销）")?;
        let verified = get_task(&tx, &id)?.ok_or("导入后本地读回失败")?;
        if Content::from(&verified) != content {
            return Err("本地读回内容不一致，已回滚".into());
        }
        let baseline = Baseline {
            content,
            local_evidence: Evidence {modified_at: task.content_modified_at, source: TimeSource::LocalClock, version: Some(task.revision.to_string())},
            cloud_evidence: Evidence {modified_at: cloud.modified_at, source: TimeSource::PlatformSystem, version: cloud.modified_at.map(|v| v.to_string())},
        };
        let link = Link {local_id:id, cloud_id: input.cloud_id.clone(), baseline:Some(baseline), last_successful_sync_at:Some(content_now()?)};
        tx.execute("INSERT INTO local_sync_links(connection_id,local_id,cloud_id,data) VALUES(?1,?2,?3,?4)",
            params![connection_id,link.local_id,link.cloud_id,serde_json::to_string(&link).map_err(error)?]).map_err(error)?;
    }
    tx.commit().map_err(error)
}

pub(super) fn download(
    c: &mut Connection,
    inspection: &crate::feishu::SyncInspection,
    input: &DownloadInput,
) -> Result<(), String> {
    if !inspection.read_only || !inspection.complete || inspection.subject_id != input.subject_id {
        return Err("授权账号或读取结果已变化，请重新检查".into());
    }
    let cloud = inspection
        .records
        .iter()
        .find(|r| r.record_id == input.cloud_id)
        .ok_or("飞书记录不存在或当前不可见")?;
    if cloud.modified_at.is_none()
        || cloud.modified_at != input.expected_cloud_modified_at
        || cloud.shared_fields != input.expected_cloud_fields
    {
        return Err("飞书内容或版本已变化，请重新检查后确认".into());
    }
    let content = validated_cloud_content(cloud)?;
    let start_date = content.start_date.clone();
    let planned_date = content.planned_date.clone();
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let scope = scope_from_inspection(inspection);
    upload::check_pending(&tx, &scope, &input.local_id, &input.cloud_id)?;
    let links = existing_links(&tx, &scope)?;
    if !links
        .iter()
        .any(|l| l.local_id == input.local_id && l.cloud_id == input.cloud_id)
    {
        return Err("对应关系已变化，请重新检查".into());
    }
    let before = get_task(&tx, &input.local_id)?.ok_or("本地任务不存在")?;
    if before.trashed
        || before.revision != input.expected_local_revision
        || before.content_modified_at != input.expected_local_modified_at
    {
        return Err("本地任务已变化，请重新检查后确认".into());
    }
    let mut after = before.clone();
    let changed = Content::from(&before) != content;
    if changed {
        after.title = content.title.clone();
        after.detail = content.detail.clone();
        after.progress = content.progress;
        after.quadrant = content.quadrant.clone();
        after.today = content.today;
        after.start_date = start_date;
        after.planned_date = planned_date;
        after.revision = next_revision(&tx)?;
        after.content_modified_at = Some(content_now()?);
        put(&tx, &after)?;
        log(&tx, &Some(before), &after, "飞书同步到本地（可撤销）")?;
    }
    let verified = get_task(&tx, &after.id)?.ok_or("同步后本地读回失败")?;
    if Content::from(&verified) != content {
        return Err("本地读回内容不一致，已回滚".into());
    }
    let baseline = Baseline {
        content,
        local_evidence: Evidence {
            modified_at: verified.content_modified_at,
            source: TimeSource::LocalClock,
            version: Some(verified.revision.to_string()),
        },
        cloud_evidence: Evidence {
            modified_at: cloud.modified_at,
            source: TimeSource::PlatformSystem,
            version: cloud.modified_at.map(|v| v.to_string()),
        },
    };
    let link = Link {
        local_id: input.local_id.clone(),
        cloud_id: input.cloud_id.clone(),
        baseline: Some(baseline),
        last_successful_sync_at: Some(content_now()?),
    };
    let connection_id = register(&tx, &scope)?;
    let updated = tx.execute("UPDATE local_sync_links SET data=?1 WHERE connection_id=?2 AND local_id=?3 AND cloud_id=?4", params![serde_json::to_string(&link).map_err(error)?, connection_id, input.local_id, input.cloud_id]).map_err(error)?;
    if updated != 1 {
        return Err("对应关系保存失败，已回滚".into());
    }
    tx.commit().map_err(error)
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SavePairingInput {
    pub local_id: String,
    pub cloud_id: String,
    pub expected_local_revision: i64,
    pub expected_local_modified_at: Option<i64>,
    pub expected_cloud_modified_at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavePairingResult {
    kind: &'static str,
    local_id: String,
    cloud_id: String,
    already_saved: bool,
    baseline_established: bool,
    successful_sync_recorded: bool,
    local_task_changed: bool,
    cloud_record_changed: bool,
}

fn scope_from_inspection(inspection: &crate::feishu::SyncInspection) -> Scope {
    Scope {
        platform: Platform::Feishu,
        subject_id: inspection.subject_id.clone(),
        space_id: inspection.app_token.clone(),
        table_id: inspection.table_id.clone(),
    }
}

fn existing_links(c: &Connection, scope: &Scope) -> Result<Vec<PairingSavedLink>, String> {
    let platform = match scope.platform {
        Platform::Feishu => "feishu",
        Platform::Wps => "wps",
    };
    let connection_id = c
        .query_row(
            "SELECT id FROM local_sync_connections WHERE platform=?1 AND subject_id=?2 AND space_id=?3 AND table_id=?4",
            params![platform, scope.subject_id, scope.space_id, scope.table_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(error)?;
    let Some(connection_id) = connection_id else {
        return Ok(Vec::new());
    };
    let mut statement = c
        .prepare(
            "SELECT local_id,cloud_id,data FROM local_sync_links WHERE connection_id=?1 ORDER BY local_id,cloud_id",
        )
        .map_err(error)?;
    let links = statement
        .query_map([connection_id], |row| {
            let data: String = row.get(2)?;
            let link: Link = serde_json::from_str(&data).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(PairingSavedLink {
                baseline: link.baseline.clone(),
                local_id: row.get(0)?,
                cloud_id: row.get(1)?,
                baseline_established: link.baseline.is_some(),
                last_successful_sync_at: link.last_successful_sync_at,
            })
        })
        .map_err(error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(error)?;
    Ok(links)
}

pub(super) fn pairing_preview(
    c: &Connection,
    local: &[LocalTask],
    inspection: crate::feishu::SyncInspection,
) -> Result<PairingPreview, String> {
    if !inspection.read_only || !inspection.complete {
        return Err("飞书读取不完整，不能预览首次关联".into());
    }
    let mut local_ids = std::collections::HashSet::new();
    if local.iter().any(|task| !local_ids.insert(&task.id)) {
        return Err("本地任务编号重复，不能预览首次关联".into());
    }
    let local_candidates = local
        .iter()
        .filter(|task| !task.trashed)
        .map(|task| PairingLocalCandidate {
            id: task.id.clone(),
            title: task.title.clone(),
            progress: task.progress,
            planned_date: task.planned_date.clone(),
            revision: task.revision,
            modified_at: task.content_modified_at,
            time_source: if task.content_modified_at.is_some() {
                TimeSource::LocalClock
            } else {
                TimeSource::Unknown
            },
        })
        .collect();
    let saved_links = existing_links(c, &scope_from_inspection(&inspection))?;
    let upload_operations = upload::list(c, &scope_from_inspection(&inspection))?;
    let comparisons = linked_comparisons(local, &inspection, &saved_links)?;
    let batch_jobs = batch::list(c, &scope_from_inspection(&inspection))?;
    let mut warnings = inspection.warnings;
    warnings.push("同名任务也不会自动关联；只有人工选择后才保存对应关系。".into());
    warnings.push("保存对应关系不会建立同步基线，也不会修改任一侧任务内容。".into());
    warnings.push(
        "已关联任务展示全部 7 个共有字段；字段存在且记录未返回值时按空值核实，字段缺失或类型不支持时待核实。同步方向须人工选择。".into(),
    );
    Ok(PairingPreview {
        batch_jobs,
        upload_operations,
        comparisons,
        kind: "pairingPreview",
        read_only: true,
        requires_explicit_selection: true,
        account_name: inspection.account_name,
        subject_id: inspection.subject_id,
        app_id: inspection.app_id,
        app_token: inspection.app_token,
        table_id: inspection.table_id,
        checked_at: inspection.checked_at,
        local_candidates,
        cloud_candidates: inspection.records,
        existing_links: saved_links,
        warnings,
    })
}

pub(super) fn save_pairing(
    c: &mut Connection,
    local: &LocalTask,
    cloud: &crate::feishu::SyncRecordTime,
    inspection: &crate::feishu::SyncInspection,
    input: &SavePairingInput,
) -> Result<SavePairingResult, String> {
    if !inspection.read_only || !inspection.complete {
        return Err("飞书读取不完整，不能保存对应关系".into());
    }
    if local.trashed || local.id != input.local_id || cloud.record_id != input.cloud_id {
        return Err("所选任务不存在或已变化，请重新检查后选择".into());
    }
    if local.revision != input.expected_local_revision
        || local.content_modified_at != input.expected_local_modified_at
    {
        return Err("本地任务已发生变化，请重新检查后选择".into());
    }
    if cloud.modified_at != input.expected_cloud_modified_at {
        return Err("飞书任务已发生变化，请重新检查后选择".into());
    }
    let scope = scope_from_inspection(inspection);
    if !valid_scope(&scope) {
        return Err("同步连接身份或目标表格不完整".into());
    }
    let tx = c
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(error)?;
    let connection_id = register(&tx, &scope)?;
    let existing = tx
        .query_row(
            "SELECT local_id,cloud_id FROM local_sync_links WHERE connection_id=?1 AND (local_id=?2 OR cloud_id=?3)",
            params![connection_id, input.local_id, input.cloud_id],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(error)?;
    let already_saved = match existing {
        Some((local_id, cloud_id)) if local_id == input.local_id && cloud_id == input.cloud_id => {
            true
        }
        Some(_) => return Err("所选本地任务或飞书记录已经关联到其他任务".into()),
        None => {
            bind(
                &tx,
                &connection_id,
                &Link {
                    local_id: input.local_id.clone(),
                    cloud_id: input.cloud_id.clone(),
                    baseline: None,
                    last_successful_sync_at: None,
                },
            )?;
            false
        }
    };
    tx.commit().map_err(error)?;
    Ok(SavePairingResult {
        kind: "pairingSaved",
        local_id: input.local_id.clone(),
        cloud_id: input.cloud_id.clone(),
        already_saved,
        baseline_established: false,
        successful_sync_recorded: false,
        local_task_changed: false,
        cloud_record_changed: false,
    })
}

pub(super) fn initialize(c: &Connection) -> Result<(), String> {
    c.execute_batch("PRAGMA foreign_keys=ON;
      CREATE TABLE IF NOT EXISTS local_sync_connections(
        id TEXT PRIMARY KEY,platform TEXT NOT NULL,subject_id TEXT NOT NULL,space_id TEXT NOT NULL,table_id TEXT NOT NULL,
        UNIQUE(platform,subject_id,space_id,table_id));
      CREATE TABLE IF NOT EXISTS local_sync_links(
        connection_id TEXT NOT NULL REFERENCES local_sync_connections(id) ON DELETE CASCADE,
        local_id TEXT NOT NULL REFERENCES local_tasks(id) ON DELETE CASCADE,cloud_id TEXT NOT NULL,
        data TEXT NOT NULL,PRIMARY KEY(connection_id,local_id),UNIQUE(connection_id,cloud_id));").map_err(error)?;
    upload::initialize(c)?;
    batch::initialize(c)
}
fn valid_scope(scope: &Scope) -> bool {
    [&scope.subject_id, &scope.space_id, &scope.table_id]
        .into_iter()
        .all(|v| !v.trim().is_empty() && v.len() <= 256 && !v.chars().any(char::is_control))
}
// Internal persistence for future explicit account/table binding, not an IPC command.
fn register(c: &Connection, scope: &Scope) -> Result<String, String> {
    if !valid_scope(scope) {
        return Err("同步连接身份或目标表格不完整".into());
    }
    let platform = match scope.platform {
        Platform::Feishu => "feishu",
        Platform::Wps => "wps",
    };
    c.execute("INSERT OR IGNORE INTO local_sync_connections(id,platform,subject_id,space_id,table_id) VALUES(lower(hex(randomblob(16))),?1,?2,?3,?4)",params![platform,scope.subject_id,scope.space_id,scope.table_id]).map_err(error)?;
    c.query_row("SELECT id FROM local_sync_connections WHERE platform=?1 AND subject_id=?2 AND space_id=?3 AND table_id=?4",params![platform,scope.subject_id,scope.space_id,scope.table_id],|r|r.get(0)).map_err(error)
}
fn bind(c: &Connection, connection_id: &str, link: &Link) -> Result<(), String> {
    if link.cloud_id.trim().is_empty()
        || link.cloud_id.len() > 256
        || link.baseline.is_some()
        || link.last_successful_sync_at.is_some()
    {
        return Err("首次关联不能冒充已有同步基线或同步成功".into());
    }
    c.execute(
        "INSERT INTO local_sync_links(connection_id,local_id,cloud_id,data) VALUES(?1,?2,?3,?4)",
        params![
            connection_id,
            link.local_id,
            link.cloud_id,
            serde_json::to_string(link).map_err(error)?
        ],
    )
    .map_err(error)?;
    Ok(())
}
fn trusted(e: &Evidence, local: bool, now: i64) -> bool {
    e.source
        == (if local {
            TimeSource::LocalClock
        } else {
            TimeSource::PlatformSystem
        })
        && e.modified_at
            .is_some_and(|v| v > 0 && v <= now.saturating_add(300_000) && v <= 253402300799999)
}
fn differences(local: &Content, cloud: &Content) -> Result<Vec<Difference>, String> {
    let local = serde_json::to_value(local).map_err(error)?;
    let cloud = serde_json::to_value(cloud).map_err(error)?;
    Ok(local
        .as_object()
        .ok_or("无效任务内容")?
        .iter()
        .filter(|(k, v)| cloud[*k] != **v)
        .map(|(k, v)| Difference {
            field: k.clone(),
            local: v.clone(),
            cloud: cloud[k].clone(),
        })
        .collect())
}
fn compare(
    local: &Record,
    cloud: Option<&Record>,
    link: Option<&Link>,
    now: i64,
) -> Result<Item, String> {
    let mut item = Item {
        local_id: local.id.clone(),
        cloud_id: link.map(|l| l.cloud_id.clone()),
        title: local.content.title.clone(),
        status: "manual".into(),
        message: String::new(),
        recommendation: None,
        local_evidence: local.evidence.clone(),
        cloud_evidence: cloud.map(|r| r.evidence.clone()),
        last_successful_sync_at: link.and_then(|l| l.last_successful_sync_at),
        differences: Vec::new(),
    };
    if link.is_none() {
        item.status = "unlinked".into();
        item.message = "尚未确认对应记录；不会按同名覆盖，请先确认关联或新增目标。".into();
        return Ok(item);
    }
    let Some(cloud) = cloud else {
        item.status = "unverified".into();
        item.message = "关联记录未读到；请核实权限、分页与目标，不推断已删除。".into();
        return Ok(item);
    };
    item.differences = differences(&local.content, &cloud.content)?;
    if local.trashed || cloud.trashed {
        item.status = "deletionConflict".into();
        item.message = "涉及回收站或删除；本阶段不执行云端删除或恢复。".into();
        return Ok(item);
    }
    if local.content == cloud.content {
        item.status = "equal".into();
        item.message = "共有内容一致，无需覆盖；预览不推进同步时间或基线。".into();
        return Ok(item);
    }
    let Some(base) = link.and_then(|l| l.baseline.as_ref()) else {
        item.message = "没有已核实同步基线，请人工选择；时间较晚不代表包含另一侧修改。".into();
        return Ok(item);
    };
    let l = local.content != base.content;
    let r = cloud.content != base.content;
    if l && r {
        item.status = "conflict".into();
        item.message = "两边都有修改；即使时间较晚，也不默认选择整体覆盖。".into();
        return Ok(item);
    }
    if !trusted(&local.evidence, true, now)
        || !trusted(&cloud.evidence, false, now)
        || !trusted(&base.local_evidence, true, now)
        || !trusted(&base.cloud_evidence, false, now)
    {
        item.message = "缺少可靠内容修改时间、来源未核实或时间异常，请人工选择。".into();
        return Ok(item);
    }
    let lt = local.evidence.modified_at.unwrap();
    let rt = cloud.evidence.modified_at.unwrap();
    let (changed, old) = if l {
        (&local.evidence, &base.local_evidence)
    } else {
        (&cloud.evidence, &base.cloud_evidence)
    };
    if lt == rt
        || (l && lt < rt)
        || (!l && rt < lt)
        || old
            .modified_at
            .is_none_or(|t| changed.modified_at.is_none_or(|v| v <= t))
    {
        item.message = "时间相同、倒退或与单边变化证据矛盾，请人工选择。".into();
        return Ok(item);
    }
    item.status = if l { "localChanged" } else { "cloudChanged" }.into();
    item.recommendation = Some(if l { "localToCloud" } else { "cloudToLocal" }.into());
    item.message = if l {
        "仅本地有更新；后续写入阶段可询问是否更新云端。本页只预览。"
    } else {
        "仅云端有更新；后续写入阶段可询问是否更新本地。本页只预览。"
    }
    .into();
    Ok(item)
}
fn preview(
    scope: &Scope,
    local: &[Record],
    links: &[Link],
    cloud: &CloudRead,
    now: i64,
) -> Result<Preview, String> {
    if !valid_scope(scope) || *scope != cloud.scope {
        return Err("账号、平台或表格身份不一致，拒绝比较".into());
    }
    if !cloud.complete {
        return Err("云端读取未完成或分页不全，不能据此判断任务差异".into());
    }
    let mut cloud_ids = std::collections::HashSet::new();
    if cloud.records.iter().any(|r| !cloud_ids.insert(&r.id)) {
        return Err("云端记录编号重复，拒绝自动关联".into());
    }
    let mut local_ids = std::collections::HashSet::new();
    let mut linked_ids = std::collections::HashSet::new();
    let mut linked_cloud = std::collections::HashSet::new();
    if local.iter().any(|r| !local_ids.insert(&r.id))
        || links.iter().any(|l| {
            !linked_ids.insert(&l.local_id)
                || !linked_cloud.insert(&l.cloud_id)
                || !local_ids.contains(&l.local_id)
        })
    {
        return Err("任务对应关系重复或失效，请重新确认关联".into());
    }
    let mut items: Vec<Item> = local
        .iter()
        .map(|l| {
            let link = links.iter().find(|m| m.local_id == l.id);
            let c = link.and_then(|m| cloud.records.iter().find(|r| r.id == m.cloud_id));
            compare(l, c, link, now)
        })
        .collect::<Result<_, _>>()?;
    for r in cloud
        .records
        .iter()
        .filter(|r| !linked_cloud.contains(&r.id))
    {
        items.push(Item {
            local_id: String::new(),
            cloud_id: Some(r.id.clone()),
            title: r.content.title.clone(),
            status: "cloudUnlinked".into(),
            message: "飞书记录尚未确认本地对应关系；不会按同名关联或自动导入。".into(),
            recommendation: None,
            local_evidence: Evidence {
                modified_at: None,
                source: TimeSource::Unknown,
                version: None,
            },
            cloud_evidence: Some(r.evidence.clone()),
            last_successful_sync_at: None,
            differences: Vec::new(),
        });
    }
    Ok(Preview {
        scope: scope.clone(),
        checked_at: now,
        read_only: true,
        items,
    })
}
pub(super) fn demo(now: i64) -> Result<SyncDemo, String> {
    let scope = Scope {
        platform: Platform::Feishu,
        subject_id: "示例账号（非当前登录）".into(),
        space_id: "示例任务空间".into(),
        table_id: "示例任务表（未连接真实表格）".into(),
    };
    let base = Content {
        title: "示例任务".into(),
        detail: "仅用于只读规则验收".into(),
        progress: 25,
        quadrant: "in".into(),
        today: true,
        start_date: None,
        planned_date: Some("2026-10-20".into()),
    };
    let evidence = |local, time| Evidence {
        modified_at: Some(now - time),
        source: if local {
            TimeSource::LocalClock
        } else {
            TimeSource::PlatformSystem
        },
        version: None,
    };
    let mut local = Vec::new();
    let mut cloud = Vec::new();
    let mut links = Vec::new();
    for (i, label) in [
        "仅本地更新",
        "仅云端更新",
        "双边冲突",
        "历史时间未知",
        "内容一致",
        "关联记录未读到",
        "尚未关联",
    ]
    .into_iter()
    .enumerate()
    {
        let id = format!("demo-{i}");
        let mut c = base.clone();
        c.title = format!("【同步示例】{label}");
        let mut l = Record {
            id: id.clone(),
            content: c.clone(),
            evidence: evidence(true, 60_000),
            trashed: false,
        };
        let mut r = Record {
            id: format!("record-{i}"),
            content: c.clone(),
            evidence: evidence(false, 60_000),
            trashed: false,
        };
        let baseline = Baseline {
            content: c,
            local_evidence: evidence(true, 120_000),
            cloud_evidence: evidence(false, 120_000),
        };
        match i {
            0 => {
                l.content.progress = 71;
                l.evidence = evidence(true, 30_000);
            }
            1 => {
                r.content.planned_date = Some("2026-10-25".into());
                r.evidence = evidence(false, 30_000);
            }
            2 => {
                l.content.progress = 71;
                r.content.planned_date = Some("2026-10-25".into());
                r.evidence = evidence(false, 10_000);
            }
            3 => {
                l.content.progress = 71;
                l.evidence.modified_at = None;
            }
            _ => {}
        }
        if i < 6 {
            links.push(Link {
                local_id: id,
                cloud_id: r.id.clone(),
                baseline: Some(baseline),
                last_successful_sync_at: Some(now - 120_000),
            });
        }
        if i != 5 && i != 6 {
            cloud.push(r);
        }
        local.push(l);
    }
    Ok(SyncDemo {
        simulated: true,
        label: "只读规则示例；不是当前账号或云表，未执行同步。".into(),
        preview: preview(
            &scope,
            &local,
            &links,
            &CloudRead {
                scope: scope.clone(),
                complete: true,
                records: cloud,
            },
            now,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn demo_covers_directions_and_never_advances_success_times() {
        let a = demo(1_800_000_000_000).unwrap();
        let items = &a.preview.items;
        assert_eq!(
            items.iter().map(|i| i.status.as_str()).collect::<Vec<_>>(),
            vec![
                "localChanged",
                "cloudChanged",
                "conflict",
                "manual",
                "equal",
                "unverified",
                "unlinked"
            ]
        );
        assert_eq!(items[0].recommendation.as_deref(), Some("localToCloud"));
        assert_eq!(items[1].recommendation.as_deref(), Some("cloudToLocal"));
        for i in &items[2..] {
            assert!(i.recommendation.is_none());
        }
        assert_eq!(
            items[0].last_successful_sync_at,
            Some(1_800_000_000_000 - 120_000)
        );
        assert!(a.preview.read_only && a.simulated);
    }
    #[test]
    fn read_times_application_metadata_future_equal_and_backwards_are_not_trusted() {
        let n = 1_800_000_000_000;
        for (v, source) in [
            (None, TimeSource::PlatformSystem),
            (Some(n), TimeSource::Unknown),
            (Some(n), TimeSource::ApplicationMetadata),
            (Some(n + 300_001), TimeSource::PlatformSystem),
            (Some(0), TimeSource::PlatformSystem),
        ] {
            assert!(!trusted(
                &Evidence {
                    modified_at: v,
                    source,
                    version: None
                },
                false,
                n
            ));
        }
        assert!(trusted(
            &Evidence {
                modified_at: Some(n),
                source: TimeSource::PlatformSystem,
                version: Some("opaque".into())
            },
            false,
            n
        ));
    }
    fn fixture() -> (Record, Record, Link, i64) {
        let n = 1_800_000_000_000;
        let c = Content {
            title: "同名任务".into(),
            detail: "详情".into(),
            progress: 25,
            quadrant: "in".into(),
            today: false,
            start_date: None,
            planned_date: None,
        };
        let le = Evidence {
            modified_at: Some(n - 120_000),
            source: TimeSource::LocalClock,
            version: Some("1".into()),
        };
        let ce = Evidence {
            modified_at: Some(n - 120_000),
            source: TimeSource::PlatformSystem,
            version: Some("opaque".into()),
        };
        let link = Link {
            local_id: "local".into(),
            cloud_id: "cloud".into(),
            baseline: Some(Baseline {
                content: c.clone(),
                local_evidence: le.clone(),
                cloud_evidence: ce.clone(),
            }),
            last_successful_sync_at: Some(n - 120_000),
        };
        (
            Record {
                id: "local".into(),
                content: c.clone(),
                evidence: le,
                trashed: false,
            },
            Record {
                id: "cloud".into(),
                content: c,
                evidence: ce,
                trashed: false,
            },
            link,
            n,
        )
    }
    #[test]
    fn ambiguous_times_missing_baseline_and_same_names_do_not_select_overwrites() {
        let (mut l, r, link, n) = fixture();
        l.content.progress = 71;
        for time in [
            None,
            Some(n - 120_001),
            Some(n - 120_000),
            Some(n + 300_001),
        ] {
            l.evidence.modified_at = time;
            let p = compare(&l, Some(&r), Some(&link), n).unwrap();
            assert_eq!(p.status, "manual");
            assert!(p.recommendation.is_none());
        }
        l.evidence.modified_at = Some(n - 30_000);
        let mut no_base = link.clone();
        no_base.baseline = None;
        assert_eq!(
            compare(&l, Some(&r), Some(&no_base), n).unwrap().status,
            "manual"
        );
        assert_eq!(compare(&l, Some(&r), None, n).unwrap().status, "unlinked");
        l.trashed = true;
        assert_eq!(
            compare(&l, Some(&r), Some(&link), n).unwrap().status,
            "deletionConflict"
        );
    }
    #[test]
    fn identical_content_ignores_metadata_and_preview_keeps_baseline_immutable() {
        let (mut l, r, link, n) = fixture();
        let before = serde_json::to_string(&link).unwrap();
        l.evidence.modified_at = None;
        l.evidence.version = Some("99".into());
        for _ in 0..3 {
            let p = compare(&l, Some(&r), Some(&link), n).unwrap();
            assert_eq!(p.status, "equal");
            assert!(p.differences.is_empty());
        }
        assert_eq!(serde_json::to_string(&link).unwrap(), before);
    }
    #[test]
    fn cloud_only_record_is_visible_but_never_auto_matched_by_name() {
        let (l, r, _, n) = fixture();
        let scope = Scope {
            platform: Platform::Feishu,
            subject_id: "u".into(),
            space_id: "s".into(),
            table_id: "t".into(),
        };
        let p = preview(
            &scope,
            &[l],
            &[],
            &CloudRead {
                scope: scope.clone(),
                complete: true,
                records: vec![r],
            },
            n,
        )
        .unwrap();
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.items[0].status, "unlinked");
        assert_eq!(p.items[1].status, "cloudUnlinked");
        assert!(p.items[1].local_id.is_empty());
        assert!(p
            .items
            .iter()
            .all(|i| i.recommendation.is_none() && i.last_successful_sync_at.is_none()));
    }

    #[test]
    fn real_pairing_preview_is_read_only_and_never_auto_links_same_name() {
        let (local, _, _, now) = fixture();
        let inspection = crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "第二账号".into(),
            subject_id: "cli:open".into(),
            app_id: "cli".into(),
            app_token: "base".into(),
            table_id: "table".into(),
            checked_at: now,
            records: vec![crate::feishu::SyncRecordTime {
                shared_fields: serde_json::json!({}),
                record_id: "cloud".into(),
                title: local.content.title.clone(),
                progress: 60,
                status: "进行中".into(),
                planned_date: Some("2026-10-10".into()),
                modified_at: Some(now - 1),
                time_source: "platformSystem".into(),
            }],
            conditional_write_verified: false,
            warnings: vec![],
        };
        let task = LocalTask {
            id: local.id,
            title: local.content.title,
            detail: local.content.detail,
            progress: local.content.progress,
            quadrant: local.content.quadrant,
            today: local.content.today,
            start_date: local.content.start_date,
            planned_date: local.content.planned_date,
            trashed: false,
            revision: 1,
            content_modified_at: local.evidence.modified_at,
        };
        let db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        let preview = pairing_preview(&db.connection, &[task], inspection).unwrap();
        assert!(preview.read_only && preview.requires_explicit_selection);
        assert_eq!(preview.local_candidates.len(), 1);
        assert_eq!(preview.cloud_candidates.len(), 1);
        assert!(preview
            .warnings
            .iter()
            .any(|value| value.contains("不会自动关联")));
    }
    #[test]
    fn explicit_pairing_saves_only_the_link_and_rejects_stale_selection() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        let local = db
            .save(super::super::tests::input(None, 25))
            .unwrap()
            .tasks
            .remove(0);
        let cloud = crate::feishu::SyncRecordTime {
            shared_fields: serde_json::json!({}),
            record_id: "cloud-record".into(),
            title: "云端候选".into(),
            progress: 60,
            status: "进行中".into(),
            planned_date: Some("2026-10-10".into()),
            modified_at: Some(1_791_684_436_000),
            time_source: "platformSystem".into(),
        };
        let inspection = crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "第二账号".into(),
            subject_id: "open-id".into(),
            app_id: "cli".into(),
            app_token: "base".into(),
            table_id: "table".into(),
            checked_at: 1_791_684_437_000,
            records: vec![],
            conditional_write_verified: false,
            warnings: vec![],
        };
        let input = SavePairingInput {
            local_id: local.id.clone(),
            cloud_id: cloud.record_id.clone(),
            expected_local_revision: local.revision,
            expected_local_modified_at: local.content_modified_at,
            expected_cloud_modified_at: cloud.modified_at,
        };
        let result = save_pairing(&mut db.connection, &local, &cloud, &inspection, &input).unwrap();
        assert!(!result.already_saved);
        assert!(!result.baseline_established);
        assert!(!result.successful_sync_recorded);
        assert!(!result.local_task_changed && !result.cloud_record_changed);
        let stored: String = db
            .connection
            .query_row("SELECT data FROM local_sync_links", [], |row| row.get(0))
            .unwrap();
        let stored: Link = serde_json::from_str(&stored).unwrap();
        assert!(stored.baseline.is_none() && stored.last_successful_sync_at.is_none());
        assert!(
            save_pairing(&mut db.connection, &local, &cloud, &inspection, &input)
                .unwrap()
                .already_saved
        );
        assert_eq!(
            existing_links(&db.connection, &scope_from_inspection(&inspection))
                .unwrap()
                .len(),
            1
        );
        let stale_cloud = SavePairingInput {
            expected_cloud_modified_at: Some(cloud.modified_at.unwrap() + 1),
            ..input.clone()
        };
        assert!(save_pairing(
            &mut db.connection,
            &local,
            &cloud,
            &inspection,
            &stale_cloud
        )
        .unwrap_err()
        .contains("飞书任务已发生变化"));
        let stale = SavePairingInput {
            expected_local_revision: local.revision + 1,
            ..input
        };
        assert!(
            save_pairing(&mut db.connection, &local, &cloud, &inspection, &stale)
                .unwrap_err()
                .contains("本地任务已发生变化")
        );
    }
    #[test]
    fn linked_fields_require_complete_equality_before_baseline_candidate() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        let local = db
            .save(super::super::tests::input(None, 25))
            .unwrap()
            .tasks
            .remove(0);
        let mut inspection = crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "test".into(),
            subject_id: "cli:open".into(),
            app_id: "cli".into(),
            app_token: "base".into(),
            table_id: "table".into(),
            checked_at: 1_791_684_437_000,
            records: vec![crate::feishu::SyncRecordTime {
                shared_fields: serde_json::to_value(Content::from(&local)).unwrap(),
                record_id: "cloud".into(),
                title: local.title.clone(),
                progress: local.progress,
                status: "进行中".into(),
                planned_date: local.planned_date.clone(),
                modified_at: Some(1_791_684_436_000),
                time_source: "platformSystem".into(),
            }],
            conditional_write_verified: false,
            warnings: vec![],
        };
        let links = vec![PairingSavedLink {
            baseline: None,
            local_id: local.id.clone(),
            cloud_id: "cloud".into(),
            baseline_established: false,
            last_successful_sync_at: None,
        }];
        let before = db.snapshot().unwrap();
        let result = linked_comparisons(&[local.clone()], &inspection, &links).unwrap();
        assert_eq!(result[0].fields.len(), 7);
        assert!(result[0].baseline_candidate_ready);
        inspection.records[0].shared_fields["progress"] = serde_json::json!(60);
        let result = linked_comparisons(&[local.clone()], &inspection, &links).unwrap();
        assert!(!result[0].baseline_candidate_ready);
        assert_eq!(result[0].fields.iter().filter(|f| !f.equal).count(), 1);
        // Exercise the real inspection path, including persisted baseline loading.
        let cloud = &inspection.records[0];
        save_pairing(
            &mut db.connection,
            &local,
            cloud,
            &inspection,
            &SavePairingInput {
                local_id: local.id.clone(),
                cloud_id: cloud.record_id.clone(),
                expected_local_revision: local.revision,
                expected_local_modified_at: local.content_modified_at,
                expected_cloud_modified_at: cloud.modified_at,
            },
        )
        .unwrap();
        let persisted = Link {
            local_id: local.id.clone(),
            cloud_id: cloud.record_id.clone(),
            baseline: Some(Baseline {
                content: Content::from(&local),
                local_evidence: Evidence {
                    modified_at: local.content_modified_at,
                    source: TimeSource::LocalClock,
                    version: None,
                },
                cloud_evidence: Evidence {
                    modified_at: cloud.modified_at,
                    source: TimeSource::PlatformSystem,
                    version: None,
                },
            }),
            last_successful_sync_at: Some(inspection.checked_at),
        };
        db.connection
            .execute(
                "UPDATE local_sync_links SET data=?1",
                [serde_json::to_string(&persisted).unwrap()],
            )
            .unwrap();
        let mut changed = local.clone();
        changed.progress = 71;
        let saved = existing_links(&db.connection, &scope_from_inspection(&inspection)).unwrap();
        let preview = linked_comparisons(&[changed], &inspection, &saved).unwrap();
        let serialized = serde_json::to_value(preview).unwrap();
        assert_eq!(
            serialized[0]["status"], "conflict",
            "real inspection must classify both sides relative to stored baseline"
        );
        // Content evidence, not a newer timestamp alone, controls classification.
        let stored_before: String = db
            .connection
            .query_row("SELECT data FROM local_sync_links", [], |r| r.get(0))
            .unwrap();
        let mut check = local.clone();
        let only_cloud = linked_comparisons(&[check.clone()], &inspection, &saved).unwrap();
        assert_ne!(only_cloud[0].status, "conflict");
        check.progress = 60;
        assert_eq!(
            linked_comparisons(&[check.clone()], &inspection, &saved).unwrap()[0].status,
            "equal"
        );
        check.progress = 71;
        check.content_modified_at = None;
        assert_eq!(
            linked_comparisons(&[check], &inspection, &saved).unwrap()[0].status,
            "conflict"
        );
        let stored_after: String = db
            .connection
            .query_row("SELECT data FROM local_sync_links", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            stored_before, stored_after,
            "inspection cannot advance baseline"
        );
        inspection.records[0]
            .shared_fields
            .as_object_mut()
            .unwrap()
            .remove("startDate");
        let result = linked_comparisons(&[local], &inspection, &links).unwrap();
        assert!(!result[0].baseline_candidate_ready);
        assert_eq!(result[0].status, "unverified");
        assert!(result[0]
            .fields
            .iter()
            .any(|f| f.field == "startDate" && !f.available));
        assert_eq!(
            serde_json::to_value(before.tasks).unwrap(),
            serde_json::to_value(db.snapshot().unwrap().tasks).unwrap()
        );
    }
    #[test]
    fn confirmed_download_is_atomic_rejects_stale_and_can_undo() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        let local = db
            .save(super::super::tests::input(None, 25))
            .unwrap()
            .tasks
            .remove(0);
        let mut content = Content::from(&local);
        content.progress = 60;
        let mut inspection = crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "test".into(),
            subject_id: "cli:open".into(),
            app_id: "cli".into(),
            app_token: "base".into(),
            table_id: "table".into(),
            checked_at: 1_791_684_437_000,
            records: vec![crate::feishu::SyncRecordTime {
                shared_fields: serde_json::to_value(&content).unwrap(),
                record_id: "cloud".into(),
                title: local.title.clone(),
                progress: 60,
                status: "进行中".into(),
                planned_date: local.planned_date.clone(),
                modified_at: Some(1_791_684_436_000),
                time_source: "platformSystem".into(),
            }],
            conditional_write_verified: false,
            warnings: vec![],
        };
        save_pairing(
            &mut db.connection,
            &local,
            &inspection.records[0],
            &inspection,
            &SavePairingInput {
                local_id: local.id.clone(),
                cloud_id: "cloud".into(),
                expected_local_revision: local.revision,
                expected_local_modified_at: local.content_modified_at,
                expected_cloud_modified_at: inspection.records[0].modified_at,
            },
        )
        .unwrap();
        let input = DownloadInput {
            local_id: local.id.clone(),
            cloud_id: "cloud".into(),
            subject_id: inspection.subject_id.clone(),
            expected_local_revision: local.revision,
            expected_local_modified_at: local.content_modified_at,
            expected_cloud_modified_at: inspection.records[0].modified_at,
            expected_cloud_fields: inspection.records[0].shared_fields.clone(),
        };
        inspection.subject_id = "other".into();
        assert!(download(&mut db.connection, &inspection, &input).is_err());
        inspection.subject_id = input.subject_id.clone();
        inspection.records[0].modified_at = Some(1);
        assert!(download(&mut db.connection, &inspection, &input).is_err());
        inspection.records[0].modified_at = input.expected_cloud_modified_at;
        db.connection.execute_batch("CREATE TRIGGER fail_sync_history BEFORE INSERT ON local_history BEGIN SELECT RAISE(ABORT,'fixture'); END;").unwrap();
        assert!(download(&mut db.connection, &inspection, &input).is_err());
        assert_eq!(get_task(&db.connection, &local.id).unwrap().unwrap(), local);
        assert!(
            !existing_links(&db.connection, &scope_from_inspection(&inspection)).unwrap()[0]
                .baseline_established
        );
        db.connection
            .execute_batch("DROP TRIGGER fail_sync_history")
            .unwrap();
        download(&mut db.connection, &inspection, &input).unwrap();
        assert_eq!(db.snapshot().unwrap().tasks[0].progress, 60);
        assert!(
            existing_links(&db.connection, &scope_from_inspection(&inspection)).unwrap()[0]
                .baseline_established
        );
        assert!(
            download(&mut db.connection, &inspection, &input).is_err(),
            "stale repeat must not apply twice"
        );
        assert_eq!(db.undo().unwrap().tasks[0].progress, 25);
    }
    #[test]
    fn wrong_identity_partial_pages_and_duplicate_ids_block_preview() {
        let (l, r, link, n) = fixture();
        let scope = Scope {
            platform: Platform::Feishu,
            subject_id: "a".into(),
            space_id: "s".into(),
            table_id: "t".into(),
        };
        let mut read = CloudRead {
            scope: scope.clone(),
            complete: false,
            records: vec![r.clone()],
        };
        assert!(preview(&scope, &[l.clone()], &[link.clone()], &read, n).is_err());
        read.complete = true;
        for field in [0, 1, 2] {
            read.scope = scope.clone();
            match field {
                0 => read.scope.subject_id = "b".into(),
                1 => read.scope.table_id = "other".into(),
                _ => read.scope.platform = Platform::Wps,
            };
            assert!(preview(&scope, &[l.clone()], &[link.clone()], &read, n).is_err());
        }
        read.scope = scope.clone();
        read.records.push(r);
        assert!(preview(&scope, &[l.clone()], &[link.clone()], &read, n).is_err());
        read.records.pop();
        assert!(preview(&scope, &[l.clone(), l], &[link], &read, n).is_err());
    }
    #[test]
    fn bindings_survive_reopen_and_cannot_fake_a_successful_first_link() {
        let path = std::env::temp_dir().join(format!(
            "local-sync-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut db = LocalWorkspace::open(&path).unwrap();
        let t = db
            .save(super::super::tests::input(None, 25))
            .unwrap()
            .tasks
            .remove(0);
        let scope = Scope {
            platform: Platform::Wps,
            subject_id: "user".into(),
            space_id: "file".into(),
            table_id: "sheet".into(),
        };
        let id = register(&db.connection, &scope).unwrap();
        let mut link = Link {
            local_id: t.id,
            cloud_id: "record".into(),
            baseline: None,
            last_successful_sync_at: Some(1),
        };
        assert!(bind(&db.connection, &id, &link).is_err());
        link.last_successful_sync_at = None;
        bind(&db.connection, &id, &link).unwrap();
        drop(db);
        let reopened = LocalWorkspace::open(&path).unwrap();
        assert_eq!(register(&reopened.connection, &scope).unwrap(), id);
        let data: String = reopened
            .connection
            .query_row(
                "SELECT data FROM local_sync_links WHERE connection_id=?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        let stored: Link = serde_json::from_str(&data).unwrap();
        assert_eq!(stored.cloud_id, "record");
        assert!(stored.baseline.is_none() && stored.last_successful_sync_at.is_none());
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn connections_and_links_are_account_scoped_persistent_unique_and_restore_detachable() {
        let mut db = LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap();
        let t = db
            .save(super::super::tests::input(None, 25))
            .unwrap()
            .tasks
            .remove(0);
        let mut scope = Scope {
            platform: Platform::Feishu,
            subject_id: "user-a".into(),
            space_id: "app".into(),
            table_id: "table".into(),
        };
        let a = register(&db.connection, &scope).unwrap();
        assert_eq!(a, register(&db.connection, &scope).unwrap());
        scope.subject_id = "user-b".into();
        let b = register(&db.connection, &scope).unwrap();
        assert_ne!(a, b);
        let link = Link {
            local_id: t.id,
            cloud_id: "same-record-id".into(),
            baseline: None,
            last_successful_sync_at: None,
        };
        bind(&db.connection, &a, &link).unwrap();
        bind(&db.connection, &b, &link).unwrap();
        assert!(bind(&db.connection, &a, &link).is_err());
        db.connection
            .execute("DELETE FROM local_sync_connections", [])
            .unwrap();
        assert_eq!(
            db.connection
                .query_row("SELECT COUNT(*) FROM local_sync_links", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }
}
