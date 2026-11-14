//! Platform-neutral orchestration for creating or connecting a task workspace.
//!
//! The adapter owns authentication and all platform identifiers. Checkpoints deliberately
//! contain no access tokens or application secrets and can be persisted for later recovery.

use serde::{Deserialize, Serialize};

use crate::{
    feishu::{self, FeishuTemplateCopyError},
    storage::Storage,
    wps::{self, WpsConfig},
};

const PROVISION_CHECKPOINT_SETTING: &str = "workspace.provisioning.checkpoint.v1";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspacePlatform {
    Feishu,
    Wps,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProvisionMode {
    CreateNew,
    ConnectExisting,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProvisionPhase {
    Authorize,
    ProvisionWorkspace,
    DiscoverConnection,
    SaveConnection,
    Complete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionRequest {
    /// Stable across retries and reconciliation of this one setup attempt.
    pub operation_id: String,
    pub platform: WorkspacePlatform,
    pub mode: ProvisionMode,
    pub workspace_name: String,
    /// Optional, non-secret destination metadata selected by the user (for example, a folder id).
    /// It is persisted so recovery cannot silently change the chosen save location.
    #[serde(default)]
    pub workspace_location: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceReference {
    pub(crate) name: String,
    pub(crate) url: Option<String>,
    /// Opaque, non-secret platform handle. Never put credentials in this field.
    pub(crate) platform_handle: String,
}

impl WorkspaceReference {
    pub fn new(name: String, url: Option<String>, platform_handle: String) -> Self {
        Self {
            name,
            url,
            platform_handle,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConnection {
    /// Adapter-owned, non-secret metadata. Callers treat this as opaque.
    pub(crate) adapter_data: String,
    pub(crate) workspace_url: Option<String>,
}

impl WorkspaceConnection {
    pub fn new(adapter_data: String, workspace_url: Option<String>) -> Self {
        Self {
            adapter_data,
            workspace_url,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionCheckpoint {
    pub(crate) request: ProvisionRequest,
    pub(crate) phase: ProvisionPhase,
    pub(crate) workspace: Option<WorkspaceReference>,
    pub(crate) connection: Option<WorkspaceConnection>,
    /// Set when a remote create request may have succeeded but its response could not be
    /// verified. The caller must reconcile before resuming to avoid duplicate workspaces.
    pub(crate) needs_reconciliation: bool,
    /// Persisted immediately before a remote create call. If the process exits before saving
    /// the response, recovery treats the result as uncertain and never repeats the call.
    #[serde(default)]
    pub(crate) remote_mutation_started: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ProvisionState {
    Completed,
    AwaitingUserAction,
    Cancelled,
    PartialFailure,
    NeedsReconciliation,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionSummary {
    pub platform: WorkspacePlatform,
    pub workspace_name: String,
    pub workspace_url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProvisionResult {
    pub state: ProvisionState,
    pub phase: ProvisionPhase,
    pub checkpoint: ProvisionCheckpoint,
    pub summary: Option<ProvisionSummary>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterFailure {
    /// The user must perform an action (for example, finish browser authorization).
    NeedsUserAction(String),
    Cancelled(String),
    /// The current step can safely be retried with the same operation id.
    Retryable(String),
    /// A remote mutation may have been accepted; do not retry until reconciled.
    Uncertain(String),
    Fatal(String),
}

/// Platform boundary. Implementations keep access tokens and secrets inside the adapter;
/// the orchestration layer only receives a safe resource reference and connection metadata.
pub trait WorkspaceAdapter {
    fn platform(&self) -> WorkspacePlatform;

    fn authorize(&mut self, request: &ProvisionRequest) -> Result<(), AdapterFailure>;

    /// If the platform cannot establish whether a create succeeded, return
    /// `AdapterFailure::Uncertain`; the orchestrator persists this and will not retry silently.
    fn provision_workspace(
        &mut self,
        request: &ProvisionRequest,
    ) -> Result<WorkspaceReference, AdapterFailure>;

    fn discover_connection(
        &mut self,
        request: &ProvisionRequest,
        workspace: &WorkspaceReference,
    ) -> Result<WorkspaceConnection, AdapterFailure>;

    /// Store non-secret connection metadata and any credentials in the OS credential store.
    fn save_connection(
        &mut self,
        request: &ProvisionRequest,
        connection: &WorkspaceConnection,
    ) -> Result<(), AdapterFailure>;
}

/// Feishu adapter for the one-click create-new path. The short-lived user token is supplied
/// by the authorization layer and is never copied into the serializable checkpoint.
pub struct FeishuWorkspaceAdapter<'a> {
    user_access_token: String,
    template_app_token: String,
    storage: &'a Storage,
}

fn retry_feishu_copy_readiness<T>(
    mut operation: impl FnMut() -> Result<T, AdapterFailure>,
    delays_seconds: &[u64],
    mut sleep: impl FnMut(std::time::Duration),
) -> Result<T, AdapterFailure> {
    for delay_seconds in delays_seconds {
        let result = operation();
        if matches!(&result, Err(AdapterFailure::Retryable(message)) if message.contains("1254036"))
        {
            sleep(std::time::Duration::from_secs(*delay_seconds));
        } else {
            return result;
        }
    }
    operation()
}

impl<'a> FeishuWorkspaceAdapter<'a> {
    pub fn new(
        user_access_token: String,
        template_app_token: String,
        storage: &'a Storage,
    ) -> Self {
        Self {
            user_access_token,
            template_app_token,
            storage,
        }
    }

    pub fn from_saved_user_session(
        template_app_token: String,
        storage: &'a Storage,
    ) -> Result<Self, String> {
        let app_id = storage.get_setting("feishu.app_id")?.unwrap_or_default();
        let user_access_token = feishu::read_user_access_token(&app_id)?;
        Ok(Self::new(user_access_token, template_app_token, storage))
    }
}

impl WorkspaceAdapter for FeishuWorkspaceAdapter<'_> {
    fn platform(&self) -> WorkspacePlatform {
        WorkspacePlatform::Feishu
    }

    fn authorize(&mut self, _: &ProvisionRequest) -> Result<(), AdapterFailure> {
        if self.user_access_token.trim().is_empty() {
            return Err(AdapterFailure::NeedsUserAction(
                "请先完成飞书账号授权。".to_string(),
            ));
        }
        feishu::ensure_template_tables_empty(&self.template_app_token, &self.user_access_token)
            .map_err(|error| {
                if error.contains("99991679") || error.contains("91403") {
                    AdapterFailure::NeedsUserAction(format!(
                        "飞书授权尚不能核验模板空记录。请安装最新版后重新连接飞书账号，再继续：{error}"
                    ))
                } else if error.contains("至少包含 1 条任务记录") {
                    AdapterFailure::Fatal(format!(
                        "模板源已包含记录，已停止创建，避免把模板数据复制给新用户：{error}"
                    ))
                } else {
                    AdapterFailure::Retryable(format!(
                        "未能核验飞书模板源为空；没有发起复制请求：{error}"
                    ))
                }
            })?;
        Ok(())
    }

    fn provision_workspace(
        &mut self,
        request: &ProvisionRequest,
    ) -> Result<WorkspaceReference, AdapterFailure> {
        if request.mode != ProvisionMode::CreateNew {
            return Err(AdapterFailure::Fatal(
                "飞书一键初始化目前只接受创建新任务空间。".to_string(),
            ));
        }
        let copied = feishu::copy_task_workspace_template(
            &self.template_app_token,
            &self.user_access_token,
            &request.workspace_name,
            request.workspace_location.as_deref(),
        )
        .map_err(|error| match error {
            FeishuTemplateCopyError::OutcomeUnknown(message) => AdapterFailure::Uncertain(message),
            FeishuTemplateCopyError::Rejected(message) if message.contains("HTTP 401") => {
                AdapterFailure::NeedsUserAction("飞书授权已失效，请重新授权后继续。".to_string())
            }
            FeishuTemplateCopyError::Rejected(message) if message.contains("91403") => {
                AdapterFailure::NeedsUserAction(message)
            }
            FeishuTemplateCopyError::Rejected(message) => AdapterFailure::Fatal(message),
        })?;
        Ok(WorkspaceReference::new(
            copied.name,
            copied.url,
            copied.app_token,
        ))
    }

    fn discover_connection(
        &mut self,
        _: &ProvisionRequest,
        workspace: &WorkspaceReference,
    ) -> Result<WorkspaceConnection, AdapterFailure> {
        let discover_once = || -> Result<WorkspaceConnection, AdapterFailure> {
            let table =
                feishu::discover_task_table(&workspace.platform_handle, &self.user_access_token)
                    .map_err(AdapterFailure::Retryable)?;
            feishu::ensure_task_table_empty(
                &workspace.platform_handle,
                &table.table_id,
                &self.user_access_token,
            )
            .map_err(AdapterFailure::Retryable)?;
            let status = feishu::inspect_workspace_as_user(
                &workspace.platform_handle,
                &table.table_id,
                &self.user_access_token,
            )
            .map_err(AdapterFailure::Retryable)?;
            let unreadable_metadata = status
                .inspection_warnings
                .iter()
                .filter(|warning| {
                    warning.contains("读取视图")
                        || warning.contains("读取仪表盘")
                        || warning.contains("读取自动化")
                })
                .cloned()
                .collect::<Vec<_>>();
            if !unreadable_metadata.is_empty() {
                return Err(AdapterFailure::Retryable(format!(
                "副本已创建，但无法核验视图、仪表盘或自动化：{}。任务空间链接已保留，请修复权限后继续，不要重新复制。",
                unreadable_metadata.join("；")
            )));
            }
            let missing_base = status.missing_required_fields.iter().filter(|f| f.as_str() != "计划完成日期").cloned().collect::<Vec<_>>();
            if !missing_base.is_empty() {
                return Err(AdapterFailure::Retryable(format!(
                    "复制后的任务表缺少必需字段：{}",
                    missing_base.join("、")
                )));
            }
            let status_board_name = if status.status_is_formula {
                "今日必做看板"
            } else {
                "状态看板"
            };
            let required_views = [
                ("任务总览", "grid"),
                ("四象限看板", "kanban"),
                (status_board_name, "kanban"),
                ("今日任务", "grid"),
            ];
            let missing_views = required_views
                .iter()
                .filter(|(required_name, required_kind)| {
                    !status
                        .views
                        .iter()
                        .any(|view| view.name == *required_name && view.kind == *required_kind)
                })
                .map(|(name, _)| *name)
                .collect::<Vec<_>>();
            if !missing_views.is_empty() {
                return Err(AdapterFailure::Retryable(format!(
                "副本已创建，但未核验到必需视图：{}。任务空间链接已保留，请修复后继续，不要重新复制。",
                missing_views.join("、")
            )));
            }
            if !status
                .dashboards
                .iter()
                .any(|dashboard| dashboard.name == "个人任务仪表盘")
            {
                return Err(AdapterFailure::Retryable(
                "副本已创建，但未核验到个人任务仪表盘。任务空间链接已保留，请修复后继续，不要重新复制。"
                    .to_string(),
            ));
            }
            let workflow_issues = feishu::status_linkage_issues(
                status.status_is_formula,
                &status.workflows,
                cfg!(feature = "feishu-template-v2-acceptance"),
            );
            if !workflow_issues.is_empty() {
                return Err(AdapterFailure::Retryable(format!(
                    "副本已创建，但状态联动自动化未就绪：{}。请打开已创建的任务空间补齐并启用这三条规则，然后返回点击“继续上次初始化”；不要重新创建。",
                    workflow_issues.join("；")
                )));
            }
            Ok(WorkspaceConnection::new(
                serde_json::json!({
                    "appToken": workspace.platform_handle,
                    "tableId": table.table_id
                })
                .to_string(),
                workspace.url.clone(),
            ))
        };
        // Feishu returns 1254036 while an accepted copy is still materializing.
        // Retry only read-only discovery here, never the remote copy request.
        let connection = retry_feishu_copy_readiness(discover_once, &[1, 2, 3, 5, 8], std::thread::sleep)?;
        let target: serde_json::Value = serde_json::from_str(&connection.adapter_data)
            .map_err(|_| AdapterFailure::Fatal("新任务空间的连接信息无效，未补齐字段".into()))?;
        let table_id = target["tableId"].as_str().ok_or_else(|| AdapterFailure::Fatal("新任务空间缺少数据表标识".into()))?;
        feishu::ensure_new_workspace_initialization_fields(&workspace.platform_handle, table_id, &self.user_access_token)
            .map_err(|e| AdapterFailure::Retryable(format!("副本已保留，但初始化字段尚未核实：{e}。请检查权限后继续上次初始化，不要重新复制。")))?;
        Ok(connection)
    }

    fn save_connection(
        &mut self,
        _: &ProvisionRequest,
        connection: &WorkspaceConnection,
    ) -> Result<(), AdapterFailure> {
        let metadata: serde_json::Value = serde_json::from_str(&connection.adapter_data)
            .map_err(|_| AdapterFailure::Fatal("飞书连接检查点格式无效。".to_string()))?;
        let app_token = metadata["appToken"]
            .as_str()
            .ok_or_else(|| AdapterFailure::Fatal("飞书连接检查点缺少表格标识。".to_string()))?;
        let table_id = metadata["tableId"]
            .as_str()
            .ok_or_else(|| AdapterFailure::Fatal("飞书连接检查点缺少数据表标识。".to_string()))?;
        self.storage
            .set_setting("feishu.app_token", app_token)
            .map_err(AdapterFailure::Retryable)?;
        self.storage
            .set_setting("feishu.table_id", table_id)
            .map_err(AdapterFailure::Retryable)?;
        if let Some(url) = connection.workspace_url.as_deref() {
            self.storage
                .set_setting("feishu.workspace_url", url)
                .map_err(AdapterFailure::Retryable)?;
        }
        self.storage
            .set_setting("task_source.active", "feishu")
            .map_err(AdapterFailure::Retryable)
    }
}

/// Compatibility adapter for connecting an already configured WPS task table.
/// WPS creation/import remains a later onboarding phase; existing AirScript access is retained.
pub struct WpsWorkspaceAdapter<'a> {
    config: WpsConfig,
    token: String,
    storage: &'a Storage,
}

impl<'a> WpsWorkspaceAdapter<'a> {
    pub fn new(config: WpsConfig, token: String, storage: &'a Storage) -> Self {
        Self {
            config,
            token,
            storage,
        }
    }
}

impl WorkspaceAdapter for WpsWorkspaceAdapter<'_> {
    fn platform(&self) -> WorkspacePlatform {
        WorkspacePlatform::Wps
    }

    fn authorize(&mut self, _: &ProvisionRequest) -> Result<(), AdapterFailure> {
        if self.config.webhook_url.trim().is_empty() || self.token.trim().is_empty() {
            return Err(AdapterFailure::NeedsUserAction(
                "请先完成 WPS 高级连接配置。".to_string(),
            ));
        }
        Ok(())
    }

    fn provision_workspace(
        &mut self,
        request: &ProvisionRequest,
    ) -> Result<WorkspaceReference, AdapterFailure> {
        if request.mode != ProvisionMode::ConnectExisting {
            return Err(AdapterFailure::NeedsUserAction(
                "WPS 一键创建任务空间将在后续阶段接入；当前可连接已有 WPS 任务表。".to_string(),
            ));
        }
        let (_, result) =
            wps::fetch_tasks_resolving_sheet(&self.config, &self.token, crate::storage::unix_now())
                .map_err(AdapterFailure::Retryable)?;
        Ok(WorkspaceReference::new(
            result.sheet_name.clone(),
            None,
            result.sheet_name,
        ))
    }

    fn discover_connection(
        &mut self,
        _: &ProvisionRequest,
        workspace: &WorkspaceReference,
    ) -> Result<WorkspaceConnection, AdapterFailure> {
        let (_, result) =
            wps::fetch_tasks_resolving_sheet(&self.config, &self.token, crate::storage::unix_now())
                .map_err(AdapterFailure::Retryable)?;
        if result.sheet_name != workspace.platform_handle {
            return Err(AdapterFailure::Retryable(
                "WPS 数据表名称在检查期间发生变化；请重新分析后继续。".to_string(),
            ));
        }
        Ok(WorkspaceConnection::new(
            serde_json::json!({ "sheetName": result.sheet_name }).to_string(),
            None,
        ))
    }

    fn save_connection(
        &mut self,
        _: &ProvisionRequest,
        connection: &WorkspaceConnection,
    ) -> Result<(), AdapterFailure> {
        let metadata: serde_json::Value = serde_json::from_str(&connection.adapter_data)
            .map_err(|_| AdapterFailure::Fatal("WPS 连接检查点格式无效。".to_string()))?;
        let sheet_name = metadata["sheetName"]
            .as_str()
            .ok_or_else(|| AdapterFailure::Fatal("WPS 连接检查点缺少数据表名称。".to_string()))?;
        self.storage
            .set_setting("wps.sheet_name", sheet_name)
            .map_err(AdapterFailure::Retryable)?;
        self.storage
            .set_setting("task_source.active", "wps")
            .map_err(AdapterFailure::Retryable)
    }
}

pub fn provision_workspace<A: WorkspaceAdapter>(
    adapter: &mut A,
    request: ProvisionRequest,
) -> ProvisionResult {
    let checkpoint = ProvisionCheckpoint {
        request,
        phase: ProvisionPhase::Authorize,
        workspace: None,
        connection: None,
        needs_reconciliation: false,
        remote_mutation_started: false,
    };
    run(adapter, checkpoint)
}

/// Persist the current operation before any remote mutation and after every phase transition.
/// A prior unfinished operation blocks a new one until it has been resumed or reconciled.
pub fn provision_workspace_persisted<A: WorkspaceAdapter>(
    adapter: &mut A,
    storage: &Storage,
    request: ProvisionRequest,
) -> ProvisionResult {
    match storage.get_setting(PROVISION_CHECKPOINT_SETTING) {
        Ok(Some(serialized)) => {
            let Some(checkpoint) = serde_json::from_str::<ProvisionCheckpoint>(&serialized).ok()
            else {
                return result(
                    ProvisionState::Failed,
                    new_checkpoint(request),
                    None,
                    Some("已有任务空间恢复记录损坏；已停止新建，避免生成重复副本。"),
                );
            };
            let state = if checkpoint.needs_reconciliation || checkpoint.remote_mutation_started {
                ProvisionState::NeedsReconciliation
            } else {
                ProvisionState::PartialFailure
            };
            let summary = checkpoint
                .workspace
                .as_ref()
                .map(|workspace| ProvisionSummary {
                    platform: adapter.platform(),
                    workspace_name: workspace.name.clone(),
                    workspace_url: workspace.url.clone(),
                });
            return result(
                state,
                checkpoint,
                summary,
                Some("已有任务空间初始化尚未完成；请先恢复或核对上次操作，避免生成重复副本。"),
            );
        }
        Err(error) => {
            return result(
                ProvisionState::Failed,
                new_checkpoint(request),
                None,
                Some(&format!("无法读取初始化恢复记录：{error}")),
            );
        }
        Ok(None) => {}
    }

    let checkpoint = new_checkpoint(request);
    if let Err(error) = persist_checkpoint(storage, &checkpoint) {
        return result(
            ProvisionState::Failed,
            checkpoint,
            None,
            Some(&format!(
                "无法保存初始化恢复记录，因此没有开始远端操作：{error}"
            )),
        );
    }
    let result = run_with_persistence(adapter, checkpoint, |value| {
        persist_checkpoint(storage, value)
    });
    if result.state == ProvisionState::Completed {
        let _ = storage.remove_setting(PROVISION_CHECKPOINT_SETTING);
    }
    result
}

/// Load and resume the operation saved in local app settings. If a prior process exited while
/// the remote create call was in flight, force reconciliation before allowing another attempt.
pub fn resume_persisted_workspace<A: WorkspaceAdapter>(
    adapter: &mut A,
    storage: &Storage,
) -> Result<Option<ProvisionResult>, String> {
    let Some(serialized) = storage.get_setting(PROVISION_CHECKPOINT_SETTING)? else {
        return Ok(None);
    };
    let mut checkpoint: ProvisionCheckpoint = serde_json::from_str(&serialized)
        .map_err(|_| "本地任务空间恢复记录无法读取，请保留数据库并联系支持".to_string())?;

    if checkpoint.remote_mutation_started && !checkpoint.needs_reconciliation {
        checkpoint.needs_reconciliation = true;
        persist_checkpoint(storage, &checkpoint)?;
        return Ok(Some(result(
            ProvisionState::NeedsReconciliation,
            checkpoint,
            None,
            Some("上次操作在飞书复制请求期间中断。请先检查飞书空间列表；本次没有自动重试。"),
        )));
    }

    if checkpoint.needs_reconciliation {
        return Ok(Some(result(
            ProvisionState::NeedsReconciliation,
            checkpoint,
            None,
            Some("请先确认飞书是否已创建任务空间，再继续；为避免生成重复副本，本次没有重试复制。"),
        )));
    }

    let result = run_with_persistence(adapter, checkpoint, |value| {
        persist_checkpoint(storage, value)
    });
    if result.state == ProvisionState::Completed {
        storage.remove_setting(PROVISION_CHECKPOINT_SETTING)?;
    }
    Ok(Some(result))
}

/// Resolve a pending create after the user has checked Feishu. Supplying a workspace resumes
/// discovery; supplying None is an explicit assertion that no copy exists and makes retry safe.
pub fn resolve_persisted_uncertain_creation(
    storage: &Storage,
    workspace: Option<WorkspaceReference>,
) -> Result<(), String> {
    let serialized = storage
        .get_setting(PROVISION_CHECKPOINT_SETTING)?
        .ok_or_else(|| "没有待核对的任务空间初始化记录".to_string())?;
    let checkpoint: ProvisionCheckpoint = serde_json::from_str(&serialized)
        .map_err(|_| "本地任务空间恢复记录无法读取，请保留数据库并联系支持".to_string())?;
    if !checkpoint.needs_reconciliation {
        return Err("当前初始化记录不需要远端结果核对".to_string());
    }
    let resolved = resolve_uncertain_creation(checkpoint, workspace);
    persist_checkpoint(storage, &resolved)
}

pub fn get_persisted_checkpoint(storage: &Storage) -> Result<Option<ProvisionCheckpoint>, String> {
    storage
        .get_setting(PROVISION_CHECKPOINT_SETTING)?
        .map(|serialized| {
            serde_json::from_str(&serialized)
                .map_err(|_| "本地任务空间恢复记录无法读取，请保留数据库并联系支持".to_string())
        })
        .transpose()
}

fn new_checkpoint(request: ProvisionRequest) -> ProvisionCheckpoint {
    ProvisionCheckpoint {
        request,
        phase: ProvisionPhase::Authorize,
        workspace: None,
        connection: None,
        needs_reconciliation: false,
        remote_mutation_started: false,
    }
}

fn persist_checkpoint(storage: &Storage, checkpoint: &ProvisionCheckpoint) -> Result<(), String> {
    let serialized =
        serde_json::to_string(checkpoint).map_err(|_| "无法序列化任务空间恢复记录".to_string())?;
    storage.set_setting(PROVISION_CHECKPOINT_SETTING, &serialized)
}

pub fn resume_workspace<A: WorkspaceAdapter>(
    adapter: &mut A,
    checkpoint: ProvisionCheckpoint,
) -> ProvisionResult {
    if checkpoint.needs_reconciliation {
        return result(
            ProvisionState::NeedsReconciliation,
            checkpoint,
            None,
            Some(
                "请先确认远端是否已经创建任务空间，再继续；为避免生成重复副本，本次没有重试创建。",
            ),
        );
    }
    run(adapter, checkpoint)
}

/// Resolve an uncertain create only after the caller has verified the remote state.
/// `Some(workspace)` resumes at discovery; `None` means the caller verified no workspace was
/// created, so it is safe to retry the same operation id.
pub fn resolve_uncertain_creation(
    mut checkpoint: ProvisionCheckpoint,
    workspace: Option<WorkspaceReference>,
) -> ProvisionCheckpoint {
    checkpoint.needs_reconciliation = false;
    checkpoint.remote_mutation_started = false;
    if let Some(workspace) = workspace {
        checkpoint.workspace = Some(workspace);
        checkpoint.phase = ProvisionPhase::DiscoverConnection;
    } else {
        checkpoint.workspace = None;
        checkpoint.phase = ProvisionPhase::ProvisionWorkspace;
    }
    checkpoint
}

fn run<A: WorkspaceAdapter>(adapter: &mut A, checkpoint: ProvisionCheckpoint) -> ProvisionResult {
    run_with_persistence(adapter, checkpoint, |_| Ok(()))
}

fn run_with_persistence<
    A: WorkspaceAdapter,
    F: FnMut(&ProvisionCheckpoint) -> Result<(), String>,
>(
    adapter: &mut A,
    mut checkpoint: ProvisionCheckpoint,
    mut persist: F,
) -> ProvisionResult {
    if adapter.platform() != checkpoint.request.platform {
        return result(
            ProvisionState::Failed,
            checkpoint,
            None,
            Some("任务空间平台与连接器不一致。"),
        );
    }

    loop {
        if let Err(error) = persist(&checkpoint) {
            return result(
                ProvisionState::Failed,
                checkpoint,
                None,
                Some(&format!("无法保存初始化恢复记录，已停止后续操作：{error}")),
            );
        }
        match checkpoint.phase {
            ProvisionPhase::Authorize => match adapter.authorize(&checkpoint.request) {
                Ok(()) => {
                    checkpoint.phase = ProvisionPhase::ProvisionWorkspace;
                    continue;
                }
                Err(error) => return persisted_failure(checkpoint, error, false, &mut persist),
            },
            ProvisionPhase::ProvisionWorkspace => {
                if !checkpoint.remote_mutation_started {
                    checkpoint.remote_mutation_started = true;
                    if let Err(error) = persist(&checkpoint) {
                        checkpoint.remote_mutation_started = false;
                        return result(
                            ProvisionState::Failed,
                            checkpoint,
                            None,
                            Some(&format!("无法保存远端操作保护记录，已停止复制：{error}")),
                        );
                    }
                }
                match adapter.provision_workspace(&checkpoint.request) {
                    Ok(workspace) => {
                        checkpoint.remote_mutation_started = false;
                        checkpoint.workspace = Some(workspace);
                        checkpoint.phase = ProvisionPhase::DiscoverConnection;
                        continue;
                    }
                    Err(error) => {
                        let uncertain = matches!(error, AdapterFailure::Uncertain(_));
                        if !uncertain {
                            checkpoint.remote_mutation_started = false;
                        }
                        return persisted_failure(checkpoint, error, uncertain, &mut persist);
                    }
                }
            }
            ProvisionPhase::DiscoverConnection => {
                let Some(workspace) = checkpoint.workspace.as_ref() else {
                    return result(
                        ProvisionState::Failed,
                        checkpoint,
                        None,
                        Some("恢复记录缺少已创建的任务空间信息。"),
                    );
                };
                match adapter.discover_connection(&checkpoint.request, workspace) {
                    Ok(connection) => {
                        checkpoint.connection = Some(connection);
                        checkpoint.phase = ProvisionPhase::SaveConnection;
                        continue;
                    }
                    Err(error) => return persisted_failure(checkpoint, error, false, &mut persist),
                }
            }
            ProvisionPhase::SaveConnection => {
                let Some(connection) = checkpoint.connection.as_ref() else {
                    return result(
                        ProvisionState::Failed,
                        checkpoint,
                        None,
                        Some("恢复记录缺少发现到的连接信息。"),
                    );
                };
                match adapter.save_connection(&checkpoint.request, connection) {
                    Ok(()) => {
                        checkpoint.phase = ProvisionPhase::Complete;
                        continue;
                    }
                    Err(error) => return persisted_failure(checkpoint, error, false, &mut persist),
                }
            }
            ProvisionPhase::Complete => {
                let summary = checkpoint
                    .workspace
                    .as_ref()
                    .map(|workspace| ProvisionSummary {
                        platform: adapter.platform(),
                        workspace_name: workspace.name.clone(),
                        workspace_url: workspace.url.clone(),
                    });
                return result(ProvisionState::Completed, checkpoint, summary, None);
            }
        }
    }
}

fn persisted_failure<F: FnMut(&ProvisionCheckpoint) -> Result<(), String>>(
    checkpoint: ProvisionCheckpoint,
    error: AdapterFailure,
    uncertain: bool,
    persist: &mut F,
) -> ProvisionResult {
    let failed = failure(checkpoint, error, uncertain);
    match persist(&failed.checkpoint) {
        Ok(()) => failed,
        Err(persist_error) => result(
            ProvisionState::Failed,
            failed.checkpoint,
            None,
            Some(&format!(
                "初始化步骤失败，且恢复记录保存失败：{persist_error}。请勿重新开始，先检查飞书空间。"
            )),
        ),
    }
}

fn failure(
    mut checkpoint: ProvisionCheckpoint,
    error: AdapterFailure,
    uncertain: bool,
) -> ProvisionResult {
    checkpoint.needs_reconciliation = uncertain;
    checkpoint.remote_mutation_started = uncertain;
    let (state, message) = match error {
        AdapterFailure::NeedsUserAction(message) => (ProvisionState::AwaitingUserAction, message),
        AdapterFailure::Cancelled(message) => (ProvisionState::Cancelled, message),
        AdapterFailure::Retryable(message) => (ProvisionState::PartialFailure, message),
        AdapterFailure::Uncertain(message) => (ProvisionState::NeedsReconciliation, message),
        AdapterFailure::Fatal(message) => (ProvisionState::Failed, message),
    };
    result(state, checkpoint, None, Some(&message))
}

fn result(
    state: ProvisionState,
    checkpoint: ProvisionCheckpoint,
    summary: Option<ProvisionSummary>,
    message: Option<&str>,
) -> ProvisionResult {
    ProvisionResult {
        state,
        phase: checkpoint.phase,
        checkpoint,
        summary,
        message: message.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct FakeAdapter {
        failures: VecDeque<(ProvisionPhase, AdapterFailure)>,
        calls: Vec<ProvisionPhase>,
        provisioned_locations: Vec<Option<String>>,
        created: usize,
    }

    impl FakeAdapter {
        fn new() -> Self {
            Self {
                failures: VecDeque::new(),
                calls: Vec::new(),
                provisioned_locations: Vec::new(),
                created: 0,
            }
        }

        fn fail_at(mut self, phase: ProvisionPhase, failure: AdapterFailure) -> Self {
            self.failures.push_back((phase, failure));
            self
        }

        fn fail_if_configured(&mut self, phase: ProvisionPhase) -> Result<(), AdapterFailure> {
            self.calls.push(phase);
            if self.failures.front().is_some_and(|(at, _)| *at == phase) {
                return Err(self.failures.pop_front().expect("configured failure").1);
            }
            Ok(())
        }
    }

    impl WorkspaceAdapter for FakeAdapter {
        fn platform(&self) -> WorkspacePlatform {
            WorkspacePlatform::Feishu
        }

        fn authorize(&mut self, _: &ProvisionRequest) -> Result<(), AdapterFailure> {
            self.fail_if_configured(ProvisionPhase::Authorize)
        }

        fn provision_workspace(
            &mut self,
            request: &ProvisionRequest,
        ) -> Result<WorkspaceReference, AdapterFailure> {
            self.provisioned_locations
                .push(request.workspace_location.clone());
            self.fail_if_configured(ProvisionPhase::ProvisionWorkspace)?;
            self.created += 1;
            Ok(WorkspaceReference {
                name: "个人任务".into(),
                url: Some("https://example.feishu.cn/base/test".into()),
                platform_handle: "fake-handle".into(),
            })
        }

        fn discover_connection(
            &mut self,
            _: &ProvisionRequest,
            _: &WorkspaceReference,
        ) -> Result<WorkspaceConnection, AdapterFailure> {
            self.fail_if_configured(ProvisionPhase::DiscoverConnection)?;
            Ok(WorkspaceConnection::new(
                "fake-opaque-connection".into(),
                Some("https://example.feishu.cn/base/test".into()),
            ))
        }

        fn save_connection(
            &mut self,
            _: &ProvisionRequest,
            _: &WorkspaceConnection,
        ) -> Result<(), AdapterFailure> {
            self.fail_if_configured(ProvisionPhase::SaveConnection)
        }
    }

    fn request() -> ProvisionRequest {
        ProvisionRequest {
            operation_id: "op-1".into(),
            platform: WorkspacePlatform::Feishu,
            mode: ProvisionMode::CreateNew,
            workspace_name: "个人任务".into(),
            workspace_location: None,
        }
    }

    #[test]
    fn waits_for_an_accepted_feishu_copy_without_repeating_creation() {
        let mut reads = 0;
        let mut waits = Vec::new();
        let result = retry_feishu_copy_readiness(
            || {
                reads += 1;
                if reads < 3 {
                    Err(AdapterFailure::Retryable(
                        "Bitable is copying（代码 1254036）".into(),
                    ))
                } else {
                    Ok("ready")
                }
            },
            &[1, 2, 3],
            |duration| waits.push(duration.as_secs()),
        );
        assert_eq!(result.expect("copy becomes readable"), "ready");
        assert_eq!(reads, 3);
        assert_eq!(waits, [1, 2]);
    }

    #[test]
    fn does_not_wait_or_retry_unrelated_feishu_discovery_errors() {
        let mut reads = 0;
        let result = retry_feishu_copy_readiness(
            || {
                reads += 1;
                Err::<(), _>(AdapterFailure::Retryable("权限不足（代码 91403）".into()))
            },
            &[1, 2, 3],
            |_| panic!("unrelated error must not wait"),
        );
        assert!(
            matches!(result, Err(AdapterFailure::Retryable(message)) if message.contains("91403"))
        );
        assert_eq!(reads, 1);
    }

    #[test]
    fn selected_workspace_location_survives_retry_and_legacy_checkpoint_restore() {
        let selected_location = "fldFolder-456".to_string();
        let mut setup_request = request();
        setup_request.workspace_location = Some(selected_location.clone());
        let mut adapter = FakeAdapter::new().fail_at(
            ProvisionPhase::ProvisionWorkspace,
            AdapterFailure::Retryable("复制请求被明确拒绝".into()),
        );

        let first = provision_workspace(&mut adapter, setup_request);
        assert_eq!(first.state, ProvisionState::PartialFailure);
        let serialized = serde_json::to_string(&first.checkpoint).expect("checkpoint serializes");
        let recovered: ProvisionCheckpoint =
            serde_json::from_str(&serialized).expect("checkpoint restores");
        let resumed = resume_workspace(&mut adapter, recovered);

        assert_eq!(resumed.state, ProvisionState::Completed);
        assert_eq!(
            adapter.provisioned_locations,
            vec![Some(selected_location.clone()), Some(selected_location)]
        );

        let mut legacy_checkpoint = serde_json::to_value(request()).expect("request serializes");
        legacy_checkpoint
            .as_object_mut()
            .expect("request JSON object")
            .remove("workspaceLocation");
        let legacy_request: ProvisionRequest =
            serde_json::from_value(legacy_checkpoint).expect("legacy request restores");
        assert_eq!(legacy_request.workspace_location, None);
    }

    #[test]
    fn completes_all_steps_and_returns_only_a_safe_summary() {
        let mut adapter = FakeAdapter::new();
        let result = provision_workspace(&mut adapter, request());

        assert_eq!(result.state, ProvisionState::Completed);
        assert_eq!(result.phase, ProvisionPhase::Complete);
        assert_eq!(adapter.created, 1);
        assert_eq!(
            result.summary,
            Some(ProvisionSummary {
                platform: WorkspacePlatform::Feishu,
                workspace_name: "个人任务".into(),
                workspace_url: Some("https://example.feishu.cn/base/test".into()),
            })
        );
    }

    #[test]
    fn reports_user_cancellation_without_advancing_the_checkpoint() {
        let mut adapter = FakeAdapter::new().fail_at(
            ProvisionPhase::Authorize,
            AdapterFailure::Cancelled("已取消授权".into()),
        );
        let result = provision_workspace(&mut adapter, request());

        assert_eq!(result.state, ProvisionState::Cancelled);
        assert_eq!(result.phase, ProvisionPhase::Authorize);
        assert_eq!(adapter.created, 0);
    }

    #[test]
    fn resumes_after_a_partial_failure_without_creating_a_second_workspace() {
        let mut adapter = FakeAdapter::new().fail_at(
            ProvisionPhase::DiscoverConnection,
            AdapterFailure::Retryable("暂时无法读取数据表".into()),
        );
        let first = provision_workspace(&mut adapter, request());
        assert_eq!(first.state, ProvisionState::PartialFailure);
        assert_eq!(first.phase, ProvisionPhase::DiscoverConnection);
        assert_eq!(adapter.created, 1);

        let serialized = serde_json::to_string(&first.checkpoint).expect("checkpoint serializes");
        let recovered: ProvisionCheckpoint =
            serde_json::from_str(&serialized).expect("checkpoint restores");
        let resumed = resume_workspace(&mut adapter, recovered);
        assert_eq!(resumed.state, ProvisionState::Completed);
        assert_eq!(adapter.created, 1);
        assert_eq!(
            adapter.calls,
            vec![
                ProvisionPhase::Authorize,
                ProvisionPhase::ProvisionWorkspace,
                ProvisionPhase::DiscoverConnection,
                ProvisionPhase::DiscoverConnection,
                ProvisionPhase::SaveConnection,
            ]
        );
    }

    #[test]
    fn uncertain_creation_cannot_be_retried_until_explicitly_reconciled() {
        let mut adapter = FakeAdapter::new().fail_at(
            ProvisionPhase::ProvisionWorkspace,
            AdapterFailure::Uncertain("复制请求结果未知".into()),
        );
        let first = provision_workspace(&mut adapter, request());
        assert_eq!(first.state, ProvisionState::NeedsReconciliation);

        let blocked = resume_workspace(&mut adapter, first.checkpoint.clone());
        assert_eq!(blocked.state, ProvisionState::NeedsReconciliation);
        assert_eq!(adapter.created, 0);

        let resolved = resolve_uncertain_creation(first.checkpoint, None);
        let finished = resume_workspace(&mut adapter, resolved);
        assert_eq!(finished.state, ProvisionState::Completed);
        assert_eq!(adapter.created, 1);
    }

    #[test]
    fn persisted_checkpoint_survives_restart_and_does_not_repeat_uncertain_create() {
        let storage = Storage::open(std::path::PathBuf::from(":memory:")).expect("storage opens");
        let mut adapter = FakeAdapter::new().fail_at(
            ProvisionPhase::ProvisionWorkspace,
            AdapterFailure::Uncertain("复制响应未能确认".into()),
        );
        let first = provision_workspace_persisted(&mut adapter, &storage, request());
        assert_eq!(first.state, ProvisionState::NeedsReconciliation);

        let persisted = get_persisted_checkpoint(&storage)
            .expect("checkpoint reads")
            .expect("checkpoint remains saved");
        let serialized = serde_json::to_string(&persisted).expect("checkpoint serializes");
        assert!(!serialized.contains("access_token"));
        assert!(persisted.needs_reconciliation);

        let resumed = resume_persisted_workspace(&mut adapter, &storage)
            .expect("resume reads")
            .expect("checkpoint exists");
        assert_eq!(resumed.state, ProvisionState::NeedsReconciliation);
        assert_eq!(adapter.created, 0);
    }

    #[test]
    fn restart_during_remote_copy_marks_the_result_uncertain_before_retry() {
        let storage = Storage::open(std::path::PathBuf::from(":memory:")).expect("storage opens");
        let mut checkpoint = new_checkpoint(request());
        checkpoint.phase = ProvisionPhase::ProvisionWorkspace;
        checkpoint.remote_mutation_started = true;
        persist_checkpoint(&storage, &checkpoint).expect("checkpoint persists");

        let mut adapter = FakeAdapter::new();
        let resumed = resume_persisted_workspace(&mut adapter, &storage)
            .expect("resume reads")
            .expect("checkpoint exists");
        assert_eq!(resumed.state, ProvisionState::NeedsReconciliation);
        assert_eq!(adapter.created, 0);
        assert!(
            get_persisted_checkpoint(&storage)
                .expect("checkpoint reads")
                .expect("checkpoint exists")
                .needs_reconciliation
        );
    }

    #[test]
    fn persisted_partial_failure_resumes_without_copying_a_second_workspace() {
        let storage = Storage::open(std::path::PathBuf::from(":memory:")).expect("storage opens");
        let mut adapter = FakeAdapter::new().fail_at(
            ProvisionPhase::DiscoverConnection,
            AdapterFailure::Retryable("暂时无法读取数据表".into()),
        );
        let first = provision_workspace_persisted(&mut adapter, &storage, request());
        assert_eq!(first.state, ProvisionState::PartialFailure);
        assert_eq!(adapter.created, 1);

        let resumed = resume_persisted_workspace(&mut adapter, &storage)
            .expect("resume reads")
            .expect("checkpoint exists");
        assert_eq!(resumed.state, ProvisionState::Completed);
        assert_eq!(adapter.created, 1);
        assert!(get_persisted_checkpoint(&storage)
            .expect("checkpoint reads")
            .is_none());
    }
}
