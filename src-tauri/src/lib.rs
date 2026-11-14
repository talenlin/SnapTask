mod ai;
mod local_workbench;
mod local_cloud_access;
#[cfg(feature="local-main-flow-ui-acceptance")]
mod local_main_flow_lab;
mod feishu;
mod feishu_auth;
mod direct_account_scope;
#[cfg(feature = "feishu-direct-sync-ui-acceptance")]
pub mod startup_diagnostics;
#[cfg(all(feature = "feishu-direct-account-acceptance",not(feature="feishu-direct-sync-ui-acceptance")))]
mod direct_account_auth;
mod retrieval;
mod storage;
mod task_source;
pub mod workspace_provisioning;
mod wps;
pub mod wps_open_schema;
mod write_acceptance;
mod floating_feishu;
mod floating_tasks;

use std::{
    collections::HashSet,
    fs,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

use ai::{AiConfig, ListModelsInput, ModelListResult, SaveAiConfigInput};
use feishu::{
    FeishuConfig, FeishuSchemaInitResult, FeishuSchemaPlan, FeishuSyncResult, FeishuUserAuthStatus,
    FeishuWorkspaceInitResult, FeishuWriteRequest, SaveFeishuConfigInput,
    SaveFeishuUserSessionInput,
};
use feishu_auth::{FeishuAuthFlow, FeishuAuthProgress};
use retrieval::RetrievalStats;
use storage::{
    AnalysisResult, ConfirmInput, Habit, PendingWriteSummary, Storage, SyncedWriteLog, Task,
    UndoResult,
};
use task_source::TaskSourceDescriptor;
use tauri::{
    menu::MenuBuilder,
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewWindow,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use workspace_provisioning::{
    FeishuWorkspaceAdapter, ProvisionMode, ProvisionPhase, ProvisionRequest, ProvisionResult,
    ProvisionState, WorkspacePlatform, WorkspaceReference,
};
use wps::{SaveWpsConfigInput, WpsConfig, WpsSyncResult, WpsWriteRequest};

struct AppState {
    storage: Mutex<Storage>,
    write_lock: Mutex<()>,
    feishu_auth_flow: Mutex<Option<FeishuAuthFlow>>,
    floating: floating_feishu::State,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmResult {
    task: Option<Task>,
    synced: bool,
    queued: bool,
    habit_saved: bool,
    habit_warning: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TemplateSaveResult {
    saved: bool,
    file_name: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct HabitLearningInput {
    phrase: String,
    kind: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeResponse {
    analysis: AnalysisResult,
    retrieval: Option<RetrievalStats>,
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskSyncResult {
    source: String,
    source_name: String,
    task_count: usize,
    completed_count: usize,
    fields: Vec<String>,
    write_ready: bool,
    undo_ready: bool,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskWorkspaceProvisionResponse {
    state: ProvisionState,
    phase: ProvisionPhase,
    workspace_name: Option<String>,
    workspace_url: Option<String>,
    message: Option<String>,
}

impl From<ProvisionResult> for TaskWorkspaceProvisionResponse {
    fn from(result: ProvisionResult) -> Self {
        let (workspace_name, workspace_url) = result
            .summary
            .map(|summary| (Some(summary.workspace_name), summary.workspace_url))
            .unwrap_or_default();
        Self {
            state: result.state,
            phase: result.phase,
            workspace_name,
            workspace_url,
            message: result.message,
        }
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskWorkspaceSetupStatus {
    template_available: bool,
    user_authorized: bool,
    has_pending_operation: bool,
    phase: Option<ProvisionPhase>,
    needs_reconciliation: bool,
    remote_mutation_started: bool,
    workspace_name: Option<String>,
    workspace_url: Option<String>,
}

impl From<WpsSyncResult> for TaskSyncResult {
    fn from(value: WpsSyncResult) -> Self {
        Self {
            source: "wps".to_string(),
            source_name: value.sheet_name,
            task_count: value.task_count,
            completed_count: value.completed_count,
            fields: value.fields,
            write_ready: value.write_ready,
            undo_ready: value.undo_ready,
        }
    }
}

impl From<FeishuSyncResult> for TaskSyncResult {
    fn from(value: FeishuSyncResult) -> Self {
        Self {
            source: "feishu".to_string(),
            source_name: value.table_id,
            task_count: value.task_count,
            completed_count: value.completed_count,
            fields: value.fields,
            write_ready: value.write_ready,
            undo_ready: value.undo_ready,
        }
    }
}

fn verified_written_task(before: &[Task], after: &[Task], input: &ConfirmInput) -> Option<Task> {
    let expected_status = desired_status(input.progress);
    if let Some(task_id) = input.task_id.as_ref() {
        return after
            .iter()
            .find(|task| {
                &task.id == task_id
                    && task.progress == input.progress
                    && task.status == expected_status
                    && (!input.change_planned_date || task.planned_date == input.planned_date)
            })
            .cloned();
    }
    let before_ids = before
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    let title = input.temporary_title.as_deref().unwrap_or_default().trim();
    after
        .iter()
        .find(|task| {
            !before_ids.contains(task.id.as_str())
                && task.title == title
                && task.progress == input.progress
                && task.status == expected_status
                && (!input.change_planned_date || task.planned_date == input.planned_date)
        })
        .cloned()
}

fn remote_task_changed(input: &ConfirmInput, current: &Task) -> bool {
    input
        .expected_title
        .as_deref()
        .is_some_and(|expected| expected != current.title)
        || input
            .expected_progress
            .is_some_and(|expected| expected != current.progress)
        || input
            .expected_status
            .as_deref()
            .is_some_and(|expected| expected != current.status)
        || (input.expected_planned_date_known
            && input.expected_planned_date != current.planned_date)
}

fn remote_matches_synced_write(entry: &SyncedWriteLog, current: &Task) -> bool {
    if entry.created_task {
        current.progress == entry.next_progress
            && current.title == entry.task_title
            && current.detail == entry.description
            && (!entry.planned_date_changed
                || current.planned_date
                    == entry
                        .observed_planned_date
                        .clone()
                        .or_else(|| entry.next_planned_date.clone()))
    } else {
        let written_status = desired_status(entry.next_progress);
        current.progress == entry.observed_progress.unwrap_or(entry.next_progress)
            && current.status == entry.observed_status.as_deref().unwrap_or(written_status)
            && (!entry.planned_date_changed
                || current.planned_date
                    == entry
                        .observed_planned_date
                        .clone()
                        .or_else(|| entry.next_planned_date.clone()))
    }
}

fn feishu_conflict_message(input: &ConfirmInput, current: &Task) -> String {
    let date_detail = if input.expected_planned_date_known
        && input.expected_planned_date != current.planned_date
    {
        format!("；计划日期由 {} 变为 {}",
            input.expected_planned_date.as_deref().unwrap_or("未设置"),
            current.planned_date.as_deref().unwrap_or("未设置"))
    } else { String::new() };
    format!(
        "检测到飞书冲突：分析时为 {}%、{}，当前为 {}%、{}{}。为避免覆盖，本次没有写入；请重新分析",
        input.expected_progress.map(|value| value.to_string()).unwrap_or_else(|| "未知".to_string()),
        input.expected_status.as_deref().unwrap_or("未知"), current.progress, current.status, date_detail
    )
}

fn remote_matches_partial_blank_status_undo(entry: &SyncedWriteLog, current: &Task) -> bool {
    if entry.created_task || entry.previous_status.as_deref() != Some("") {
        return false;
    }
    let written_status = desired_status(entry.next_progress);
    entry.previous_progress == Some(current.progress) && current.status == written_status
}

fn validate_confirm_input(input: &ConfirmInput) -> Result<(), String> {
    if input.description.trim().is_empty() || input.progress > 100 {
        return Err("写入内容无效".to_string());
    }
    if input.change_planned_date {
        let date = input
            .planned_date
            .as_deref()
            .ok_or_else(|| "请选择新的计划完成日期".to_string())?;
        if !valid_iso_date(date) {
            return Err("计划完成日期格式无效，请重新选择日期".to_string());
        }
    }
    if input.task_id.is_some() {
        if input.expected_progress.is_none() || input.expected_status.is_none() {
            return Err("任务确认信息不完整；请重新分析后再写入".to_string());
        }
        return Ok(());
    }
    input
        .temporary_title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "临时任务名称不能为空".to_string())?;
    input
        .quadrant
        .as_deref()
        .filter(|value| ["iu", "un", "in", "nn"].contains(value))
        .ok_or_else(|| "新增临时任务前请选择象限".to_string())?;
    Ok(())
}

fn build_write_request(input: &ConfirmInput) -> Result<WpsWriteRequest, String> {
    if let Some(task_id) = input.task_id.as_ref() {
        let desired_status = desired_status(input.progress);
        let status = input
            .expected_status
            .as_deref()
            .filter(|current| *current != desired_status)
            .map(|_| desired_status.to_string());
        return Ok(WpsWriteRequest::Update {
            record_id: task_id.clone(),
            progress: input.progress,
            status,
            planned_date: if input.change_planned_date {
                Some(input.planned_date.clone())
            } else {
                None
            },
            restore: false,
        });
    }
    Ok(WpsWriteRequest::CreateTemporary {
        title: input
            .temporary_title
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        description: input.description.trim().to_string(),
        progress: input.progress,
        quadrant: input.quadrant.clone().unwrap_or_default(),
        today: input.today,
        planned_date: input
            .change_planned_date
            .then(|| input.planned_date.clone())
            .flatten(),
    })
}

fn build_feishu_write_request(input: &ConfirmInput) -> Result<FeishuWriteRequest, String> {
    if let Some(task_id) = input.task_id.as_ref() {
        let desired_status = desired_status(input.progress);
        let status = input
            .expected_status
            .as_deref()
            .filter(|current| *current != desired_status)
            .map(|_| desired_status.to_string());
        return Ok(FeishuWriteRequest::Update {
            record_id: task_id.clone(),
            progress: input.progress,
            status,
            planned_date: if input.change_planned_date {
                Some(input.planned_date.clone())
            } else {
                None
            },
        });
    }
    Ok(FeishuWriteRequest::CreateTemporary {
        title: input
            .temporary_title
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_string(),
        description: input.description.trim().to_string(),
        progress: input.progress,
        quadrant: input.quadrant.clone().unwrap_or_default(),
        today: input.today,
        planned_date: input
            .change_planned_date
            .then(|| input.planned_date.clone())
            .flatten(),
    })
}

fn valid_iso_date(value: &str) -> bool {
    let parts = value.split('-').collect::<Vec<_>>();
    if parts.len() != 3 || parts[0].len() != 4 || parts[1].len() != 2 || parts[2].len() != 2 {
        return false;
    }
    let Ok(year) = parts[0].parse::<u16>() else {
        return false;
    };
    let Ok(month) = parts[1].parse::<u8>() else {
        return false;
    };
    let Ok(day) = parts[2].parse::<u8>() else {
        return false;
    };
    if year < 2000 || !(1..=12).contains(&month) {
        return false;
    }
    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=max_day).contains(&day)
}

fn desired_status(progress: u8) -> &'static str {
    match progress {
        0 => "未开始",
        100 => "已完成",
        _ => "进行中",
    }
}

fn possible_partial_write(input: &ConfirmInput, current: &Task) -> bool {
    input.task_id.as_deref() == Some(current.id.as_str())
        && input.expected_title.as_deref() == Some(current.title.as_str())
        && input
            .expected_progress
            .is_some_and(|value| value != current.progress)
        && current.progress == input.progress
        && current.status != desired_status(input.progress)
}

fn previous_task_from_snapshot(input: &ConfirmInput, current: &Task) -> Option<Task> {
    Some(Task {
        title: input.expected_title.clone()?,
        progress: input.expected_progress?,
        status: input.expected_status.clone()?,
        planned_date: input.expected_planned_date.clone(),
        ..current.clone()
    })
}

fn persist_uncertain_write(
    state: &AppState,
    input: &ConfirmInput,
    previous: Option<&Task>,
    observed: Option<&Task>,
    habit: Option<&HabitLearningInput>,
    replaces_queue_id: Option<i64>,
    error: &str,
) -> Result<(), String> {
    let habit_json = habit
        .map(serde_json::to_string)
        .transpose()
        .map_err(|serialize_error| serialize_error.to_string())?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    if let Some(previous) = previous {
        storage.record_uncertain_wps_write(input, previous, observed)?;
    }
    if let Some(id) = replaces_queue_id {
        storage.replace_pending_write(id, input, habit_json.as_deref(), error)?;
        storage.set_pending_write_state(id, "uncertain", error)?;
    } else {
        storage.enqueue_uncertain_write(input, habit_json.as_deref(), error)?;
    }
    Ok(())
}

#[cfg(all(test, windows, feature = "feishu-template-v2-acceptance", feature = "feishu-write-acceptance"))]
mod feishu_final_safety_acceptance {
    use super::*;

    #[test]
    #[ignore = "one real scoped update with injected post-delivery read failure, blocked repeat, and verified undo; never retries"]
    fn live_final_uncertain_delivery_blocks_repeat_and_undo_restores() {
        let report_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap().join("artifacts/feishu-v2-cross-account/FSV2-final-uncertain-delivery.json");
        assert!(!report_path.exists(), "evidence already exists; inspect it instead of replaying writes");
        write_acceptance::configure(write_acceptance::TEMPLATE_V2_FINAL_IDENTIFIER).expect("isolated identity");
        let storage = Storage::open(std::path::PathBuf::from(":memory:")).expect("independent in-memory acceptance log");
        for (key, value) in [
            ("feishu.app_id", write_acceptance::TEMPLATE_V2_APP_ID),
            ("feishu.app_token", write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN),
            ("feishu.table_id", write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID),
            ("task_source.active", "feishu"),
        ] { storage.set_setting(key, value).expect("non-secret scoped setting"); }
        let state = AppState { storage: Mutex::new(storage), write_lock: Mutex::new(()), feishu_auth_flow: Mutex::new(None), floating:floating_feishu::State::open(":memory:").unwrap() };
        let (config, token) = saved_feishu_credentials(&state).expect("bound authorization");
        let read_cloud = || feishu::fetch_tasks_with_access_token(&config, &token, storage::unix_now()).expect("independent cloud GET").0;
        let before = read_cloud();
        assert_eq!(before.len(), 1);
        let original = before[0].clone();
        assert_eq!(original.id, "rec0000000000001");
        assert_eq!(original.title, "【完整性复验】模板联动示例");
        assert_eq!(original.progress, 57);
        assert_eq!(original.status, "进行中");
        assert_eq!(original.planned_date.as_deref(), Some("2026-10-10"));
        state.storage.lock().unwrap().replace_remote_tasks(&before, "feishu").unwrap();
        let input = ConfirmInput {
            task_id: Some(original.id.clone()), temporary_title: None,
            description: "【受控验收】真实写入后模拟回读失败".into(),
            progress: 71, quadrant: None, today: false,
            change_planned_date: false, planned_date: None,
            expected_title: Some(original.title.clone()), expected_progress: Some(57),
            expected_status: Some(original.status.clone()),
            expected_planned_date_known: true, expected_planned_date: original.planned_date.clone(),
        };
        let writes = std::cell::Cell::new(0);
        let first = confirm_feishu_update_with_io(&state, input.clone(), None, None,
            |config, token, request| {
                writes.set(writes.get() + 1);
                feishu::write_task_with_access_token(config, token, request)
            },
            |_, _| Err("受控测试：仅模拟已送达后的回读不可用".into()));
        let failure = first.err().expect("uncertain, not success");
        assert!(failure.contains("核对与撤销记录") && failure.contains("请勿重复提交"));
        assert_eq!(writes.get(), 1, "no automatic retry");
        let delivered = read_cloud();
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].id, original.id);
        assert_eq!(delivered[0].progress, 71);
        assert_eq!(delivered[0].status, "进行中");
        assert_eq!(delivered[0].planned_date, original.planned_date);
        let log = state.storage.lock().unwrap().latest_synced_write().unwrap().expect("retained recovery log");
        assert_eq!(log.source, "feishu");
        assert_eq!(log.task_id, original.id);
        assert_eq!(log.previous_progress, Some(57));
        assert_eq!(log.next_progress, 71);
        let second = confirm_feishu_update_with_io(&state, input, None, None,
            |_, _, _| { writes.set(writes.get() + 1); panic!("stale confirmation must not reach PUT") },
            |_, _| panic!("stale confirmation must not reach post-write readback"));
        assert!(second.err().expect("repeat rejected").contains("本次没有写入"));
        assert_eq!(writes.get(), 1);
        let undo = undo_latest_feishu(&state, log).expect("one explicit recovery undo; never replay on failure");
        assert!(undo.undone);
        let restored = read_cloud();
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].id, original.id);
        assert_eq!(restored[0].progress, 57);
        assert_eq!(restored[0].status, original.status);
        assert_eq!(restored[0].planned_date, original.planned_date);
        assert!(state.storage.lock().unwrap().latest_synced_write().unwrap().is_none());
        let report = serde_json::json!({
            "executedOn": "2026-10-02", "appToken": config.app_token, "tableId": config.table_id,
            "recordId": original.id, "realCloudWrite": true,
            "injectedFailure": "post-delivery readback error only; not a real provider outage",
            "normalEntryAlwaysUsesRealIo": true, "testLogStorage": "independent in-memory SQLite",
            "initialProgress": 57, "deliveredProgress": 71, "updateAttempts": writes.get(),
            "automaticWriteRetries": 0, "repeatedConfirmationBlockedBeforeWrite": true,
            "recoveryLogRetained": true, "explicitUndoCloudReadback": true,
            "finalRecordCount": restored.len(), "finalProgress": 57,
            "finalStatus": restored[0].status, "finalPlannedDate": restored[0].planned_date,
            "passed": true
        });
        fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).expect("save non-secret evidence");
        println!("UNCERTAIN_DELIVERY_ACCEPTANCE {}", report);
    }
}

#[cfg(test)]
mod write_verification_tests {
    use super::*;

    fn task(id: &str, title: &str, progress: u8, status: &str) -> Task {
        Task {
            id: id.to_string(),
            title: title.to_string(),
            detail: "说明".to_string(),
            progress,
            quadrant: "in".to_string(),
            status: status.to_string(),
            today: false,
            planned_date: None,
            updated_at: 1,
        }
    }

    #[test]
    fn verifies_existing_task_only_when_progress_and_status_match() {
        let before = vec![task("rec-1", "E技术报告", 40, "进行中")];
        let after = vec![task("rec-1", "E技术报告", 60, "进行中")];
        let input = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "补充测试数据".to_string(),
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
        assert_eq!(
            verified_written_task(&before, &after, &input)
                .expect("verified task")
                .id,
            "rec-1"
        );
        let mismatched = vec![task("rec-1", "E技术报告", 59, "进行中")];
        assert!(verified_written_task(&before, &mismatched, &input).is_none());
    }

    #[test]
    fn progress_regression_omits_unchanged_status() {
        let input = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "方案需要再次修改".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(80),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        let WpsWriteRequest::Update {
            status, restore, ..
        } = build_write_request(&input).expect("update request")
        else {
            panic!("expected update request")
        };
        assert_eq!(status, None);
        assert!(!restore);
    }

    #[test]
    fn reschedule_repairs_an_unreadable_status_with_the_expected_text() {
        let input = ConfirmInput {
            task_id: Some("rec-date".to_string()),
            temporary_title: None,
            description: "计划日期验收任务延期到2026-10-20".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            change_planned_date: true,
            planned_date: Some("2026-10-20".to_string()),
            expected_title: Some("计划日期验收任务".to_string()),
            expected_progress: Some(40),
            expected_status: Some(String::new()),
            expected_planned_date_known: true,
            expected_planned_date: Some("2026-10-15".to_string()),
        };
        let WpsWriteRequest::Update {
            status,
            planned_date,
            ..
        } = build_write_request(&input).expect("reschedule request")
        else {
            panic!("expected update request")
        };
        assert_eq!(status.as_deref(), Some("进行中"));
        assert_eq!(planned_date, Some(Some("2026-10-20".to_string())));
    }

    #[test]
    fn completion_sends_status_only_when_it_changes() {
        let input = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "任务已经完成".to_string(),
            progress: 100,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(80),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        let WpsWriteRequest::Update {
            status, restore, ..
        } = build_write_request(&input).expect("update request")
        else {
            panic!("expected update request")
        };
        assert_eq!(status.as_deref(), Some("已完成"));
        assert!(!restore);
    }

    #[test]
    fn zero_progress_changes_status_to_not_started() {
        let input = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "任务尚未开始".to_string(),
            progress: 0,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(25),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        let WpsWriteRequest::Update { status, .. } =
            build_write_request(&input).expect("update request")
        else {
            panic!("expected update request")
        };
        assert_eq!(status.as_deref(), Some("未开始"));
        assert_eq!(desired_status(25), "进行中");
        assert_eq!(desired_status(100), "已完成");
    }

    #[test]
    fn verifies_temporary_task_by_new_record_id_and_values() {
        let before = vec![task("rec-1", "E技术报告", 40, "进行中")];
        let after = vec![
            task("rec-1", "E技术报告", 40, "进行中"),
            task("rec-2", "准备临时会议", 20, "进行中"),
        ];
        let input = ConfirmInput {
            task_id: None,
            temporary_title: Some("准备临时会议".to_string()),
            description: "这是临时任务：准备临时会议".to_string(),
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
        };
        assert_eq!(
            verified_written_task(&before, &after, &input)
                .expect("verified new task")
                .id,
            "rec-2"
        );
    }

    #[test]
    fn detects_progress_or_status_changed_after_analysis() {
        let current = task("rec-1", "E技术报告", 50, "进行中");
        let unchanged = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "补充测试数据".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            expected_title: Some("E技术报告".to_string()),
            expected_progress: Some(50),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        assert!(!remote_task_changed(&unchanged, &current));

        let changed_progress = ConfirmInput {
            expected_progress: Some(40),
            ..unchanged.clone()
        };
        assert!(remote_task_changed(&changed_progress, &current));

        let changed_status = ConfirmInput {
            expected_status: Some("未开始".to_string()),
            ..unchanged.clone()
        };
        assert!(remote_task_changed(&changed_status, &current));

        let changed_title = ConfirmInput {
            expected_title: Some("D项目调研与规划".to_string()),
            ..unchanged
        };
        assert!(remote_task_changed(&changed_title, &current));
    }

    #[test]
    fn accepts_legacy_confirmation_without_snapshot() {
        let current = task("rec-1", "E技术报告", 50, "进行中");
        let input = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "补充测试数据".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            expected_title: None,
            expected_progress: None,
            expected_status: None,
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        assert!(!remote_task_changed(&input, &current));
    }

    #[test]
    fn accepts_an_empty_status_as_a_real_confirmation_snapshot() {
        let input = ConfirmInput {
            task_id: Some("rec-1".to_string()),
            temporary_title: None,
            description: "D项目需要回退".to_string(),
            progress: 39,
            quadrant: None,
            today: false,
            expected_title: Some("D项目调研与规划".to_string()),
            expected_progress: Some(40),
            expected_status: Some(String::new()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        assert!(validate_confirm_input(&input).is_ok());
    }

    #[test]
    fn cloud_undo_only_accepts_the_exact_written_remote_state() {
        let entry = SyncedWriteLog {
            id: 1,
            source: "wps".to_string(),
            task_id: "rec-1".to_string(),
            task_title: "E技术报告".to_string(),
            description: "补充测试数据".to_string(),
            previous_progress: Some(40),
            next_progress: 60,
            previous_status: Some("进行中".to_string()),
            created_task: false,
            observed_progress: Some(60),
            observed_status: Some("进行中".to_string()),
            previous_planned_date: None,
            next_planned_date: None,
            planned_date_changed: false,
            observed_planned_date: None,
        };
        assert!(remote_matches_synced_write(
            &entry,
            &task("rec-1", "E技术报告", 60, "进行中")
        ));
        assert!(!remote_matches_synced_write(
            &entry,
            &task("rec-1", "E技术报告", 70, "进行中")
        ));
        assert!(!remote_matches_synced_write(
            &entry,
            &task("rec-1", "E技术报告", 60, "已完成")
        ));

        let created = SyncedWriteLog {
            id: 2,
            source: "wps".to_string(),
            task_id: "rec-2".to_string(),
            task_title: "资料核对".to_string(),
            description: "完成资料核对并提交项目组".to_string(),
            previous_progress: None,
            next_progress: 100,
            previous_status: None,
            created_task: true,
            observed_progress: Some(100),
            observed_status: Some("已完成".to_string()),
            previous_planned_date: None,
            next_planned_date: None,
            planned_date_changed: false,
            observed_planned_date: None,
        };
        let mut created_task = task("rec-2", "资料核对", 100, "已完成");
        created_task.detail = created.description.clone();
        assert!(remote_matches_synced_write(&created, &created_task));
        created_task.detail = "后来手动修改的详情".to_string();
        assert!(!remote_matches_synced_write(&created, &created_task));
    }

    #[test]
    fn cloud_undo_can_repair_the_old_blank_status_partial_restore() {
        let entry = SyncedWriteLog {
            id: 3,
            source: "wps".to_string(),
            task_id: "rec-3".to_string(),
            task_title: "C技术方案".to_string(),
            description: "需要再次修改".to_string(),
            previous_progress: Some(80),
            next_progress: 60,
            previous_status: Some(String::new()),
            created_task: false,
            observed_progress: None,
            observed_status: None,
            previous_planned_date: None,
            next_planned_date: None,
            planned_date_changed: false,
            observed_planned_date: None,
        };
        assert!(remote_matches_partial_blank_status_undo(
            &entry,
            &task("rec-3", "C技术方案", 80, "进行中")
        ));
        assert!(!remote_matches_partial_blank_status_undo(
            &entry,
            &task("rec-3", "C技术方案", 70, "进行中")
        ));
        assert!(!remote_matches_partial_blank_status_undo(
            &entry,
            &task("rec-3", "C技术方案", 80, "未开始")
        ));
    }

    #[test]
    fn cloud_undo_accepts_the_exact_observed_uncertain_state() {
        let entry = SyncedWriteLog {
            id: 4,
            source: "wps".to_string(),
            task_id: "rec-4".to_string(),
            task_title: "D项目调研与规划".to_string(),
            description: "项目推进遇到困难，先做技术分析".to_string(),
            previous_progress: Some(40),
            next_progress: 25,
            previous_status: Some(String::new()),
            created_task: false,
            observed_progress: Some(25),
            observed_status: Some(String::new()),
            previous_planned_date: None,
            next_planned_date: None,
            planned_date_changed: false,
            observed_planned_date: None,
        };
        assert!(remote_matches_synced_write(
            &entry,
            &task("rec-4", "D项目调研与规划", 25, "")
        ));
        assert!(!remote_matches_synced_write(
            &entry,
            &task("rec-4", "D项目调研与规划", 25, "进行中")
        ));
    }

    #[test]
    fn repeated_confirmation_recognizes_a_partial_previous_delivery() {
        let input = ConfirmInput {
            task_id: Some("rec-4".to_string()),
            temporary_title: None,
            description: "项目推进遇到困难，先做技术分析".to_string(),
            progress: 25,
            quadrant: None,
            today: false,
            expected_title: Some("D项目调研与规划".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            change_planned_date: false,
            planned_date: None,
            expected_planned_date_known: false,
            expected_planned_date: None,
        };
        assert!(possible_partial_write(
            &input,
            &task("rec-4", "D项目调研与规划", 25, "")
        ));
        assert!(!possible_partial_write(
            &input,
            &task("rec-4", "D项目调研与规划", 25, "进行中")
        ));
    }

    #[test]
    fn ordinary_progress_update_never_writes_the_planned_date() {
        let input = ConfirmInput {
            task_id: Some("rec-date".to_string()),
            temporary_title: None,
            description: "继续完成技术分析".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            change_planned_date: false,
            planned_date: None,
            expected_title: Some("E技术分析".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            expected_planned_date_known: true,
            expected_planned_date: Some("2026-10-08".to_string()),
        };
        let WpsWriteRequest::Update { planned_date, .. } =
            build_write_request(&input).expect("update request")
        else {
            panic!("expected update request")
        };
        assert_eq!(planned_date, None);
    }

    #[test]
    fn explicit_reschedule_writes_and_verifies_the_new_date() {
        assert!(valid_iso_date("2028-02-29"));
        assert!(!valid_iso_date("2027-02-29"));
        assert!(!valid_iso_date("2026-04-31"));
        let mut before_task = task("rec-date", "E技术分析", 40, "进行中");
        before_task.planned_date = Some("2026-10-08".to_string());
        let input = ConfirmInput {
            task_id: Some("rec-date".to_string()),
            temporary_title: None,
            description: "E技术分析延期到2026-10-20".to_string(),
            progress: 60,
            quadrant: None,
            today: false,
            change_planned_date: true,
            planned_date: Some("2026-10-20".to_string()),
            expected_title: Some("E技术分析".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            expected_planned_date_known: true,
            expected_planned_date: Some("2026-10-08".to_string()),
        };
        let WpsWriteRequest::Update { planned_date, .. } =
            build_write_request(&input).expect("update request")
        else {
            panic!("expected update request")
        };
        assert_eq!(planned_date, Some(Some("2026-10-20".to_string())));

        let mut after_task = before_task.clone();
        after_task.progress = 60;
        after_task.planned_date = Some("2026-10-20".to_string());
        assert!(verified_written_task(&[before_task], &[after_task], &input).is_some());
    }

    #[test]
    fn planned_date_snapshot_prevents_overwriting_a_concurrent_change() {
        let mut current = task("rec-date", "E技术分析", 40, "进行中");
        current.planned_date = Some("2026-10-18".to_string());
        let input = ConfirmInput {
            task_id: Some("rec-date".to_string()),
            temporary_title: None,
            description: "延期到2026-10-20".to_string(),
            progress: 40,
            quadrant: None,
            today: false,
            change_planned_date: true,
            planned_date: Some("2026-10-20".to_string()),
            expected_title: Some("E技术分析".to_string()),
            expected_progress: Some(40),
            expected_status: Some("进行中".to_string()),
            expected_planned_date_known: true,
            expected_planned_date: Some("2026-10-08".to_string()),
        };
        assert!(remote_task_changed(&input, &current));
        let message = feishu_conflict_message(&input, &current);
        assert!(message.contains("计划日期由 2026-10-08 变为 2026-10-18"), "{message}");
        assert!(message.contains("本次没有写入"));
    }
}

#[cfg(windows)]
fn apply_native_material_frame(window: &WebviewWindow) -> Result<(), String> {
    use std::{ffi::c_void, mem::size_of};
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        color: u32,
        animation: u32,
    }

    #[repr(C)]
    struct CompositionAttributeData {
        attribute: u32,
        data: *mut c_void,
        size: usize,
    }

    type SetWindowCompositionAttribute =
        unsafe extern "system" fn(*mut c_void, *mut CompositionAttributeData) -> i32;

    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    let border_color = 0xffff_fffe_u32;
    let corner_preference = DWMWCP_ROUND;
    let no_system_backdrop = 1_i32;
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR as u32,
            &border_color as *const _ as *const c_void,
            size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &corner_preference as *const _ as *const c_void,
            size_of_val(&corner_preference) as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            &no_system_backdrop as *const _ as *const c_void,
            size_of_val(&no_system_backdrop) as u32,
        );

        let user32 = LoadLibraryA(b"user32.dll\0".as_ptr());
        if !user32.is_null() {
            if let Some(procedure) =
                GetProcAddress(user32, b"SetWindowCompositionAttribute\0".as_ptr())
            {
                let set_attribute: SetWindowCompositionAttribute = std::mem::transmute(procedure);
                let mut policy = AccentPolicy {
                    state: 4,
                    flags: 0,
                    color: 0x18f7_faf4,
                    animation: 0,
                };
                let mut data = CompositionAttributeData {
                    attribute: 0x13,
                    data: &mut policy as *mut _ as *mut c_void,
                    size: size_of::<AccentPolicy>(),
                };
                let _ = set_attribute(hwnd, &mut data);
            }
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn apply_native_material_frame(_window: &WebviewWindow) -> Result<(), String> {
    Ok(())
}

fn show_main(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

#[tauri::command]
fn open_local_feishu_setup(app: AppHandle) -> Result<(), String> {
    let window = app.get_webview_window("main")
        .ok_or_else(|| "当前独立工作台没有主程序连接向导，请从主 App 打开工作台。".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())?;
    app.emit_to("main", "open-feishu-setup", ()).map_err(|error| error.to_string())
}

fn snap(window: &WebviewWindow) -> Result<(), String> {
    let monitor = window
        .current_monitor()
        .map_err(|error| error.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "无法读取显示器信息".to_string())?;
    let area = monitor.work_area();
    let size = window.outer_size().map_err(|error| error.to_string())?;
    let current = window.outer_position().map_err(|error| error.to_string())?;
    let midpoint = area.position.x + area.size.width as i32 / 2;
    let current_midpoint = current.x + size.width as i32 / 2;
    let margin = 16;
    let x = if current_midpoint < midpoint {
        area.position.x + margin
    } else {
        area.position.x + area.size.width as i32 - size.width as i32 - margin
    };
    window
        .set_position(PhysicalPosition::new(x, area.position.y + margin))
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_expanded(window: WebviewWindow, expanded: bool) -> Result<(), String> {
    let size = if expanded {
        LogicalSize::new(420.0, 700.0)
    } else {
        LogicalSize::new(220.0, 52.0)
    };
    window.set_size(size).map_err(|error| error.to_string())?;
    apply_native_material_frame(&window)?;
    snap(&window)
}

#[tauri::command]
fn hide_window(window: WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|error| error.to_string())
}

#[tauri::command]
fn list_task_sources() -> Vec<TaskSourceDescriptor> {
    task_source::available_sources()
}

#[tauri::command]
fn list_tasks(state: tauri::State<'_, AppState>) -> Result<Vec<Task>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .list_tasks()
}

#[tauri::command]
fn list_task_candidates(state: tauri::State<'_, AppState>) -> Result<Vec<Task>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .list_task_candidates(50)
}

#[tauri::command]
fn remember_task_mapping(
    state: tauri::State<'_, AppState>,
    task_id: String,
    expected_title: String,
    habit: HabitLearningInput,
) -> Result<(), String> {
    if !["alias", "correction"].contains(&habit.kind.as_str()) {
        return Err("这里只能保存已有任务的说法或纠正记录".to_string());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let task = storage
        .list_task_candidates(50)?
        .into_iter()
        .find(|task| task.id == task_id && task.title == expected_title)
        .ok_or_else(|| "所选任务已经变化；请重新分析后再记住".to_string())?;
    storage.save_habit(&habit.phrase, &task, &habit.kind)?;
    Ok(())
}

#[tauri::command]
fn list_habits(state: tauri::State<'_, AppState>) -> Result<Vec<Habit>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .list_habits()
}

#[tauri::command]
fn delete_habit(state: tauri::State<'_, AppState>, id: i64) -> Result<bool, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .delete_habit(id)
}

fn load_wps_config(storage: &Storage) -> Result<WpsConfig, String> {
    Ok(WpsConfig {
        webhook_url: storage.get_setting("wps.webhook_url")?.unwrap_or_default(),
        sheet_name: storage
            .get_setting("wps.sheet_name")?
            .unwrap_or_else(|| "任务清单".to_string()),
        token_configured: wps::read_token().is_ok(),
    })
}

#[tauri::command]
fn get_wps_config(state: tauri::State<'_, AppState>) -> Result<WpsConfig, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    load_wps_config(&storage)
}

#[tauri::command]
fn save_wps_config(
    state: tauri::State<'_, AppState>,
    input: SaveWpsConfigInput,
) -> Result<WpsConfig, String> {
    let webhook_url = wps::validate_webhook_url(&input.webhook_url)?;
    let sheet_name = input.sheet_name.trim();
    if sheet_name.is_empty() {
        return Err("请填写任务数据表名称".to_string());
    }
    if let Some(token) = input
        .token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        wps::save_token(token)?;
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.set_setting("wps.webhook_url", &webhook_url)?;
    storage.set_setting("wps.sheet_name", sheet_name)?;
    load_wps_config(&storage)
}

#[tauri::command]
fn test_wps_connection(state: tauri::State<'_, AppState>) -> Result<WpsSyncResult, String> {
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_wps_config(&storage)?
    };
    if config.webhook_url.is_empty() {
        return Err("请先保存 WPS Webhook 地址".to_string());
    }
    let token = wps::read_token()?;
    let (_, result) = wps::fetch_tasks_resolving_sheet(&config, &token, storage::unix_now())?;
    if result.sheet_name != config.sheet_name {
        state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?
            .set_setting("wps.sheet_name", &result.sheet_name)?;
    }
    Ok(result)
}

fn active_task_source(storage: &Storage) -> Result<String, String> {
    Ok(storage
        .get_setting("task_source.active")?
        .unwrap_or_else(|| "wps".to_string()))
}

#[tauri::command]
fn get_active_task_source(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    active_task_source(&storage)
}

#[tauri::command]
fn set_active_task_source(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    source: String,
) -> Result<String, String> {
    if !["wps", "feishu"].contains(&source.as_str()) {
        return Err("当前不支持这个任务数据源".to_string());
    }
    let _context_change = FeishuConnectionChange::new(app)?;
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .set_setting("task_source.active", &source)?;
    Ok(source)
}

fn load_feishu_config(storage: &Storage) -> Result<FeishuConfig, String> {
    let app_id = storage.get_setting("feishu.app_id")?.unwrap_or_default();
    let secret_configured =
        feishu::read_secret().is_ok() || feishu::user_authorized_for_app(&app_id);
    Ok(FeishuConfig {
        app_id,
        app_token: storage.get_setting("feishu.app_token")?.unwrap_or_default(),
        table_id: storage.get_setting("feishu.table_id")?.unwrap_or_default(),
        secret_configured,
    })
}

#[tauri::command]
fn get_feishu_config(state: tauri::State<'_, AppState>) -> Result<FeishuConfig, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    load_feishu_config(&storage)
}

#[tauri::command]
fn save_feishu_config(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    input: SaveFeishuConfigInput,
) -> Result<FeishuConfig, String> {
    let (app_id, app_token, table_id) = feishu::validate_config(&input)?;
    let _context_change = FeishuConnectionChange::new(app)?;
    if let Some(secret) = input
        .app_secret
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        feishu::save_secret(secret)?;
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.set_setting("feishu.app_id", &app_id)?;
    storage.set_setting("feishu.app_token", &app_token)?;
    storage.set_setting("feishu.table_id", &table_id)?;
    load_feishu_config(&storage)
}

#[tauri::command]
fn get_feishu_user_auth_status() -> FeishuUserAuthStatus {
    feishu::user_auth_status()
}

#[tauri::command]
fn start_feishu_user_authorization(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<FeishuAuthProgress, String> {
    let _context_change = FeishuConnectionChange::new(app)?;
    let mut pending = state
        .feishu_auth_flow
        .lock()
        .map_err(|_| "飞书授权流程状态异常".to_string())?;
    if pending.is_some() {
        return Err("已有飞书授权流程正在进行，请先继续或取消它。".to_string());
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let (flow, progress) = FeishuAuthFlow::start(&storage)?;
    *pending = Some(flow);
    Ok(progress)
}

#[tauri::command]
fn poll_feishu_user_authorization(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<FeishuAuthProgress, String> {
    let _context_change = FeishuConnectionChange::new(app)?;
    let mut pending = state
        .feishu_auth_flow
        .lock()
        .map_err(|_| "飞书授权流程状态异常".to_string())?;
    let Some(flow) = pending.as_mut() else {
        return Ok(FeishuAuthProgress {
            state: "idle".to_string(),
            message: "当前没有进行中的飞书授权。".to_string(),
            verification_url: None,
            user_code: None,
            retry_after_seconds: 0,
            done: true,
        });
    };
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let progress = flow.poll(&storage)?;
    if progress.done {
        *pending = None;
    }
    Ok(progress)
}

#[tauri::command]
fn cancel_feishu_user_authorization(state: tauri::State<'_, AppState>) -> Result<(), String> {
    *state
        .feishu_auth_flow
        .lock()
        .map_err(|_| "飞书授权流程状态异常".to_string())? = None;
    Ok(())
}

#[tauri::command]
fn save_feishu_user_session(
    app: AppHandle,
    input: SaveFeishuUserSessionInput,
) -> Result<FeishuUserAuthStatus, String> {
    let _context_change = FeishuConnectionChange::new(app)?;
    feishu::save_user_session(&input)?;
    Ok(feishu::user_auth_status())
}

#[tauri::command]
fn disconnect_feishu_user(app: AppHandle) -> Result<(), String> {
    write_acceptance::ensure_normal_build()?;
    let _context_change = FeishuConnectionChange::new(app)?;
    feishu::clear_user_session()
}

#[tauri::command]
fn get_write_acceptance_status(app: AppHandle) -> serde_json::Value {
    serde_json::json!({
        "enabled": write_acceptance::enabled(),
        "floatingWriteEnabled": floating_feishu::enabled(&app.config().identifier),
        "workspaceUrl": write_acceptance::workspace_app_token()
            .map(|token| format!("https://my.feishu.cn/base/{token}"))
    })
}

fn bundled_feishu_template_app_token() -> Result<String, String> {
    #[cfg(feature = "feishu-template-v2-acceptance")]
    {
        return Ok("Example00000000000000000005".to_string());
    }
    #[cfg(not(feature = "feishu-template-v2-acceptance"))]
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../resources/task-workspace-template.v1.json"))
            .map_err(|_| "内置任务模板清单无法读取".to_string())?;
    #[cfg(not(feature = "feishu-template-v2-acceptance"))]
    manifest["provisioning"]["feishuTemplateAppToken"]
        .as_str()
        .filter(|token| !token.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| "飞书个人任务模板尚未配置；当前没有开始任何云端操作".to_string())
}

#[cfg(all(test, not(feature = "feishu-template-v2-acceptance")))]
#[test]
fn public_source_bundles_the_empty_shareable_feishu_template() {
    assert_eq!(
        bundled_feishu_template_app_token().unwrap(),
        "SFXybzuvmaJaxps1RNZcRipD6Ed"
    );
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../resources/task-workspace-template.v1.json"))
            .expect("template manifest");
    assert_eq!(manifest["templateVersion"], "2.0.0");
    assert_eq!(manifest["sampleRecords"], serde_json::json!([]));
    assert!(!write_acceptance::enabled());
}

#[tauri::command]
fn get_task_workspace_setup_status(
    state: tauri::State<'_, AppState>,
) -> Result<TaskWorkspaceSetupStatus, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let checkpoint = workspace_provisioning::get_persisted_checkpoint(&storage)?;
    let app_id = storage.get_setting("feishu.app_id")?.unwrap_or_default();
    let user_authorized = feishu::user_authorized_for_app(&app_id);
    Ok(match checkpoint {
        Some(checkpoint) => TaskWorkspaceSetupStatus {
            template_available: bundled_feishu_template_app_token().is_ok(),
            user_authorized,
            has_pending_operation: true,
            phase: Some(checkpoint.phase),
            needs_reconciliation: checkpoint.needs_reconciliation,
            remote_mutation_started: checkpoint.remote_mutation_started,
            workspace_name: checkpoint
                .workspace
                .as_ref()
                .map(|workspace| workspace.name.clone()),
            workspace_url: checkpoint.workspace.and_then(|workspace| workspace.url),
        },
        None => TaskWorkspaceSetupStatus {
            template_available: bundled_feishu_template_app_token().is_ok(),
            user_authorized,
            has_pending_operation: false,
            phase: None,
            needs_reconciliation: false,
            remote_mutation_started: false,
            workspace_name: None,
            workspace_url: storage.get_setting("feishu.workspace_url")?,
        },
    })
}

#[tauri::command]
fn start_feishu_task_workspace_setup(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    workspace_name: String,
    folder_link: Option<String>,
) -> Result<TaskWorkspaceProvisionResponse, String> {
    write_acceptance::ensure_normal_build()?;
    let _context_change = FeishuConnectionChange::new(app)?;
    let workspace_name = workspace_name.trim();
    if workspace_name.is_empty() || workspace_name.chars().count() > 100 {
        return Err("任务空间名称不能为空，且不能超过 100 个字符".to_string());
    }
    let folder_token = folder_link
        .as_deref()
        .map(str::trim)
        .filter(|link| !link.is_empty())
        .map(feishu::folder_token_from_link)
        .transpose()?;
    let template_app_token = bundled_feishu_template_app_token()?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    if storage.get_setting("task_source.active")?.as_deref() == Some("feishu")
        && storage
            .get_setting("feishu.app_token")?
            .is_some_and(|token| !token.trim().is_empty())
    {
        return Err(
            "当前已连接飞书任务空间；为避免重复创建，请先在设置中检查现有连接。".to_string(),
        );
    }
    let mut adapter =
        FeishuWorkspaceAdapter::from_saved_user_session(template_app_token, &storage)?;
    static NEXT_SETUP_OPERATION: AtomicU64 = AtomicU64::new(1);
    let request = ProvisionRequest {
        operation_id: format!(
            "feishu-{}-{}-{}",
            storage::unix_now(),
            std::process::id(),
            NEXT_SETUP_OPERATION.fetch_add(1, Ordering::Relaxed)
        ),
        platform: WorkspacePlatform::Feishu,
        mode: ProvisionMode::CreateNew,
        workspace_name: workspace_name.to_string(),
        workspace_location: folder_token,
    };
    Ok(
        workspace_provisioning::provision_workspace_persisted(&mut adapter, &storage, request)
            .into(),
    )
}

#[tauri::command]
fn resume_feishu_task_workspace_setup(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<Option<TaskWorkspaceProvisionResponse>, String> {
    write_acceptance::ensure_normal_build()?;
    let _context_change = FeishuConnectionChange::new(app)?;
    let template_app_token = bundled_feishu_template_app_token()?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    if workspace_provisioning::get_persisted_checkpoint(&storage)?.is_none() {
        return Ok(None);
    }
    let mut adapter =
        FeishuWorkspaceAdapter::from_saved_user_session(template_app_token, &storage)?;
    workspace_provisioning::resume_persisted_workspace(&mut adapter, &storage)
        .map(|result| result.map(Into::into))
}

#[tauri::command]
fn reconcile_feishu_task_workspace_setup(
    state: tauri::State<'_, AppState>,
    remote_workspace_exists: bool,
    app_token: Option<String>,
    workspace_url: Option<String>,
) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let workspace = if remote_workspace_exists {
        let checkpoint = workspace_provisioning::get_persisted_checkpoint(&storage)?
            .ok_or_else(|| "没有待核对的任务空间初始化记录".to_string())?;
        let app_token = app_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .ok_or_else(|| "请提供在飞书确认找到的副本链接或 App Token".to_string())?;
        Some(WorkspaceReference::new(
            checkpoint.request.workspace_name,
            workspace_url,
            app_token.to_string(),
        ))
    } else {
        None
    };
    workspace_provisioning::resolve_persisted_uncertain_creation(&storage, workspace)
}

fn saved_feishu_credentials(state: &AppState) -> Result<(FeishuConfig, String), String> {
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_feishu_config(&storage)?
    };
    if config.app_id.is_empty() || config.app_token.is_empty() || config.table_id.is_empty() {
        return Err("请先保存飞书连接信息".to_string());
    }
    let access_token = feishu::read_active_access_token(&config)?;
    Ok((config, access_token))
}

/// Manual read-only inspection for the formal local-workbench window only.
/// Never falls back from an expired/missing user account to app identity.
fn local_feishu_target(storage: &Storage) -> Result<FeishuConfig, String> {
    // Local metadata only: opening the panel must not read secrets or refresh tokens.
    Ok(FeishuConfig {
        app_id: storage.get_setting("feishu.app_id")?.unwrap_or_default(),
        app_token: storage.get_setting("feishu.app_token")?.unwrap_or_default(),
        table_id: storage.get_setting("feishu.table_id")?.unwrap_or_default(),
        secret_configured: false,
    })
}
// The in-memory context revision also fences local commits against concurrent
// authorization/target changes. Notifications carry no account data or authority.
struct FeishuConnectionChange(AppHandle);
impl FeishuConnectionChange {
    fn new(app: AppHandle) -> Result<Self,String> {
        app.state::<local_workbench::LocalWorkspaceState>().begin_feishu_change()?;
        let _ = app.emit("feishu-connection-changed", ());
        Ok(Self(app))
    }
}
impl Drop for FeishuConnectionChange {
    fn drop(&mut self) {
        self.0.state::<local_workbench::LocalWorkspaceState>().end_feishu_change();
        let _ = self.0.emit("feishu-connection-changed", ());
    }
}
fn local_feishu_target_key(config: &FeishuConfig) -> String {
    serde_json::json!([config.app_id, config.app_token, config.table_id]).to_string()
}
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
struct LocalFeishuAccessStatus {
    target_key: String,
    app_id: String,
    app_token: String,
    table_id: String,
    configured: bool,
    enabled: bool,
    write_enabled: bool,
    account_verified: bool,
}
fn local_feishu_access_status(config: FeishuConfig, local: &local_workbench::LocalWorkspaceState) -> Result<LocalFeishuAccessStatus,String> {
    let configured = !config.app_id.is_empty() && !config.app_token.is_empty() && !config.table_id.is_empty();
    let target_key = local_feishu_target_key(&config);
    let enabled = configured && local.cloud_access.enabled(&target_key)?;
    // Process capability for individually confirmed uploads, not proof of platform permission.
    Ok(LocalFeishuAccessStatus { target_key, app_id:config.app_id, app_token:config.app_token, table_id:config.table_id, configured, enabled, write_enabled:configured&&enabled, account_verified:false })
}
#[tauri::command]
async fn get_local_feishu_access(window: WebviewWindow, app: AppHandle) -> Result<LocalFeishuAccessStatus,String> {
    if window.label()!="local-workbench" { return Err("请从本地工作台查看连接".into()); }
    tauri::async_runtime::spawn_blocking(move|| {
        let state=app.state::<AppState>();
        let local=app.state::<local_workbench::LocalWorkspaceState>();
        let config=local_feishu_target(&*state.storage.lock().map_err(|_|"本地设置繁忙")?)?;
        local_feishu_access_status(config,&local)
    }).await.map_err(|_|"本地连接检查未完成".to_string())?
}
#[tauri::command]
async fn set_local_feishu_access(window: WebviewWindow, app: AppHandle, target_key:String, enabled:bool) -> Result<LocalFeishuAccessStatus,String> {
    if window.label()!="local-workbench" { return Err("请从本地工作台设置云端访问".into()); }
    tauri::async_runtime::spawn_blocking(move|| {
        let state=app.state::<AppState>();
        let local=app.state::<local_workbench::LocalWorkspaceState>();
        let config=local_feishu_target(&*state.storage.lock().map_err(|_|"本地设置繁忙")?)?;
        if local_feishu_target_key(&config)!=target_key { return Err("飞书目标已变化，请重新查看连接后确认".into()); }
        if enabled && (config.app_id.is_empty()||config.app_token.is_empty()||config.table_id.is_empty()) { return Err("请先在设置中保存飞书连接；仍可离线使用".into()); }
        local.cloud_access.set_enabled(&target_key,enabled)?;
        local_feishu_access_status(config,&local)
    }).await.map_err(|_|"云端访问设置未完成".to_string())?
}
fn with_local_feishu_inspection<T>(app:&AppHandle,target_key:&str,expected_context:Option<u64>,
    action:impl FnOnce(&local_workbench::LocalWorkspaceState,&FeishuConfig,feishu::SyncInspection,u64,&str)->Result<T,String>) -> Result<T,String> {
        let state=app.state::<AppState>();
        let local=app.state::<local_workbench::LocalWorkspaceState>();
        let config=local_feishu_target(&*state.storage.lock().map_err(|_|"本地设置繁忙")?)?;
        if local_feishu_target_key(&config)!=target_key { return Err("飞书目标已变化，请重新打开同步面板核对".into()); }
        local.cloud_access.read(&local_feishu_target_key(&config),|| {
        let version=local.feishu_context_version()?;
        if expected_context.is_some_and(|expected|expected!=version) {return Err("飞书连接或授权已变化，旧确认已失效".into());}
        #[cfg(feature="local-main-flow-ui-acceptance")]
        let (token,inspection) = (String::new(),local_main_flow_lab::synthetic_read(&state)?);
        #[cfg(not(feature="local-main-flow-ui-acceptance"))]
        let (token,inspection) = {
        if !feishu::user_authorized_for_app(&config.app_id) {return Err("请先在设置中连接对应的飞书用户账号；本检查不使用应用身份代替用户身份".into());}
        let token=feishu::read_user_access_token(&config.app_id)?;
        let now=storage::unix_now().checked_mul(1000).ok_or("检查时间无效")?;
        let inspection=feishu::inspect_sync_with_user_token(&config,&token,now)?;
        (token,inspection)
        };
        let current=local_feishu_target(&*state.storage.lock().map_err(|_|"本地设置繁忙")?)?;
        if local_feishu_target_key(&current)!=target_key { return Err("飞书目标在检查期间变化，旧结果已作废，请重新检查".into()); }
        action(&local,&config,inspection,version,&token)
        })
}
#[tauri::command]
async fn inspect_local_feishu_sync(window: WebviewWindow, app: AppHandle, target_key:String) -> Result<local_workbench::FormalPairingPreview,String> {
    if window.label()!="local-workbench" {return Err("请从本地任务工作台发起只读检查".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,None,|local,config,inspection,version,_|local.formal_feishu_preview(config,inspection,version))
    }).await.map_err(|_|"飞书只读检查未完成，请重试".to_string())?
}
#[tauri::command]
async fn save_local_feishu_pairing(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalPairingInput) -> Result<local_workbench::SavePairingResult,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台人工确认任务对应关系".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,inspection,_,_|local.save_feishu_pairing(config,inspection,&input))
    }).await.map_err(|_|"保存对应关系未完成，请重新检查；未修改任务内容".to_string())?
}
#[tauri::command]
async fn download_local_feishu_task(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalDownloadInput) -> Result<local_workbench::Snapshot,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台人工确认下载任务".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,inspection,_,_|local.download_feishu_task(config,inspection,&input))
    }).await.map_err(|_|"本地下载结果未确认，请刷新任务核对；不会自动重试".to_string())?
}
#[tauri::command]
async fn import_local_feishu_task(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalImportInput) -> Result<local_workbench::Snapshot,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台人工确认导入任务".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,inspection,_,_|local.import_feishu_task(config,inspection,&input))
    }).await.map_err(|_|"飞书导入结果未确认，请刷新本地任务核对；不会自动重试".to_string())?
}
#[tauri::command]
async fn import_local_feishu_tasks(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalBatchImportInput) -> Result<local_workbench::Snapshot,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台人工确认批量导入".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,inspection,_,_|local.import_feishu_tasks(config,inspection,&input))
    }).await.map_err(|_|"批量导入结果未确认，请刷新本地任务核对；不会自动重试".to_string())?
}

#[tauri::command]
async fn upload_local_feishu_task(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalUploadInput)->Result<local_workbench::UploadSummary,String> {
    if window.label()!="local-workbench"||!input.confirm_overwrite {return Err("请从本地工作台明确确认单条上传".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,inspection,version,token|{
            let op=local.begin_feishu_upload(config,&inspection,&input)?;
            let prepared=match feishu::prepare_user_sync_update(config,token,&inspection,&op.record_id,&op.content,&op.status) {
                Ok(prepared)=>prepared,
                Err(reason)=>{local.cancel_feishu_unsent(&op.id)?;return Err(format!("尚未发出写入：{reason}"));}
            };
            local.dispatch_feishu_upload(version,&op.id,||feishu::send_user_sync_update(prepared))?;
            // A send error is ambiguous. Always read back once; never retry the mutation.
            let read=storage::unix_now().checked_mul(1000).and_then(|now|feishu::inspect_sync_with_user_token(config,token,now).ok());
            local.finish_feishu_upload(&op.id,read.as_ref())
        })
    }).await.map_err(|_|"上传结果未确认；请手动检查待核实记录，不要重发".to_string())?
}
#[tauri::command]
async fn reconcile_local_feishu_upload(window:WebviewWindow,app:AppHandle,target_key:String,context_version:u64,operation_id:String)->Result<local_workbench::UploadSummary,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台发起只读核实".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(context_version),|local,config,inspection,version,_|local.reconcile_feishu_upload(config,inspection,version,&operation_id))
    }).await.map_err(|_|"只读核实未完成；没有重新发送写入".to_string())?
}
#[tauri::command]
async fn restore_local_feishu_upload(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalRestoreInput)->Result<local_workbench::UploadSummary,String> {
    if window.label()!="local-workbench"||!input.confirm_restore {return Err("请从本地工作台预览并明确确认恢复".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,inspection,version,token|{
            let op=local.begin_feishu_restore(config,&inspection,&input)?;
            let prepared=match feishu::prepare_user_sync_restore(config,token,&inspection,&op.record_id,&op.content,&op.status) {
                Ok(prepared)=>prepared,
                Err(reason)=>{local.cancel_feishu_unsent(&op.id)?;return Err(format!("尚未发出恢复：{reason}"));}
            };
            local.dispatch_feishu_upload(version,&op.id,||feishu::send_user_sync_update(prepared))?;
            let read=storage::unix_now().checked_mul(1000).and_then(|now|feishu::inspect_sync_with_user_token(config,token,now).ok());
            local.finish_feishu_upload(&op.id,read.as_ref())
        })
    }).await.map_err(|_|"恢复结果未确认；请手动核实，不要重复恢复".to_string())?
}

#[tauri::command]
async fn prepare_local_feishu_batch(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalBatchInput)->Result<local_workbench::FormalBatchRun,String> {
    if window.label()!="local-workbench"||!input.confirm_batch {return Err("请从本地工作台预览并明确确认批次".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,read,_,_|local.prepare_feishu_batch(config,&read,&input))
    }).await.map_err(|_|"批次准备结果未确认；请手动查看保存的计划，不会自动执行".to_string())?
}
#[tauri::command]
async fn start_local_feishu_batch(window:WebviewWindow,app:AppHandle,target_key:String,context_version:u64,batch_id:String,confirm_continue:bool)->Result<local_workbench::FormalBatchRun,String> {
    if window.label()!="local-workbench"||!confirm_continue {return Err("请核对保存的批次并确认人工继续".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(context_version),|local,config,read,version,_|local.start_feishu_batch(config,&read,version,&batch_id,confirm_continue))
    }).await.map_err(|_|"批次继续许可未确认；请重新检查，没有自动执行".to_string())?
}
#[tauri::command]
async fn stop_local_feishu_batch(window:WebviewWindow,app:AppHandle,target_key:String,batch_id:String)->Result<(),String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台停止批次".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        let state=app.state::<AppState>();
        let config=local_feishu_target(&*state.storage.lock().map_err(|_|"本地设置繁忙")?)?;
        if local_feishu_target_key(&config)!=target_key {return Err("目标已变更；旧运行许可已作废，请重新检查".into());}
        app.state::<local_workbench::LocalWorkspaceState>().stop_feishu_batch(&config,&batch_id)
    }).await.map_err(|_|"停止状态未确认；当前条目仍需核实，请重新查看批次".to_string())?
}
#[tauri::command]
async fn step_local_feishu_batch(window:WebviewWindow,app:AppHandle,target_key:String,input:local_workbench::FormalBatchAction)->Result<local_workbench::BatchSummary,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台逐条执行已确认批次".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(input.context_version),|local,config,read,version,token| {
            let op=match local.step_feishu_batch(config,&read,&input)? {
                local_workbench::FormalBatchStep::Settled(batch)=>return Ok(batch),
                local_workbench::FormalBatchStep::Upload(op)=>op,
            };
            // Exactly the R3 user-scoped adapter: durable claim + intent precede one send.
            let prepared=match feishu::prepare_user_sync_update(config,token,&read,&op.record_id,&op.content,&op.status) {
                Ok(prepared)=>prepared,
                Err(_)=>{
                    local.cancel_feishu_unsent(&op.id)?;
                    return local.finish_feishu_batch_upload(&input.batch_id,&op.id,None);
                }
            };
            if local.dispatch_feishu_upload(version,&op.id,||feishu::send_user_sync_update(prepared)).is_err() {
                return local.finish_feishu_batch_upload(&input.batch_id,&op.id,None);
            }
            let readback=storage::unix_now().checked_mul(1000).and_then(|now|feishu::inspect_sync_with_user_token(config,token,now).ok());
            local.finish_feishu_batch_upload(&input.batch_id,&op.id,readback.as_ref())
        })
    }).await.map_err(|_|"批次结果未确认；请只读核实，不能自动继续或重发".to_string())?
}
#[tauri::command]
async fn reconcile_local_feishu_batch(window:WebviewWindow,app:AppHandle,target_key:String,context_version:u64,batch_id:String)->Result<local_workbench::BatchSummary,String> {
    if window.label()!="local-workbench" {return Err("请从本地工作台只读核实批次".into());}
    tauri::async_runtime::spawn_blocking(move|| {
        with_local_feishu_inspection(&app,&target_key,Some(context_version),|local,config,read,version,_|local.reconcile_feishu_batch(config,&read,version,&batch_id))
    }).await.map_err(|_|"批次核实未完成，没有重发，也不会自动继续".to_string())?
}

#[tauri::command]
fn save_feishu_template() -> Result<TemplateSaveResult, String> {
    let Some(mut path) = rfd::FileDialog::new()
        .set_title("保存飞书字段导入模板")
        .set_file_name("飞书字段导入模板.xlsx")
        .add_filter("Excel 工作簿", &["xlsx"])
        .save_file()
    else {
        return Ok(TemplateSaveResult {
            saved: false,
            file_name: None,
        });
    };
    if path.extension().and_then(|value| value.to_str()) != Some("xlsx") {
        path.set_extension("xlsx");
    }
    fs::write(
        &path,
        include_bytes!("../resources/feishu-field-import-template.xlsx"),
    )
    .map_err(|error| format!("无法保存飞书字段导入模板：{error}"))?;
    Ok(TemplateSaveResult {
        saved: true,
        file_name: path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_string),
    })
}

#[tauri::command]
fn inspect_feishu_schema(state: tauri::State<'_, AppState>) -> Result<FeishuSchemaPlan, String> {
    let (config, access_token) = saved_feishu_credentials(&state)?;
    feishu::inspect_schema_with_access_token(&config, &access_token)
}

#[tauri::command]
fn initialize_feishu_schema(
    state: tauri::State<'_, AppState>,
) -> Result<FeishuSchemaInitResult, String> {
    write_acceptance::ensure_normal_build()?;
    let (config, access_token) = saved_feishu_credentials(&state)?;
    feishu::create_missing_required_fields_with_access_token(&config, &access_token)
}

#[tauri::command]
fn inspect_feishu_workspace(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let local = app.state::<local_workbench::LocalWorkspaceState>();
    let version = local.feishu_context_version()?;
    let (config, access_token) = saved_feishu_credentials(&state)?;
    let status = feishu::inspect_workspace_with_access_token(&config, &access_token)?;
    local.with_feishu_context(version, || {
        let s = state.storage.lock().map_err(|_| "本地设置繁忙")?;
        if local_feishu_target_key(&local_feishu_target(&s)?) != local_feishu_target_key(&config) {
            return Err("连接已变化，请重新检查任务空间".into());
        }
        Ok(())
    })?;
    let mut result = serde_json::to_value(status).map_err(|e| e.to_string())?;
    result["targetKey"] = serde_json::json!(local_feishu_target_key(&config));
    result["contextVersion"] = serde_json::json!(version);
    Ok(result)
}

#[tauri::command]
async fn initialize_feishu_workspace(
    window: WebviewWindow,
    app: AppHandle,
    expected_target_key: String,
    context_version: u64,
) -> Result<FeishuWorkspaceInitResult, String> {
    write_acceptance::ensure_normal_build()?;
    if window.label() != "main" { return Err("请从浮窗设置检查并确认字段补齐".into()); }
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _guard = state.write_lock.try_lock().map_err(|_| "有写入正在处理中，请稍后再试")?;
        let (config, access_token) = saved_feishu_credentials(&state)?;
        if local_feishu_target_key(&config) != expected_target_key { return Err("连接已变化，旧字段补齐确认失效".into()); }
        app.state::<local_workbench::LocalWorkspaceState>().with_feishu_context(context_version, || {
            let s = state.storage.lock().map_err(|_| "本地设置繁忙")?;
            if local_feishu_target_key(&local_feishu_target(&s)?) != expected_target_key { return Err("目标表已变化，未补齐字段".into()); }
            drop(s);
            feishu::initialize_workspace_with_access_token(&config, &access_token)
        })
    }).await.map_err(|_| "字段补齐结果未确认，请重新只读检查，不要直接重复创建".to_string())?
}

#[tauri::command]
fn test_feishu_connection(state: tauri::State<'_, AppState>) -> Result<FeishuSyncResult, String> {
    let (config, access_token) = saved_feishu_credentials(&state)?;
    let (_, result) =
        feishu::fetch_tasks_with_access_token(&config, &access_token, storage::unix_now())?;
    Ok(result)
}

fn perform_wps_sync(state: &AppState) -> Result<WpsSyncResult, String> {
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_wps_config(&storage)?
    };
    if config.webhook_url.is_empty() || !config.token_configured {
        return Err("WPS 尚未配置".to_string());
    }
    let token = wps::read_token()?;
    let (tasks, result) = wps::fetch_tasks_resolving_sheet(&config, &token, storage::unix_now())?;
    let mut sync_storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let current = load_wps_config(&sync_storage)?;
    if current.webhook_url != config.webhook_url || current.sheet_name != config.sheet_name {
        return Err("同步期间 WPS 连接已变化，未替换任务缓存；请在当前连接下重新同步".to_string());
    }
    if result.sheet_name != config.sheet_name {
        sync_storage.set_setting("wps.sheet_name", &result.sheet_name)?;
    }
    sync_storage.replace_wps_tasks(&tasks)?;
    Ok(result)
}

fn perform_feishu_sync(state: &AppState) -> Result<FeishuSyncResult, String> {
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_feishu_config(&storage)?
    };
    if config.app_id.is_empty() || config.app_token.is_empty() || config.table_id.is_empty() {
        return Err("飞书尚未配置".to_string());
    }
    let access_token = feishu::read_active_access_token(&config)?;
    let (tasks, result) =
        feishu::fetch_tasks_with_access_token(&config, &access_token, storage::unix_now())?;
    let mut storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    let current = load_feishu_config(&storage)?;
    if active_task_source(&storage)? != "feishu"
        || current.app_id != config.app_id
        || current.app_token != config.app_token
        || current.table_id != config.table_id
    {
        return Err("同步期间飞书连接已变化，未替换任务缓存；请在当前连接下重新同步".to_string());
    }
    storage.replace_remote_tasks(&tasks, "feishu")?;
    Ok(result)
}

fn perform_active_sync(state: &AppState) -> Result<TaskSyncResult, String> {
    let source = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        active_task_source(&storage)?
    };
    match source.as_str() {
        "feishu" => perform_feishu_sync(state).map(Into::into),
        _ => perform_wps_sync(state).map(Into::into),
    }
}

fn pending_write_summary(state: &AppState) -> Result<PendingWriteSummary, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .pending_write_summary()
}

fn mark_pending_write(
    state: &AppState,
    id: i64,
    queue_state: &str,
    error: &str,
) -> Result<(), String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .set_pending_write_state(id, queue_state, error)
}

fn flush_pending_writes_inner(state: &AppState) -> Result<PendingWriteSummary, String> {
    let source = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        active_task_source(&storage)?
    };
    if cfg!(feature = "feishu-write-acceptance") || source != "wps" {
        return pending_write_summary(state);
    }
    let queued = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .list_pending_writes()?;
    if queued.is_empty() {
        return pending_write_summary(state);
    }
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_wps_config(&storage)?
    };
    if config.webhook_url.is_empty() || !config.token_configured {
        return pending_write_summary(state);
    }
    let token = wps::read_token()?;
    for queued_write in queued {
        let (before, connection) = match wps::fetch_tasks(&config, &token, storage::unix_now()) {
            Ok(result) => result,
            Err(error) => {
                mark_pending_write(state, queued_write.id, "pending", &error)?;
                break;
            }
        };
        if !connection.write_ready {
            mark_pending_write(
                state,
                queued_write.id,
                "pending",
                "AirScript 尚未提供安全写入能力",
            )?;
            break;
        }
        let previous = queued_write
            .input
            .task_id
            .as_ref()
            .and_then(|task_id| before.iter().find(|task| &task.id == task_id))
            .cloned();
        if queued_write.input.task_id.is_some() {
            let Some(current) = previous.as_ref() else {
                mark_pending_write(state, queued_write.id, "conflict", "目标任务已不在 WPS 中")?;
                continue;
            };
            if remote_task_changed(&queued_write.input, current) {
                mark_pending_write(
                    &state,
                    queued_write.id,
                    "conflict",
                    "排队期间 WPS 任务的进度或状态已变化",
                )?;
                continue;
            }
        }
        if let Err(error) = validate_confirm_input(&queued_write.input) {
            mark_pending_write(state, queued_write.id, "conflict", &error)?;
            continue;
        }
        let request = match build_write_request(&queued_write.input) {
            Ok(request) => request,
            Err(error) => {
                mark_pending_write(state, queued_write.id, "conflict", &error)?;
                continue;
            }
        };
        if let Err(error) = wps::write_task(&config, &token, &request) {
            mark_pending_write(
                state,
                queued_write.id,
                "uncertain",
                &format!("写入请求可能已送达：{error}"),
            )?;
            continue;
        }
        let (after, _) = match wps::fetch_tasks(&config, &token, storage::unix_now()) {
            Ok(result) => result,
            Err(error) => {
                mark_pending_write(
                    state,
                    queued_write.id,
                    "uncertain",
                    &format!("写入后无法回读：{error}"),
                )?;
                continue;
            }
        };
        let Some(verified) = verified_written_task(&before, &after, &queued_write.input) else {
            mark_pending_write(
                state,
                queued_write.id,
                "uncertain",
                "写入后的 WPS 值与排队内容不一致",
            )?;
            continue;
        };
        let habit = queued_write
            .habit_json
            .as_deref()
            .map(serde_json::from_str::<HabitLearningInput>)
            .transpose();
        let mut storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        storage.replace_wps_tasks(&after)?;
        storage.record_synced_wps_write(&queued_write.input, previous.as_ref(), &verified)?;
        if let Ok(Some(habit)) = habit {
            let _ = if habit.kind == "task_type" {
                storage.save_task_type_habit(&habit.phrase, &verified.title, &verified.quadrant)
            } else {
                storage.save_habit(&habit.phrase, &verified, &habit.kind)
            };
        }
        storage.delete_pending_write(queued_write.id)?;
    }
    pending_write_summary(state)
}

#[tauri::command]
async fn sync_wps_tasks(app: AppHandle) -> Result<WpsSyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _write_guard = state
            .write_lock
            .try_lock()
            .map_err(|_| "正在处理另一笔写入，请稍后同步".to_string())?;
        let result = perform_wps_sync(&state)?;
        let _ = flush_pending_writes_inner(&state)?;
        Ok(result)
    })
    .await
    .map_err(|_| "同步处理意外中断，请先核对云端与待同步记录，避免重复写入".to_string())?
}

#[tauri::command]
async fn sync_active_tasks(app: AppHandle) -> Result<TaskSyncResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        let _write_guard = state
            .write_lock
            .try_lock()
            .map_err(|_| "正在处理另一笔写入，请稍后同步".to_string())?;
        let result = perform_active_sync(&state)?;
        if result.source == "wps" {
            let _ = flush_pending_writes_inner(&state)?;
        }
        Ok(result)
    })
    .await
    .map_err(|_| "同步处理意外中断，请先核对云端与待同步记录，避免重复写入".to_string())?
}

#[tauri::command]
fn get_pending_write_summary(
    state: tauri::State<'_, AppState>,
) -> Result<PendingWriteSummary, String> {
    pending_write_summary(&state)
}

#[tauri::command]
fn list_pending_write_items(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<storage::PendingWriteInfo>, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .list_pending_write_items()
}

#[tauri::command]
fn prepare_pending_write(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<ConfirmInput, String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .prepare_pending_write(id)
}

#[tauri::command]
fn discard_pending_write(state: tauri::State<'_, AppState>, id: i64) -> Result<(), String> {
    state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .delete_pending_write(id)
}

fn load_ai_config(storage: &Storage) -> Result<AiConfig, String> {
    let endpoint = storage.get_setting("ai.endpoint")?.unwrap_or_default();
    let provider = storage.get_setting("ai.provider")?.unwrap_or_else(|| {
        if endpoint.is_empty() {
            "deepseek".to_string()
        } else {
            "generic".to_string()
        }
    });
    Ok(AiConfig {
        provider,
        endpoint,
        model: storage.get_setting("ai.model")?.unwrap_or_default(),
        enabled: storage
            .get_setting("ai.enabled")?
            .is_some_and(|value| value == "true"),
        api_key_configured: ai::read_api_key().is_ok(),
    })
}

#[tauri::command]
fn get_ai_config(state: tauri::State<'_, AppState>) -> Result<AiConfig, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    load_ai_config(&storage)
}

#[tauri::command]
fn save_ai_config(
    state: tauri::State<'_, AppState>,
    input: SaveAiConfigInput,
) -> Result<AiConfig, String> {
    let endpoint = ai::provider_endpoint(&input.provider, &input.endpoint)?;
    let model = input.model.trim();
    if model.is_empty() {
        return Err("请填写模型名称".to_string());
    }
    if let Some(api_key) = input
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        ai::save_api_key(api_key)?;
    }
    let storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.set_setting("ai.provider", &input.provider)?;
    storage.set_setting("ai.endpoint", &endpoint)?;
    storage.set_setting("ai.model", model)?;
    storage.set_setting("ai.enabled", if input.enabled { "true" } else { "false" })?;
    load_ai_config(&storage)
}

#[tauri::command]
fn list_ai_models(input: ListModelsInput) -> Result<ModelListResult, String> {
    ai::list_models(&input)
}

#[tauri::command]
fn test_ai_connection(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_ai_config(&storage)?
    };
    if config.endpoint.is_empty() || config.model.is_empty() {
        return Err("请先保存模型接口地址和模型名称".to_string());
    }
    let api_key = ai::read_api_key()?;
    ai::test_connection(&config, &api_key)
}

#[tauri::command]
fn analyze_update(
    state: tauri::State<'_, AppState>,
    description: String,
    progress: u8,
    intent: Option<String>,
) -> Result<AnalyzeResponse, String> {
    let intent = intent.unwrap_or_else(|| "auto".to_string());
    let (local_result, config) = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        (
            storage.analyze_with_intent(&description, progress, &intent)?,
            load_ai_config(&storage)?,
        )
    };
    let learned_task_type = matches!(
        &local_result,
        AnalysisResult::Temporary {
            analysis_source,
            ..
        } if analysis_source == "habit"
    );
    if description.contains("临时任务")
        || learned_task_type
        || matches!(local_result, AnalysisResult::Match { .. })
    {
        return Ok(AnalyzeResponse {
            analysis: local_result,
            retrieval: None,
        });
    }
    if !config.enabled
        || config.endpoint.is_empty()
        || config.model.is_empty()
        || !config.api_key_configured
    {
        return Ok(AnalyzeResponse {
            analysis: local_result,
            retrieval: None,
        });
    }
    let context = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .retrieve_ai_context(&description)?;
    let api_key = ai::read_api_key()?;
    let analysis = ai::analyze(
        &config,
        &api_key,
        &description,
        progress,
        &context.tasks,
        &context.habits,
        &intent,
    )?;
    let analysis = match (&*intent, analysis) {
        ("create", value @ AnalysisResult::Temporary { .. }) => value,
        ("create", _) => local_result,
        ("update", value @ (AnalysisResult::Match { .. } | AnalysisResult::Ambiguous { .. })) => {
            value
        }
        ("update", _) => local_result,
        (_, analysis) => analysis,
    };
    Ok(AnalyzeResponse {
        analysis,
        retrieval: Some(context.stats),
    })
}

fn confirm_feishu_update(
    state: &AppState,
    input: ConfirmInput,
    habit: Option<HabitLearningInput>,
    replaces_queue_id: Option<i64>,
) -> Result<ConfirmResult, String> {
    confirm_feishu_update_with_io(
        state, input, habit, replaces_queue_id,
        feishu::write_task_with_access_token,
        |config, token| feishu::fetch_tasks_with_access_token(config, token, storage::unix_now()),
    )
}

// The normal entry always supplies real I/O. Tests can fail the post-delivery read
// without adding fault switches or alternative API endpoints to the shipped app.
fn confirm_feishu_update_with_io<W, R>(
    state: &AppState,
    input: ConfirmInput,
    habit: Option<HabitLearningInput>,
    replaces_queue_id: Option<i64>,
    write: W,
    readback: R,
) -> Result<ConfirmResult, String>
where
    W: FnOnce(&FeishuConfig, &str, &FeishuWriteRequest) -> Result<Option<String>, String>,
    R: FnOnce(&FeishuConfig, &str) -> Result<(Vec<Task>, FeishuSyncResult), String>,
{
    if replaces_queue_id.is_some() {
        return Err("待同步队列属于 WPS，不能在飞书数据源中继续提交".to_string());
    }
    let (config, access_token) = saved_feishu_credentials(state)?;
    write_acceptance::ensure_task_write(&config)?;
    let (before, connection) =
        feishu::fetch_tasks_with_access_token(&config, &access_token, storage::unix_now())?;
    if !connection.write_ready {
        return Err("当前飞书连接尚未通过写入能力校验".to_string());
    }
    let previous = input
        .task_id
        .as_ref()
        .and_then(|task_id| before.iter().find(|task| &task.id == task_id))
        .cloned();
    if input.task_id.is_some() {
        let current = previous
            .as_ref()
            .ok_or_else(|| "目标任务已不在飞书中；请同步后重新分析".to_string())?;
        if remote_task_changed(&input, current) {
            if input
                .expected_title
                .as_deref()
                .is_some_and(|expected| expected != current.title)
            {
                return Err(format!(
                    "检测到飞书任务标识对应关系已变化：分析时选择「{}」，当前同一标识指向「{}」。为避免写错任务，本次没有写入；请按最新数据重新分析",
                    input.expected_title.as_deref().unwrap_or("未知任务"),
                    current.title
                ));
            }
            return Err(feishu_conflict_message(&input, current));
        }
    }
    let request = build_feishu_write_request(&input)?;
    let write_error = write(&config, &access_token, &request).err();
    let after_result = readback(&config, &access_token);
    let (after, _) = match after_result {
        Ok(result) => result,
        Err(read_error) => {
            if let Some(previous) = previous.as_ref() {
                state
                    .storage
                    .lock()
                    .map_err(|_| "本地数据锁异常".to_string())?
                    .record_uncertain_remote_write(&input, previous, None, "feishu")?;
                return Err(format!(
                    "飞书写入请求已发送，但回读失败：{read_error}。已保留核对与撤销记录，请勿重复提交"
                ));
            }
            return Err(format!(
                "飞书新增请求已发送，但回读失败：{read_error}。请先检查飞书表格并重新同步，不要重复提交"
            ));
        }
    };
    let verified = match verified_written_task(&before, &after, &input) {
        Some(task) => task,
        None => {
            let observed = input
                .task_id
                .as_ref()
                .and_then(|task_id| after.iter().find(|task| &task.id == task_id));
            if let Some(previous) = previous.as_ref() {
                state
                    .storage
                    .lock()
                    .map_err(|_| "本地数据锁异常".to_string())?
                    .record_uncertain_remote_write(&input, previous, observed, "feishu")?;
                state
                    .storage
                    .lock()
                    .map_err(|_| "本地数据锁异常".to_string())?
                    .replace_remote_tasks(&after, "feishu")?;
                let detail = write_error
                    .as_deref()
                    .map(|error| format!("：{error}"))
                    .unwrap_or_default();
                return Err(format!(
                    "飞书写入后回读值与本次确认不一致{detail}。已保留核对与撤销记录，请勿重复提交"
                ));
            }
            let detail = write_error
                .as_deref()
                .map(|error| format!("：{error}"))
                .unwrap_or_default();
            return Err(format!(
                "飞书新增后没有找到可确认的新记录{detail}。请先检查飞书表格并重新同步，不要重复提交"
            ));
        }
    };
    let mut storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.replace_remote_tasks(&after, "feishu")?;
    storage.record_synced_remote_write(&input, previous.as_ref(), &verified, "feishu")?;
    let (habit_saved, habit_warning) = if let Some(habit) = habit {
        let saved = if habit.kind == "task_type" {
            storage.save_task_type_habit(&habit.phrase, &verified.title, &verified.quadrant)
        } else {
            storage.save_habit(&habit.phrase, &verified, &habit.kind)
        };
        match saved {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error)),
        }
    } else {
        (false, None)
    };
    Ok(ConfirmResult {
        task: Some(verified),
        synced: true,
        queued: false,
        habit_saved,
        habit_warning,
    })
}

#[tauri::command]
fn confirm_update(
    state: tauri::State<'_, AppState>,
    input: ConfirmInput,
    habit: Option<HabitLearningInput>,
    replaces_queue_id: Option<i64>,
) -> Result<ConfirmResult, String> {
    let _write_guard = state
        .write_lock
        .try_lock()
        .map_err(|_| "上一笔写入仍在处理中，请勿重复提交".to_string())?;
    validate_confirm_input(&input)?;
    let source = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        active_task_source(&storage)?
    };
    write_acceptance::ensure_source(&source)?;
    if let Some(habit) = habit.as_ref() {
        if habit.phrase.trim().chars().count() < 2 {
            return Err("需要至少 2 个字符才能记住这个说法".to_string());
        }
        if !["alias", "correction", "task_type"].contains(&habit.kind.as_str()) {
            return Err("习惯类型无效".to_string());
        }
        if habit.kind == "task_type" && input.task_id.is_some() {
            return Err("固定任务类型只能用于新增任务".to_string());
        }
    }
    if source == "feishu" {
        return confirm_feishu_update(&state, input, habit, replaces_queue_id);
    }
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_wps_config(&storage)?
    };
    if config.webhook_url.is_empty() || !config.token_configured {
        return Err("WPS 尚未连接；本次没有写入，也没有伪装为成功".to_string());
    }
    let token = wps::read_token()?;
    let (before, connection) = match wps::fetch_tasks(&config, &token, storage::unix_now()) {
        Ok(result) => result,
        Err(error) if wps::is_connection_error(&error) => {
            let habit_json = habit
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .map_err(|serialize_error| serialize_error.to_string())?;
            let storage = state
                .storage
                .lock()
                .map_err(|_| "本地数据锁异常".to_string())?;
            if let Some(replaced_id) = replaces_queue_id {
                storage.replace_pending_write(
                    replaced_id,
                    &input,
                    habit_json.as_deref(),
                    &error,
                )?;
            } else {
                storage.enqueue_pending_write(&input, habit_json.as_deref(), &error)?;
            }
            return Ok(ConfirmResult {
                task: None,
                synced: false,
                queued: true,
                habit_saved: false,
                habit_warning: None,
            });
        }
        Err(error) => return Err(error),
    };
    if !connection.write_ready {
        return Err(
            "当前 AirScript 仅通过读取验证；请粘贴并保存项目提供的最新版脚本后再写入".to_string(),
        );
    }
    let previous = input
        .task_id
        .as_ref()
        .and_then(|task_id| before.iter().find(|task| &task.id == task_id))
        .cloned();
    if input.task_id.is_some() {
        let current = previous
            .as_ref()
            .ok_or_else(|| "目标任务已不在 WPS 中；请同步后重新分析".to_string())?;
        if remote_task_changed(&input, current) {
            if possible_partial_write(&input, current) {
                let snapshot = previous_task_from_snapshot(&input, current)
                    .ok_or_else(|| "无法恢复分析前快照；请在 WPS 中核对后重新分析".to_string())?;
                let message = "检测到上次写入可能部分成功：进度已变化，但状态未正确写入。已保留核对与撤销记录；请先撤销或在 WPS 中确认后再操作";
                persist_uncertain_write(
                    &state,
                    &input,
                    Some(&snapshot),
                    Some(current),
                    habit.as_ref(),
                    replaces_queue_id,
                    message,
                )?;
                state
                    .storage
                    .lock()
                    .map_err(|_| "本地数据锁异常".to_string())?
                    .replace_wps_tasks(&before)?;
                return Err(message.to_string());
            }
            if input
                .expected_title
                .as_deref()
                .is_some_and(|expected| expected != current.title)
            {
                let expected_title = input.expected_title.as_deref().unwrap_or("未知任务");
                return Err(format!(
                    "检测到 WPS 任务标识对应关系已变化：分析时选择「{expected_title}」，当前同一标识指向「{}」。为避免写错任务，本次没有写入；请按最新数据重新分析",
                    current.title
                ));
            }
            let expected_progress = input
                .expected_progress
                .map(|value| format!("{value}%"))
                .unwrap_or_else(|| "未知".to_string());
            let expected_status = input.expected_status.as_deref().unwrap_or("未知");
            return Err(format!(
                "检测到 WPS 冲突：分析时为 {expected_progress}、{expected_status}，当前为 {}%、{}。为避免覆盖，本次没有写入；请重新分析",
                current.progress, current.status
            ));
        }
    }
    let request = build_write_request(&input)?;
    if let Err(error) = wps::write_task(&config, &token, &request) {
        let message = format!("写入请求可能已送达：{error}。已保留核对与撤销记录，请勿重复提交");
        let observed_after = wps::fetch_tasks(&config, &token, storage::unix_now())
            .ok()
            .map(|(tasks, _)| tasks);
        let observed = observed_after.as_ref().and_then(|tasks| {
            input
                .task_id
                .as_ref()
                .and_then(|task_id| tasks.iter().find(|task| &task.id == task_id))
        });
        persist_uncertain_write(
            &state,
            &input,
            previous.as_ref(),
            observed,
            habit.as_ref(),
            replaces_queue_id,
            &message,
        )?;
        if let Some(tasks) = observed_after {
            state
                .storage
                .lock()
                .map_err(|_| "本地数据锁异常".to_string())?
                .replace_wps_tasks(&tasks)?;
        }
        return Err(message);
    }
    let (after, _) = match wps::fetch_tasks(&config, &token, storage::unix_now()) {
        Ok(result) => result,
        Err(error) => {
            let message =
                format!("写入请求已发送，但回读失败：{error}。已保留核对与撤销记录，请勿重复提交");
            persist_uncertain_write(
                &state,
                &input,
                previous.as_ref(),
                None,
                habit.as_ref(),
                replaces_queue_id,
                &message,
            )?;
            return Err(message);
        }
    };
    let verified = match verified_written_task(&before, &after, &input) {
        Some(task) => task,
        None => {
            let observed = input
                .task_id
                .as_ref()
                .and_then(|task_id| after.iter().find(|task| &task.id == task_id));
            let message =
                "WPS 已响应写入，但回读值与本次确认不一致。已保留核对与撤销记录，请勿重复提交";
            persist_uncertain_write(
                &state,
                &input,
                previous.as_ref(),
                observed,
                habit.as_ref(),
                replaces_queue_id,
                message,
            )?;
            state
                .storage
                .lock()
                .map_err(|_| "本地数据锁异常".to_string())?
                .replace_wps_tasks(&after)?;
            return Err(message.to_string());
        }
    };
    let mut storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.replace_wps_tasks(&after)?;
    let _ = storage.record_synced_wps_write(&input, previous.as_ref(), &verified);
    let (habit_saved, mut habit_warning) = if let Some(habit) = habit {
        let saved = if habit.kind == "task_type" {
            storage.save_task_type_habit(&habit.phrase, &verified.title, &verified.quadrant)
        } else {
            storage.save_habit(&habit.phrase, &verified, &habit.kind)
        };
        match saved {
            Ok(_) => (true, None),
            Err(error) => (false, Some(error)),
        }
    } else {
        (false, None)
    };
    if let Some(replaced_id) = replaces_queue_id {
        if let Err(error) = storage.delete_pending_write(replaced_id) {
            habit_warning = Some(format!(
                "WPS 已写入，但旧的待处理记录清理失败：{error}。它不会自动重试，请稍后在设置中放弃"
            ));
        }
    }
    Ok(ConfirmResult {
        task: Some(verified),
        synced: true,
        queued: false,
        habit_saved,
        habit_warning,
    })
}

#[tauri::command]
async fn confirm_floating_feishu_write(window:WebviewWindow,app:AppHandle,input:ConfirmInput,habit:Option<HabitLearningInput>,replaces_queue_id:Option<i64>)->Result<floating_feishu::Confirmation,String>{
    if window.label()!="main" {return Err("请从任务浮窗确认本次写入".into());}
    if replaces_queue_id.is_some(){return Err("WPS 待同步记录不能提交到飞书".into());}
    tauri::async_runtime::spawn_blocking(move||floating_feishu::confirm(&window,&app,input,habit)).await.map_err(|_|"写入处理未完成，请先核实结果，不要重复提交".to_string())?
}
#[tauri::command]
async fn undo_floating_feishu_write(window:WebviewWindow,app:AppHandle,receipt:floating_feishu::UndoReceipt)->Result<UndoResult,String>{
    if window.label()!="main" {return Err("请从任务浮窗核对撤销".into());}
    tauri::async_runtime::spawn_blocking(move||floating_feishu::undo(&window,&app,receipt)).await.map_err(|_|"撤销处理未完成，请先核实结果".to_string())?
}
#[tauri::command]
async fn reconcile_floating_feishu_write(window:WebviewWindow,app:AppHandle)->Result<floating_feishu::Reconciliation,String>{
    if window.label()!="main" {return Err("请从任务浮窗只读核实".into());}
    tauri::async_runtime::spawn_blocking(move||floating_feishu::reconcile(&app)).await.map_err(|_|"只读核实未完成，没有重发请求".to_string())?
}
#[tauri::command]
async fn delete_floating_feishu_task(window:WebviewWindow,app:AppHandle,task:Task,expected_target_key:String,context_version:u64)->Result<(),String>{
    if window.label()!="main" { return Err("请从任务浮窗选择要删除的记录".into()); }
    tauri::async_runtime::spawn_blocking(move||floating_feishu::delete_task(&window,&app,task,expected_target_key,context_version)).await.map_err(|_|"删除结果待核实，请只读核实，不要重复删除".to_string())?
}
#[tauri::command]
fn undo_latest(state: tauri::State<'_, AppState>) -> Result<UndoResult, String> {
    let _write_guard = state
        .write_lock
        .try_lock()
        .map_err(|_| "正在处理另一笔写入，请稍后撤销".to_string())?;
    let entry = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?
        .latest_synced_write()?;
    let Some(entry) = entry else {
        return Ok(UndoResult {
            undone: false,
            task_title: None,
        });
    };
    write_acceptance::ensure_source(&entry.source)?;
    if entry.source == "feishu" {
        return undo_latest_feishu(&state, entry);
    }
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_wps_config(&storage)?
    };
    if config.webhook_url.is_empty() || !config.token_configured {
        return Err("WPS 尚未连接，不能执行云端撤销".to_string());
    }
    let token = wps::read_token()?;
    let (before, connection) = wps::fetch_tasks(&config, &token, storage::unix_now())?;
    if !connection.undo_ready {
        return Err("云端撤销需要 AirScript v17；请更新并保存最新版脚本".to_string());
    }
    let current = before.iter().find(|task| task.id == entry.task_id);
    let request = if entry.created_task {
        let current = current.ok_or_else(|| "该临时任务已不在 WPS 中，未重复删除".to_string())?;
        if !remote_matches_synced_write(&entry, current) {
            return Err("撤销冲突：该临时任务已在 WPS 中发生变化，本次没有删除".to_string());
        }
        WpsWriteRequest::Delete {
            record_id: entry.task_id.clone(),
        }
    } else {
        let current = current.ok_or_else(|| "撤销冲突：目标任务已不在 WPS 中".to_string())?;
        if !remote_matches_synced_write(&entry, current)
            && !remote_matches_partial_blank_status_undo(&entry, current)
        {
            return Err("撤销冲突：任务在本次写入后又被修改，本次没有覆盖".to_string());
        }
        let previous_progress = entry
            .previous_progress
            .ok_or_else(|| "撤销记录缺少原进度，本次没有覆盖 WPS".to_string())?;
        let previous_status = entry
            .previous_status
            .clone()
            .ok_or_else(|| "撤销记录缺少原状态，本次没有覆盖 WPS".to_string())?;
        WpsWriteRequest::Update {
            record_id: entry.task_id.clone(),
            progress: previous_progress,
            status: Some(previous_status),
            planned_date: entry
                .planned_date_changed
                .then(|| entry.previous_planned_date.clone()),
            restore: true,
        }
    };
    wps::write_task(&config, &token, &request).map_err(|error| {
        format!("撤销请求可能已送达，但结果无法确认：{error}。请核对 WPS 后再操作")
    })?;
    let (after, _) = wps::fetch_tasks(&config, &token, storage::unix_now())
        .map_err(|error| format!("撤销请求已发送，但回读失败：{error}。请在 WPS 中核对"))?;
    let reverted = if entry.created_task {
        after.iter().all(|task| task.id != entry.task_id)
    } else {
        after.iter().any(|task| {
            task.id == entry.task_id
                && Some(task.progress) == entry.previous_progress
                && Some(task.status.as_str()) == entry.previous_status.as_deref()
                && (!entry.planned_date_changed || task.planned_date == entry.previous_planned_date)
        })
    };
    if !reverted {
        return Err("WPS 已响应撤销，但回读结果不一致；请在表格中核对".to_string());
    }
    let mut storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.replace_wps_tasks(&after)?;
    storage.mark_synced_write_reverted(entry.id)?;
    storage.resolve_uncertain_write(&entry.task_id, entry.next_progress)?;
    Ok(UndoResult {
        undone: true,
        task_title: Some(entry.task_title),
    })
}

fn undo_latest_feishu(state: &AppState, entry: SyncedWriteLog) -> Result<UndoResult, String> {
    let config = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "本地数据锁异常".to_string())?;
        load_feishu_config(&storage)?
    };
    write_acceptance::ensure_task_write(&config)?;
    let access_token = feishu::read_active_access_token(&config)?;
    let (before, connection) =
        feishu::fetch_tasks_with_access_token(&config, &access_token, storage::unix_now())
            .map_err(|error| format!("读取飞书撤销前状态失败：{error}"))?;
    if !connection.undo_ready {
        return Err("写入尚未开放：当前飞书连接不允许撤销".to_string());
    }
    let current = before.iter().find(|task| task.id == entry.task_id);
    let request = if entry.created_task {
        let current = current.ok_or_else(|| "该临时任务已不在飞书中，未重复删除".to_string())?;
        if !remote_matches_synced_write(&entry, current) {
            return Err("撤销冲突：该临时任务已在飞书中发生变化，本次没有删除".to_string());
        }
        FeishuWriteRequest::Delete {
            record_id: entry.task_id.clone(),
        }
    } else {
        let current = current.ok_or_else(|| "撤销冲突：目标任务已不在飞书中".to_string())?;
        if !remote_matches_synced_write(&entry, current)
            && !remote_matches_partial_blank_status_undo(&entry, current)
        {
            return Err("撤销冲突：任务在本次写入后又被修改，本次没有覆盖".to_string());
        }
        let previous_progress = entry
            .previous_progress
            .ok_or_else(|| "撤销记录缺少原进度，本次没有覆盖飞书".to_string())?;
        let previous_status = entry
            .previous_status
            .clone()
            .ok_or_else(|| "撤销记录缺少原状态，本次没有覆盖飞书".to_string())?;
        FeishuWriteRequest::Update {
            record_id: entry.task_id.clone(),
            progress: previous_progress,
            status: Some(previous_status),
            planned_date: entry
                .planned_date_changed
                .then(|| entry.previous_planned_date.clone()),
        }
    };
    feishu::write_task_with_access_token(&config, &access_token, &request).map_err(|error| {
        format!("飞书撤销请求可能已送达，但结果无法确认：{error}。请核对飞书后再操作")
    })?;
    let (after, _) =
        feishu::fetch_tasks_with_access_token(&config, &access_token, storage::unix_now())
            .map_err(|error| format!("飞书撤销请求已发送，但回读失败：{error}。请在飞书中核对"))?;
    let reverted = if entry.created_task {
        after.iter().all(|task| task.id != entry.task_id)
    } else {
        after.iter().any(|task| {
            task.id == entry.task_id
                && Some(task.progress) == entry.previous_progress
                && Some(task.status.as_str()) == entry.previous_status.as_deref()
                && (!entry.planned_date_changed || task.planned_date == entry.previous_planned_date)
        })
    };
    if !reverted {
        return Err("飞书已响应撤销，但回读结果不一致；请在表格中核对".to_string());
    }
    let mut storage = state
        .storage
        .lock()
        .map_err(|_| "本地数据锁异常".to_string())?;
    storage.replace_remote_tasks(&after, "feishu")?;
    storage.mark_synced_write_reverted(entry.id)?;
    storage.resolve_uncertain_write(&entry.task_id, entry.next_progress)?;
    Ok(UndoResult {
        undone: true,
        task_title: Some(entry.task_title),
    })
}

#[tauri::command]
fn get_autostart(app: AppHandle) -> Result<bool, String> {
    app.autolaunch()
        .is_enabled()
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    write_acceptance::ensure_normal_build()?;
    if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    }
    .map_err(|error| error.to_string())?;
    Ok(enabled)
}

#[cfg(all(feature = "feishu-direct-account-acceptance",not(feature="feishu-direct-sync-ui-acceptance")))]
pub fn run() { direct_account_auth::run(); }

#[cfg(any(not(feature = "feishu-direct-account-acceptance"),feature="feishu-direct-sync-ui-acceptance"))]
pub fn run() {
    #[cfg(feature="local-main-flow-ui-acceptance")]
    { local_main_flow_lab::run(); return; }
    #[cfg(feature = "local-sync-fault-ui-acceptance")]
    { local_workbench::fault_lab::run(); return; }
    #[cfg(all(feature = "local-workbench-acceptance",not(feature = "local-sync-fault-ui-acceptance")))]
    {
        local_workbench::run_acceptance();
        return;
    }
    #[cfg(all(not(feature = "local-workbench-acceptance"),not(feature="local-main-flow-ui-acceptance")))]
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .setup(|app| {
            feishu::validate_credential_profile(&app.config().identifier)
                .map_err(std::io::Error::other)?;
            write_acceptance::configure(&app.config().identifier).map_err(std::io::Error::other)?;
            if cfg!(feature = "feishu-template-v2-acceptance")
                && app.config().identifier != write_acceptance::TEMPLATE_V2_IDENTIFIER
                && app.config().identifier != write_acceptance::TEMPLATE_V2_PROMOTION_IDENTIFIER
                && app.config().identifier != write_acceptance::TEMPLATE_V2_FINAL_IDENTIFIER
            {
                return Err(std::io::Error::other(
                    "v2 template acceptance feature requires the isolated application identifier",
                )
                .into());
            }
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|error| error.to_string())?;
            std::fs::create_dir_all(&data_dir)?;
            app.manage(local_workbench::LocalWorkspaceState::new(data_dir.join("local-workspace.sqlite3")));
            app.manage(local_workbench::ImeApiState::new(data_dir.clone(),app.config().identifier.clone()));
            app.state::<local_workbench::ImeApiState>().startup(app.handle().clone());
            let storage = Storage::open(data_dir.join("project-track.sqlite3"))?;
            if cfg!(feature = "feishu-write-acceptance") {
                let selected = load_feishu_config(&storage).map_err(std::io::Error::other)?;
                write_acceptance::ensure_task_write(&selected).map_err(std::io::Error::other)?;
            }
            app.manage(AppState {
                storage: Mutex::new(storage),
                write_lock: Mutex::new(()),
                feishu_auth_flow: Mutex::new(None),
                floating: floating_feishu::State::open(data_dir.join("floating-feishu.sqlite3")).map_err(std::io::Error::other)?,
            });

            let sync_app = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(30 * 60));
                let state = sync_app.state::<AppState>();
                let Ok(_write_guard) = state.write_lock.try_lock() else {
                    continue;
                };
                match perform_active_sync(&state) {
                    Ok(result) => {
                        let queue_summary = if result.source == "wps" {
                            flush_pending_writes_inner(&state).ok()
                        } else {
                            None
                        };
                        let _ = sync_app.emit("task-sync-complete", result);
                        if let Some(summary) = queue_summary {
                            let _ = sync_app.emit("pending-writes-changed", summary);
                        }
                    }
                    Err(error) if error != "WPS 尚未配置" && error != "飞书尚未配置" => {
                        let _ = sync_app.emit("task-sync-failed", error);
                    }
                    Err(_) => {}
                }
            });

            let menu = MenuBuilder::new(app)
                .text("show", "显示浮窗")
                .text("sync", "刷新浮窗云端任务（旧功能）")
                .text("settings", "浮窗与连接设置")
                .text("local-workbench", "本地任务工作台")
                .separator()
                .text("quit", "退出")
                .build()?;

            let mut tray = TrayIconBuilder::with_id("project-track")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .tooltip("个人任务进度")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_main(app),
                    "local-workbench" => {
                        let app=app.clone();
                        tauri::async_runtime::spawn(async move {
                            if let Err(error)=local_workbench::open_local_workbench(app.clone()).await {
                                let _=app.emit_to("main","local-workbench-open-failed",error);
                            }
                        });
                    },
                    "sync" => {
                        show_main(app);
                        let _ = app.emit("request-sync", ());
                    }
                    "settings" => {
                        show_main(app);
                        let _ = app.emit("open-settings", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            if std::env::args().any(|argument| argument == "--hidden") {
                if let Some(window) = app.get_webview_window("main") {
                    window.hide()?;
                }
            } else if let Some(window) = app.get_webview_window("main") {
                apply_native_material_frame(&window)
                    .map_err(|error| std::io::Error::other(error.clone()))?;
                snap(&window).map_err(|error| std::io::Error::other(error))?;
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            local_workbench::open_local_workbench,
            open_local_feishu_setup,
            local_workbench::local_snapshot,
            local_workbench::local_sync_demo,
            inspect_local_feishu_sync,
            save_local_feishu_pairing,
            download_local_feishu_task,
            import_local_feishu_task,
            import_local_feishu_tasks,
            upload_local_feishu_task,
            reconcile_local_feishu_upload,
            restore_local_feishu_upload,
            prepare_local_feishu_batch,
            start_local_feishu_batch,
            stop_local_feishu_batch,
            step_local_feishu_batch,
            reconcile_local_feishu_batch,
            get_local_feishu_access,
            set_local_feishu_access,
            local_workbench::local_save_task,
            local_workbench::local_move_task,
            local_workbench::local_set_trashed,
            local_workbench::local_undo,
            local_workbench::ime::local_ime_status,
            local_workbench::ime::local_ime_set_enabled,
            local_workbench::ime::local_ime_rotate_token,
            local_workbench::ime::local_ime_reviews,
            local_workbench::ime::local_ime_activities,
            local_workbench::ime::local_ime_confirm_review,
            local_workbench::ime::local_ime_cancel_review,
            local_workbench::local_export_backup,
            local_workbench::local_export_csv,
            local_workbench::local_preview_restore,
            local_workbench::local_preview_legacy_workspace,
            local_workbench::local_confirm_restore,
            local_workbench::local_cancel_restore,
            get_write_acceptance_status,
            confirm_floating_feishu_write,
            undo_floating_feishu_write,
            reconcile_floating_feishu_write,
            set_expanded,
            hide_window,
            list_task_sources,
            list_tasks,
            floating_tasks::get_floating_task_view,
            floating_tasks::hide_floating_task,
            floating_tasks::show_hidden_floating_tasks,
            delete_floating_feishu_task,
            list_task_candidates,
            remember_task_mapping,
            list_habits,
            delete_habit,
            get_wps_config,
            save_wps_config,
            test_wps_connection,
            get_active_task_source,
            set_active_task_source,
            get_feishu_config,
            save_feishu_config,
            get_feishu_user_auth_status,
            start_feishu_user_authorization,
            poll_feishu_user_authorization,
            cancel_feishu_user_authorization,
            save_feishu_user_session,
            disconnect_feishu_user,
            get_task_workspace_setup_status,
            start_feishu_task_workspace_setup,
            resume_feishu_task_workspace_setup,
            reconcile_feishu_task_workspace_setup,
            save_feishu_template,
            inspect_feishu_schema,
            initialize_feishu_schema,
            inspect_feishu_workspace,
            initialize_feishu_workspace,
            test_feishu_connection,
            sync_wps_tasks,
            sync_active_tasks,
            get_pending_write_summary,
            list_pending_write_items,
            prepare_pending_write,
            discard_pending_write,
            get_ai_config,
            save_ai_config,
            list_ai_models,
            test_ai_connection,
            analyze_update,
            confirm_update,
            undo_latest,
            get_autostart,
            set_autostart
        ])
        .run(tauri::generate_context!())
        .expect("启动个人任务进度浮窗失败");
}
