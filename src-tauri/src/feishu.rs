use std::{sync::OnceLock, time::Duration};

use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::storage::Task;

const API_ROOT: &str = "https://open.feishu.cn/open-apis";
#[cfg(all(test, windows, feature = "feishu-template-v2-acceptance"))]
#[path = "feishu_policy_diagnostic.rs"]
mod policy_diagnostic;
#[path = "feishu_sync_inspect.rs"]
mod sync_inspect;
pub use sync_inspect::{inspect as inspect_sync_with_user_token, SyncInspection, SyncRecordTime};
#[path = "feishu_sync_upload.rs"]
mod sync_upload;
pub(crate) use sync_upload::{prepare_user_sync_update,prepare_user_sync_restore,send_user_sync_update};
#[cfg(any(all(feature = "local-workbench-acceptance", feature = "feishu-template-v2-acceptance"),all(test,feature="feishu-direct-account-acceptance")))]
pub use sync_upload::{prepare_sync_update, send_sync_update};
#[cfg(all(feature = "local-workbench-acceptance", feature = "feishu-template-v2-acceptance"))]
pub use sync_upload::{attempt_sync_update, SyncUpdateAttempt};
#[cfg(all(test, feature = "feishu-direct-sync-ui-acceptance"))]
pub(crate) use sync_inspect::tests::fault_server as sync_test_server;
#[cfg(all(test, feature = "feishu-direct-sync-ui-acceptance"))]
pub(crate) fn sync_test_inspect_at(root: &str,config: &FeishuConfig,token: &str,now: i64) -> Result<SyncInspection,String> {
    sync_inspect::inspect_at(root,config,token,now)
}
#[cfg(all(test, feature = "feishu-direct-sync-ui-acceptance"))]
pub(crate) fn sync_test_attempt_at(
    root: &str, config: &FeishuConfig, token: &str, record_id: &str, content: &Value,
    status: &str, now: i64,
) -> SyncUpdateAttempt {
    sync_upload::attempt_sync_update_at(root,config,token,record_id,content,status,||Some(now))
}
#[cfg(not(any(feature = "feishu-release-ui-acceptance", feature = "feishu-direct-account-acceptance")))]
const CREDENTIAL_SERVICE: &str = "app.snaptask.desktop";
#[cfg(all(feature = "feishu-release-ui-acceptance", not(feature = "feishu-direct-account-acceptance")))]
const CREDENTIAL_SERVICE: &str = "app.snaptask.desktop.release060fresh";
#[cfg(feature = "feishu-direct-account-acceptance")]
const CREDENTIAL_SERVICE: &str = "app.snaptask.desktop.directfeishu20261020";
const CREDENTIAL_USER: &str = "feishu-app-secret";
#[cfg(any(not(feature = "feishu-template-v2-acceptance"),feature="feishu-direct-account-acceptance"))]
const USER_SESSION_CREDENTIAL: &str = "feishu-user-session-v1";
#[cfg(all(feature = "feishu-template-v2-acceptance",not(feature="feishu-direct-account-acceptance")))]
const USER_SESSION_CREDENTIAL: &str = "feishu-user-session-template-v2-acceptance-v1";
#[cfg(windows)]
const USER_SESSION_MANIFEST_PREFIX: &str = "project-track-feishu-chunks-v1:";
#[cfg(windows)]
const USER_SESSION_CHUNK_SIZE: usize = 2048;
#[cfg(windows)]
const USER_SESSION_MAX_CHUNKS: usize = 64;

pub fn validate_credential_profile(identifier: &str) -> Result<(), String> {
    let direct = cfg!(feature = "feishu-direct-account-acceptance");
    let direct_ui=cfg!(feature="feishu-direct-sync-ui-acceptance");
    let expected_direct=if direct_ui {crate::direct_account_scope::SYNC_IDENTIFIER}else{crate::direct_account_scope::AUTH_IDENTIFIER};
    let is_direct_profile=[crate::direct_account_scope::AUTH_IDENTIFIER,crate::direct_account_scope::SYNC_IDENTIFIER].contains(&identifier);
    if direct != is_direct_profile || (direct && identifier!=expected_direct)
        || (direct && (cfg!(feature = "feishu-release-ui-acceptance")
            || cfg!(feature = "feishu-write-acceptance")
            || (!direct_ui && (cfg!(feature = "local-workbench-acceptance")||cfg!(feature="feishu-template-v2-acceptance")))))
    {
        return Err("新飞书身份验收必须使用独立凭据空间，不能与正式或旧验收功能混用".into());
    }
    if cfg!(feature = "feishu-release-ui-acceptance")
        && (identifier != "app.snaptask.desktop.release060fresh"
            || cfg!(feature = "feishu-write-acceptance")
            || cfg!(feature = "feishu-template-v2-acceptance"))
    {
        return Err(
            "首次使用界面验收必须使用独立身份，且不得同时启用写入或模板专项验收功能".to_string(),
        );
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Deserialize, Serialize)]
struct FeishuSessionManifest {
    generation: String,
    count: usize,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveFeishuUserSessionInput {
    #[serde(default)]
    pub app_id: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub refresh_expires_in: Option<u64>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FeishuUserSessionRecord {
    #[serde(default = "legacy_user_auth_version")]
    auth_version: String,
    #[serde(default)]
    auth_app_id: Option<String>,
    access_token: String,
    refresh_token: String,
    access_expires_at: i64,
    refresh_expires_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuUserAuthStatus {
    pub authorized: bool,
    pub access_expires_at: Option<i64>,
    pub refresh_expires_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_token: String,
    pub table_id: String,
    pub secret_configured: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveFeishuConfigInput {
    pub app_id: String,
    pub app_token: String,
    pub table_id: String,
    pub app_secret: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuSyncResult {
    pub task_count: usize,
    pub completed_count: usize,
    pub table_id: String,
    pub fields: Vec<String>,
    pub write_ready: bool,
    pub undo_ready: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FeishuWriteRequest {
    Update {
        record_id: String,
        progress: u8,
        status: Option<String>,
        planned_date: Option<Option<String>>,
    },
    CreateTemporary {
        title: String,
        description: String,
        progress: u8,
        quadrant: String,
        today: bool,
        planned_date: Option<String>,
    },
    Delete {
        record_id: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuSchemaPlan {
    pub existing_fields: Vec<String>,
    pub missing_required_fields: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuSchemaInitResult {
    pub created_fields: Vec<String>,
    pub existing_fields: Vec<String>,
    pub missing_required_fields: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuWorkspaceStatus {
    pub existing_fields: Vec<String>,
    pub missing_required_fields: Vec<String>,
    pub status_is_formula: bool,
    pub views: Vec<FeishuWorkspaceItem>,
    pub dashboards: Vec<FeishuWorkspaceItem>,
    pub workflows: Vec<FeishuWorkspaceItem>,
    pub has_kanban: bool,
    pub inspection_warnings: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuWorkspaceItem {
    pub name: String,
    pub kind: String,
}

const REQUIRED_PROGRESS_WORKFLOWS: [&str; 3] = [
    "进度回退：恢复进行中",
    "进度完成：设为已完成",
    "进度归零：设为未开始",
];

fn progress_workflow_issues(
    workflows: &[FeishuWorkspaceItem],
    allow_legacy_absence: bool,
) -> Vec<String> {
    // Legacy connected tables may intentionally have no native status workflows. Once any
    // workflow from the v2 template is present, require the complete enabled set so a copied
    // template cannot be reported ready while Feishu has silently disabled its rules.
    if allow_legacy_absence
        && !workflows
            .iter()
            .any(|workflow| REQUIRED_PROGRESS_WORKFLOWS.contains(&workflow.name.as_str()))
    {
        return Vec::new();
    }
    let mut issues = Vec::new();
    for required_name in REQUIRED_PROGRESS_WORKFLOWS {
        match workflows
            .iter()
            .find(|workflow| workflow.name == required_name)
        {
            None => issues.push(format!("缺少“{required_name}”")),
            Some(workflow) if !workflow.kind.eq_ignore_ascii_case("enable") => {
                issues.push(format!("“{required_name}”未启用"));
            }
            Some(_) => {}
        }
    }
    issues
}

pub(crate) fn required_progress_workflow_issues(workflows: &[FeishuWorkspaceItem]) -> Vec<String> {
    progress_workflow_issues(workflows, true)
}

pub(crate) fn required_progress_workflow_issues_strict(
    workflows: &[FeishuWorkspaceItem],
) -> Vec<String> {
    progress_workflow_issues(workflows, false)
}

pub(crate) fn status_linkage_issues(
    status_is_formula: bool,
    workflows: &[FeishuWorkspaceItem],
    require_workflows: bool,
) -> Vec<String> {
    if status_is_formula {
        Vec::new()
    } else if require_workflows {
        required_progress_workflow_issues_strict(workflows)
    } else {
        required_progress_workflow_issues(workflows)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuWorkspaceInitResult {
    pub created_fields: Vec<String>,
    pub created_views: Vec<String>,
    pub status: FeishuWorkspaceStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuTemplateCopyResult {
    pub app_token: String,
    pub name: String,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuTaskTable {
    pub table_id: String,
    pub name: String,
    pub fields: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FeishuTemplateCopyError {
    Rejected(String),
    OutcomeUnknown(String),
}

/// Accept only a secure Feishu/Lark Drive folder link and return its opaque folder token.
/// Folder contents are never listed or read; the token is used only as the copy destination.
pub fn folder_token_from_link(value: &str) -> Result<String, String> {
    let url = Url::parse(value.trim()).map_err(|_| "请粘贴有效的飞书文件夹链接".to_string())?;
    let host = url
        .host_str()
        .ok_or_else(|| "请粘贴有效的飞书文件夹链接".to_string())?;
    let is_feishu_host = host == "feishu.cn"
        || host.ends_with(".feishu.cn")
        || host == "larksuite.com"
        || host.ends_with(".larksuite.com");
    if url.scheme() != "https" || !is_feishu_host {
        return Err("文件夹链接必须来自飞书或 Lark，并使用 HTTPS".to_string());
    }

    let segments = url
        .path_segments()
        .ok_or_else(|| "链接不是飞书云空间文件夹".to_string())?
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    let folder_token = segments
        .windows(3)
        .find(|parts| parts[0] == "drive" && parts[1] == "folder")
        .map(|parts| parts[2])
        .filter(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        })
        .ok_or_else(|| "链接中没有可识别的飞书文件夹".to_string())?;
    Ok(folder_token.to_string())
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    code: i64,
    msg: String,
    tenant_access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AppAccessTokenResponse {
    code: i64,
    #[serde(default)]
    app_access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RefreshedUserTokenData {
    access_token: String,
    expires_in: u64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    refresh_token_expires_in: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct RefreshedUserTokenV2Response {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
    #[serde(default)]
    refresh_token_expires_in: Option<u64>,
    #[serde(default)]
    error: Option<String>,
}

fn legacy_user_auth_version() -> String {
    "v1".to_string()
}

fn validate_session_app_id(session: &FeishuUserSessionRecord, app_id: &str) -> Result<(), String> {
    if session.auth_version == "v2" && session.auth_app_id.as_deref() != Some(app_id) {
        return Err("飞书授权对应的应用已变化，请重新连接飞书账号".to_string());
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct RefreshedUserTokenResponse {
    code: i64,
    #[serde(default)]
    data: Option<RefreshedUserTokenData>,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    code: i64,
    msg: String,
    data: Option<T>,
    #[serde(default)]
    error: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct FieldData {
    #[serde(default)]
    items: Vec<FieldItem>,
    #[serde(default)]
    has_more: bool,
    page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TableData {
    #[serde(default)]
    items: Vec<TableItem>,
    #[serde(default)]
    has_more: bool,
    page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TableItem {
    table_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct CopiedAppData {
    app: Option<CopiedApp>,
}

#[derive(Debug, Deserialize)]
struct CopiedApp {
    app_token: String,
    name: String,
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FieldItem {
    field_name: String,
    #[serde(default, rename = "type")]
    field_type: i64,
    #[serde(default)]
    ui_type: String,
    #[serde(default)]
    property: Value,
}

fn is_formula_field(field: &FieldItem) -> bool {
    field.field_type == 20
        || field.ui_type.eq_ignore_ascii_case("formula")
        || field.property.get("formula_expression").is_some()
        || field.property.get("formula").is_some()
}

fn has_formula_status_field(fields: &[FieldItem]) -> bool {
    field_named(fields, &["状态"])
        .map(is_formula_field)
        .unwrap_or(false)
}

#[derive(Debug, Deserialize)]
struct ViewData {
    #[serde(default)]
    items: Vec<ViewItem>,
    #[serde(default)]
    has_more: bool,
    page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ViewItem {
    #[serde(default)]
    view_name: String,
    #[serde(default)]
    view_type: String,
}

#[derive(Debug, Deserialize)]
struct DashboardData {
    #[serde(default)]
    dashboards: Vec<DashboardItem>,
    #[serde(default)]
    has_more: bool,
    page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DashboardItem {
    #[serde(default)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct WorkflowData {
    #[serde(default)]
    workflows: Vec<WorkflowItem>,
}

#[derive(Debug, Deserialize)]
struct WorkflowItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct RecordData {
    #[serde(default)]
    items: Vec<RecordItem>,
    #[serde(default)]
    has_more: bool,
    page_token: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct RecordItem {
    record_id: String,
    fields: serde_json::Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct RecordMutationData {
    record: Option<RecordItem>,
}

pub fn validate_config(input: &SaveFeishuConfigInput) -> Result<(String, String, String), String> {
    let app_id = input.app_id.trim();
    let app_token = input.app_token.trim();
    let table_id = input.table_id.trim();
    if !app_id.starts_with("cli_") {
        return Err("飞书 App ID 应以 cli_ 开头".to_string());
    }
    if app_token.is_empty() || !app_token.chars().all(|value| value.is_ascii_alphanumeric()) {
        return Err("请填写多维表格 App Token（链接中 base/ 后的标识）".to_string());
    }
    if !table_id.starts_with("tbl") {
        return Err("飞书数据表 ID 应以 tbl 开头".to_string());
    }
    Ok((
        app_id.to_string(),
        app_token.to_string(),
        table_id.to_string(),
    ))
}

#[cfg(windows)]
pub fn save_secret(secret: &str) -> Result<(), String> {
    let secret = secret.trim();
    if secret.is_empty() {
        return Err("飞书 App Secret 不能为空".to_string());
    }
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .map_err(|error| format!("无法打开 Windows 凭据库：{error}"))?
        .set_password(secret)
        .map_err(|error| format!("无法保存飞书 App Secret：{error}"))
}

#[cfg(windows)]
pub fn save_oauth_app_credentials(app_id: &str, secret: &str) -> Result<(), String> {
    let app_id = app_id.trim();
    let secret = secret.trim();
    if !app_id.starts_with("cli_") || secret.is_empty() {
        return Err("飞书应用凭据不完整，无法安全保存".to_string());
    }
    keyring::Entry::new(
        CREDENTIAL_SERVICE,
        &format!("feishu-oauth-app-secret-{app_id}"),
    )
    .map_err(|_| "无法打开 Windows 凭据库".to_string())?
    .set_password(secret)
    .map_err(|_| "无法将飞书授权应用密钥保存到 Windows 凭据库".to_string())
}

#[cfg(not(windows))]
pub fn save_oauth_app_credentials(_app_id: &str, _secret: &str) -> Result<(), String> {
    Err("当前版本仅支持将飞书应用密钥保存到 Windows 凭据库".to_string())
}

#[cfg(windows)]
pub fn read_oauth_app_secret(app_id: &str) -> Result<String, String> {
    keyring::Entry::new(
        CREDENTIAL_SERVICE,
        &format!("feishu-oauth-app-secret-{}", app_id.trim()),
    )
    .and_then(|entry| entry.get_password())
    .map_err(|_| "尚未在 Windows 凭据库中保存此飞书授权应用密钥".to_string())
}

#[cfg(not(windows))]
pub fn read_oauth_app_secret(_app_id: &str) -> Result<String, String> {
    Err("当前版本仅支持从 Windows 凭据库读取飞书授权应用密钥".to_string())
}

#[cfg(not(windows))]
pub fn save_secret(_secret: &str) -> Result<(), String> {
    Err("当前版本仅支持将飞书 App Secret 保存到 Windows 凭据库".to_string())
}

#[cfg(windows)]
pub fn read_secret() -> Result<String, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .and_then(|entry| entry.get_password())
        .map_err(|_| "尚未在 Windows 凭据库中保存飞书 App Secret".to_string())
}

#[cfg(not(windows))]
pub fn read_secret() -> Result<String, String> {
    Err("当前版本仅支持从 Windows 凭据库读取飞书 App Secret".to_string())
}

#[cfg(windows)]
pub fn save_user_session(input: &SaveFeishuUserSessionInput) -> Result<(), String> {
    save_user_session_with_version(input, "v1")
}

#[cfg(windows)]
pub fn save_user_session_v2(input: &SaveFeishuUserSessionInput) -> Result<(), String> {
    save_user_session_with_version(input, "v2")
}

#[cfg(windows)]
fn save_user_session_with_version(
    input: &SaveFeishuUserSessionInput,
    auth_version: &str,
) -> Result<(), String> {
    save_user_session_with_version_at(input, auth_version, USER_SESSION_CREDENTIAL)
}

#[cfg(windows)]
fn save_user_session_with_version_at(
    input: &SaveFeishuUserSessionInput,
    auth_version: &str,
    credential_user: &str,
) -> Result<(), String> {
    let access_token = input.access_token.trim();
    let refresh_token = input.refresh_token.trim();
    if access_token.is_empty() || refresh_token.is_empty() || input.expires_in == 0 {
        return Err("飞书授权返回的信息不完整，请重新授权".to_string());
    }
    let auth_app_id = if auth_version == "v2" {
        Some(
            input
                .app_id
                .as_deref()
                .map(str::trim)
                .filter(|value| value.starts_with("cli_"))
                .ok_or_else(|| "飞书授权缺少应用标识，请重新连接".to_string())?
                .to_string(),
        )
    } else {
        input
            .app_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };
    let now = crate::storage::unix_now();
    let record = FeishuUserSessionRecord {
        auth_version: auth_version.to_string(),
        auth_app_id,
        access_token: access_token.to_string(),
        refresh_token: refresh_token.to_string(),
        access_expires_at: now.saturating_add(input.expires_in.min(i64::MAX as u64) as i64),
        refresh_expires_at: input
            .refresh_expires_in
            .map(|duration| now.saturating_add(duration.min(i64::MAX as u64) as i64)),
    };
    store_user_session_record_at(&record, credential_user)
}

#[cfg(windows)]
fn session_chunk_user(credential_user: &str, generation: &str, index: usize) -> String {
    format!("{credential_user}-chunk-{generation}-{index}")
}

#[cfg(windows)]
fn parse_session_manifest(value: &str) -> Result<Option<FeishuSessionManifest>, String> {
    let Some(value) = value.strip_prefix(USER_SESSION_MANIFEST_PREFIX) else {
        return Ok(None);
    };
    let manifest: FeishuSessionManifest =
        serde_json::from_str(value).map_err(|_| "飞书授权信息损坏，请重新授权".to_string())?;
    if manifest.count == 0
        || manifest.count > USER_SESSION_MAX_CHUNKS
        || manifest.generation.is_empty()
        || manifest.generation.len() > 64
        || !manifest
            .generation
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("飞书授权信息损坏，请重新授权".to_string());
    }
    Ok(Some(manifest))
}

#[cfg(windows)]
fn next_session_generation() -> String {
    use std::{
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!(
        "{now:x}{:x}{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(windows)]
fn remove_session_chunks(credential_user: &str, manifest: &FeishuSessionManifest) -> bool {
    let mut removed_all = true;
    for index in 0..manifest.count {
        if keyring::Entry::new(
            CREDENTIAL_SERVICE,
            &session_chunk_user(credential_user, &manifest.generation, index),
        )
        .and_then(|entry| entry.delete_credential())
        .is_err()
        {
            removed_all = false;
        }
    }
    removed_all
}

#[cfg(windows)]
fn session_store_error(error: keyring::Error) -> String {
    match error {
        keyring::Error::TooLong(..) => "飞书授权信息超过 Windows 单条凭据的长度限制".to_string(),
        keyring::Error::NoStorageAccess(_) => {
            "Windows 凭据库暂不可访问，请解锁 Windows 后重试".to_string()
        }
        keyring::Error::PlatformFailure(_) => "Windows 凭据库拒绝写入飞书授权信息".to_string(),
        _ => "无法将飞书授权信息保存到 Windows 凭据库".to_string(),
    }
}

#[cfg(windows)]
fn store_user_session_record_at(
    record: &FeishuUserSessionRecord,
    credential_user: &str,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(record).map_err(|_| "无法安全保存飞书授权信息".to_string())?;
    let chunks = bytes.chunks(USER_SESSION_CHUNK_SIZE).collect::<Vec<_>>();
    if chunks.is_empty() || chunks.len() > USER_SESSION_MAX_CHUNKS {
        return Err("飞书授权信息超出 Windows 凭据库容量，请重新授权或联系开发者".to_string());
    }
    let entry =
        keyring::Entry::new(CREDENTIAL_SERVICE, credential_user).map_err(session_store_error)?;
    let previous = match entry.get_password() {
        Ok(value) => parse_session_manifest(&value)?,
        Err(keyring::Error::NoEntry) => None,
        Err(error) => return Err(session_store_error(error)),
    };
    let manifest = FeishuSessionManifest {
        generation: next_session_generation(),
        count: chunks.len(),
    };
    for (index, chunk) in chunks.iter().enumerate() {
        let result = keyring::Entry::new(
            CREDENTIAL_SERVICE,
            &session_chunk_user(credential_user, &manifest.generation, index),
        )
        .and_then(|entry| entry.set_secret(chunk));
        if let Err(error) = result {
            let partial = FeishuSessionManifest {
                generation: manifest.generation.clone(),
                count: index,
            };
            remove_session_chunks(credential_user, &partial);
            return Err(session_store_error(error));
        }
    }
    let marker = format!(
        "{USER_SESSION_MANIFEST_PREFIX}{}",
        serde_json::to_string(&manifest).map_err(|_| "无法安全保存飞书授权信息".to_string())?
    );
    if let Err(error) = entry.set_password(&marker) {
        remove_session_chunks(credential_user, &manifest);
        return Err(session_store_error(error));
    }
    if let Some(previous) = previous {
        remove_session_chunks(credential_user, &previous);
    }
    Ok(())
}

#[cfg(windows)]
fn read_user_session_at(credential_user: &str) -> Result<FeishuUserSessionRecord, String> {
    let serialized = keyring::Entry::new(CREDENTIAL_SERVICE, credential_user)
        .and_then(|entry| entry.get_password())
        .map_err(|_| "尚未完成飞书账号授权".to_string())?;
    let Some(manifest) = parse_session_manifest(&serialized)? else {
        return serde_json::from_str(&serialized)
            .map_err(|_| "飞书授权信息损坏，请重新授权".to_string());
    };
    let mut bytes = Vec::new();
    for index in 0..manifest.count {
        let chunk = keyring::Entry::new(
            CREDENTIAL_SERVICE,
            &session_chunk_user(credential_user, &manifest.generation, index),
        )
        .and_then(|entry| entry.get_secret())
        .map_err(|_| "飞书授权信息不完整，请重新授权".to_string())?;
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "飞书授权信息损坏，请重新授权".to_string())
}

#[cfg(not(windows))]
pub fn save_user_session(_input: &SaveFeishuUserSessionInput) -> Result<(), String> {
    Err("当前版本仅支持将飞书授权信息保存到 Windows 凭据库".to_string())
}

#[cfg(not(windows))]
pub fn save_user_session_v2(_input: &SaveFeishuUserSessionInput) -> Result<(), String> {
    Err("当前版本仅支持将飞书授权信息保存到 Windows 凭据库".to_string())
}

#[cfg(windows)]
fn read_user_session() -> Result<FeishuUserSessionRecord, String> {
    read_user_session_at(USER_SESSION_CREDENTIAL)
}

#[cfg(not(windows))]
fn read_user_session() -> Result<FeishuUserSessionRecord, String> {
    Err("当前版本仅支持从 Windows 凭据库读取飞书授权信息".to_string())
}

pub fn read_user_access_token(app_id: &str) -> Result<String, String> {
    let mut session = read_user_session()?;
    let app_id = app_id.trim();
    if app_id.is_empty() {
        return Err("飞书应用标识缺失，无法安全刷新用户授权".to_string());
    }
    validate_session_app_id(&session, app_id)?;
    let now = crate::storage::unix_now();
    if session.access_expires_at > now.saturating_add(30) {
        return Ok(session.access_token);
    }
    if session
        .refresh_expires_at
        .is_some_and(|expires_at| expires_at <= now.saturating_add(30))
    {
        return Err("飞书用户授权已过期，请重新授权".to_string());
    }

    let refreshed = if session.auth_version == "v2" {
        let app_secret = read_oauth_app_secret(app_id)?;
        refresh_user_access_token_v2_at(API_ROOT, app_id, &app_secret, &session.refresh_token)?
    } else {
        let app_secret = read_secret()?;
        refresh_user_access_token_at(API_ROOT, app_id, &app_secret, &session.refresh_token)?
    };
    let previous_refresh_expiry = session.refresh_expires_at;
    session.access_token = refreshed.access_token;
    session.access_expires_at =
        now.saturating_add(refreshed.expires_in.min(i64::MAX as u64) as i64);
    if let Some(refresh_token) = refreshed.refresh_token {
        session.refresh_token = refresh_token;
    }
    session.refresh_expires_at = refreshed
        .refresh_token_expires_in
        .map(|duration| now.saturating_add(duration.min(i64::MAX as u64) as i64))
        .or(previous_refresh_expiry);
    if session.auth_version == "v2" {
        session.auth_app_id = Some(app_id.to_string());
    }
    #[cfg(windows)]
    store_user_session_record_at(&session, USER_SESSION_CREDENTIAL)?;
    #[cfg(not(windows))]
    return Err("当前版本仅支持在 Windows 凭据库中刷新飞书授权信息".to_string());

    Ok(session.access_token)
}

fn refresh_user_access_token_v2_at(
    api_root: &str,
    app_id: &str,
    app_secret: &str,
    refresh_token: &str,
) -> Result<RefreshedUserTokenData, String> {
    let response = client()?
        .post(format!("{api_root}/authen/v2/oauth/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", app_id),
            ("client_secret", app_secret),
            ("refresh_token", refresh_token),
        ])
        .send()
        .map_err(|_| "无法连接飞书刷新用户授权，请检查网络".to_string())?;
    let status = response.status();
    let payload = response
        .json::<RefreshedUserTokenV2Response>()
        .map_err(|_| "飞书用户授权刷新响应无法解析".to_string())?;
    if !status.is_success() || payload.error.is_some() {
        return Err("飞书用户授权刷新失败，请重新授权".to_string());
    }
    let access_token = payload
        .access_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "飞书授权刷新成功但未返回访问令牌".to_string())?;
    let expires_in = payload
        .expires_in
        .filter(|duration| *duration > 0)
        .ok_or_else(|| "飞书授权刷新未返回有效期限".to_string())?;
    Ok(RefreshedUserTokenData {
        access_token,
        expires_in,
        refresh_token: payload.refresh_token,
        refresh_token_expires_in: payload.refresh_token_expires_in,
    })
}

fn refresh_user_access_token_at(
    api_root: &str,
    app_id: &str,
    app_secret: &str,
    refresh_token: &str,
) -> Result<RefreshedUserTokenData, String> {
    let client = client()?;
    let app_response = client
        .post(format!("{api_root}/auth/v3/app_access_token/internal"))
        .json(&json!({ "app_id": app_id, "app_secret": app_secret }))
        .send()
        .map_err(|_| "无法连接飞书刷新授权，请检查网络".to_string())?;
    let app_status = app_response.status();
    let app_data = app_response
        .json::<AppAccessTokenResponse>()
        .map_err(|_| "飞书应用授权响应无法解析".to_string())?;
    if !app_status.is_success() || app_data.code != 0 {
        return Err("无法获取飞书应用授权以刷新用户会话，请检查应用凭据".to_string());
    }
    let app_access_token = app_data
        .app_access_token
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "飞书应用授权响应缺少访问令牌".to_string())?;

    let response = client
        .post(format!("{api_root}/authen/v1/refresh_access_token"))
        .bearer_auth(app_access_token)
        .json(&json!({ "grant_type": "refresh_token", "refresh_token": refresh_token }))
        .send()
        .map_err(|_| "无法连接飞书刷新用户授权，请检查网络".to_string())?;
    let status = response.status();
    let payload = response
        .json::<RefreshedUserTokenResponse>()
        .map_err(|_| "飞书用户授权刷新响应无法解析".to_string())?;
    if !status.is_success() || payload.code != 0 {
        return Err("飞书用户授权刷新失败，请重新授权".to_string());
    }
    let data = payload
        .data
        .ok_or_else(|| "飞书授权刷新成功但未返回会话信息".to_string())?;
    if data.access_token.trim().is_empty() || data.expires_in == 0 {
        return Err("飞书授权刷新未返回有效会话".to_string());
    }
    Ok(data)
}

pub fn user_auth_status() -> FeishuUserAuthStatus {
    match read_user_session() {
        Ok(session) => {
            let now = crate::storage::unix_now();
            FeishuUserAuthStatus {
                authorized: session.access_expires_at > now
                    || session
                        .refresh_expires_at
                        .is_some_and(|expires_at| expires_at > now),
                access_expires_at: Some(session.access_expires_at),
                refresh_expires_at: session.refresh_expires_at,
            }
        }
        Err(_) => FeishuUserAuthStatus::default(),
    }
}

pub fn user_authorized_for_app(app_id: &str) -> bool {
    if app_id.trim().is_empty() {
        return false;
    }
    match read_user_session() {
        Ok(session) => session_authorized_for_app(&session, app_id, crate::storage::unix_now()),
        Err(_) => false,
    }
}

fn session_authorized_for_app(session: &FeishuUserSessionRecord, app_id: &str, now: i64) -> bool {
    !app_id.trim().is_empty()
        && validate_session_app_id(session, app_id).is_ok()
        && (session.access_expires_at > now
            || session
                .refresh_expires_at
                .is_some_and(|expiry| expiry > now))
}

/// Returns whether a user session is present, including one that now needs refresh.
/// A present-but-expired session must not silently fall back to app-level access.
pub fn has_user_session() -> bool {
    read_user_session().is_ok()
}

#[cfg(windows)]
pub fn clear_user_session() -> Result<(), String> {
    clear_user_session_at(USER_SESSION_CREDENTIAL)
}

#[cfg(windows)]
fn clear_user_session_at(credential_user: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(CREDENTIAL_SERVICE, credential_user)
        .map_err(|_| "无法打开 Windows 凭据库".to_string())?;
    let manifest = match entry.get_password() {
        Ok(value) => parse_session_manifest(&value)?,
        Err(keyring::Error::NoEntry) => return Ok(()),
        Err(_) => return Err("无法读取 Windows 凭据库中的飞书授权信息".to_string()),
    };
    entry
        .delete_credential()
        .map_err(|_| "无法清除 Windows 凭据库中的飞书授权信息".to_string())?;
    if let Some(manifest) = manifest {
        if !remove_session_chunks(credential_user, &manifest) {
            return Err("飞书授权已断开，但部分旧凭据清理失败，请检查 Windows 凭据库".to_string());
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn clear_user_session() -> Result<(), String> {
    Err("当前版本仅支持从 Windows 凭据库清除飞书授权信息".to_string())
}

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(25))
        .user_agent("project-track/0.1")
        .build()
        .map_err(|error| format!("无法初始化飞书连接：{error}"))
}

fn tenant_access_token(config: &FeishuConfig, secret: &str) -> Result<String, String> {
    let response = client()?
        .post(format!("{API_ROOT}/auth/v3/tenant_access_token/internal"))
        .json(&json!({ "app_id": config.app_id, "app_secret": secret }))
        .send()
        .map_err(|error| format!("无法连接飞书：{error}"))?;
    let status = response.status();
    let payload = response
        .json::<TokenResponse>()
        .map_err(|error| format!("飞书鉴权响应无法解析：{error}"))?;
    if !status.is_success() || payload.code != 0 {
        return Err(format!(
            "飞书鉴权失败：{}（代码 {}）",
            payload.msg, payload.code
        ));
    }
    payload
        .tenant_access_token
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "飞书鉴权成功但未返回访问令牌".to_string())
}

/// Resolve the credential for normal Feishu operations. User-authorized access is preferred;
/// legacy app credentials remain available only when no user session has been saved.
pub fn read_active_access_token(config: &FeishuConfig) -> Result<String, String> {
    if cfg!(feature = "feishu-write-acceptance") {
        crate::write_acceptance::ensure_task_write(config)?;
        return read_user_access_token(&config.app_id);
    }
    if has_user_session() {
        return read_user_access_token(&config.app_id);
    }
    let secret = read_secret()?;
    tenant_access_token(config, &secret)
}

fn parse_api_response<T: for<'de> Deserialize<'de>>(
    response: reqwest::blocking::Response,
    action: &str,
) -> Result<T, String> {
    let status = response.status();
    let payload = response
        .json::<ApiResponse<T>>()
        .map_err(|error| format!("飞书{action}响应无法解析：{error}"))?;
    if !status.is_success() || payload.code != 0 {
        return Err(format_api_error(action, &payload));
    }
    payload
        .data
        .ok_or_else(|| format!("飞书{action}成功但未返回数据"))
}

fn format_api_error<T>(action: &str, payload: &ApiResponse<T>) -> String {
    let mut message = format!("飞书{action}失败：{}（代码 {}）", payload.msg, payload.code);

    if payload.code == 91403 {
        let guidance = if action == "读取自动化" {
            "。当前应用无法读取多维表格自动化配置；这不影响任务字段、记录、视图或仪表盘读取。若要检查自动化详情，请核对应用对应的自动化读取权限及已发布版本"
        } else if action.contains("写入")
            || action.contains("创建记录")
            || action.contains("更新记录")
            || action.contains("删除记录")
        {
            "。当前应用尚未获得此表格的记录写入权限；请核对多维表格记录新增/更新/删除权限、该表格的协作者编辑权限及已发布版本"
        } else {
            "。当前应用无法读取此项多维表格数据；请核对应用的数据访问范围、文档协作者读取权限及已发布版本"
        };
        message.push_str(guidance);
    }

    if let Some(error) = payload.error.as_ref() {
        let subjects = error
            .get("permission_violations")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|violation| violation.get("subject").and_then(Value::as_str))
            .filter(|subject| !subject.trim().is_empty())
            .collect::<Vec<_>>();
        if !subjects.is_empty() {
            message.push_str(&format!("；缺失权限：{}", subjects.join("、")));
        }
        if let Some(log_id) = error.get("log_id").and_then(Value::as_str) {
            if !log_id.trim().is_empty() {
                message.push_str(&format!("；飞书诊断编号：{log_id}"));
            }
        }
    }

    message
}

fn list_field_items(config: &FeishuConfig, access_token: &str) -> Result<Vec<FieldItem>, String> {
    list_field_items_at(API_ROOT, &config.app_token, &config.table_id, access_token)
}

fn list_field_items_at(
    api_root: &str,
    app_token: &str,
    table_id: &str,
    access_token: &str,
) -> Result<Vec<FieldItem>, String> {
    let mut fields = Vec::new();
    let mut page_token = None::<String>;
    loop {
        let mut url = Url::parse(&format!(
            "{api_root}/bitable/v1/apps/{app_token}/tables/{table_id}/fields"
        ))
        .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "100");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: FieldData = parse_api_response(response, "读取字段")?;
        fields.extend(data.items);
        if !data.has_more {
            break;
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书字段分页缺少 page_token".to_string());
        }
    }
    Ok(fields)
}

fn list_fields(config: &FeishuConfig, access_token: &str) -> Result<Vec<String>, String> {
    Ok(list_field_items(config, access_token)?
        .into_iter()
        .map(|item| item.field_name)
        .collect())
}

fn missing_required_fields(fields: &[String]) -> Vec<String> {
    required_task_fields()
        .iter()
        .filter(|(name, aliases)| {
            !std::iter::once(name)
                .chain(aliases.iter())
                .any(|alias| fields.iter().any(|field| field == alias))
        })
        .map(|(name, _)| name.clone())
        .collect()
}

fn required_task_fields() -> &'static [(String, Vec<String>)] {
    static REQUIRED_FIELDS: OnceLock<Vec<(String, Vec<String>)>> = OnceLock::new();
    REQUIRED_FIELDS.get_or_init(|| template_task_fields(false))
}

fn template_task_fields(initializing: bool) -> Vec<(String, Vec<String>)> {
        let manifest: Value =
            serde_json::from_str(include_str!("../resources/task-workspace-template.v1.json"))
                .expect("embedded task workspace template manifest must be valid JSON");
        manifest["taskTable"]["fields"]
            .as_array()
            .expect("embedded task workspace manifest must include fields")
            .iter()
            .filter(|field| field["required"].as_bool() == Some(true)
                || (initializing && field["initializationRequired"].as_bool() == Some(true)))
            .map(|field| {
                let name = field["name"]
                    .as_str()
                    .expect("required field must have a name")
                    .to_string();
                let aliases = field["aliases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect();
                (name, aliases)
            })
            .collect()
}

fn missing_initialization_fields(fields: &[FieldItem]) -> Result<Vec<String>, String> {
    // Do not rename, convert, or shadow an existing user column of the wrong type.
    if let Some(date) = field_named(fields, &["任务计划完成日期", "任务计划完成时间", "计划完成日期", "截止日期"]) {
        if date.field_type != 5 && !["date", "datetime"].contains(&date.ui_type.to_ascii_lowercase().as_str()) {
            return Err(format!("已有字段“{}”不是日期类型，请手动核对；未新增重复日期列，也未更改现有列", date.field_name));
        }
    }
    if let Some(date) = field_named(fields, &["开始日期", "任务开始日期"]) {
        if date.field_type != 5 && !["date", "datetime"].contains(&date.ui_type.to_ascii_lowercase().as_str()) {
            return Err(format!("已有字段“{}”不是日期类型，请手动核对；未新增重复开始日期列", date.field_name));
        }
    }
    if let Some(today) = field_named(fields, &["今日必完成"]) {
        if today.field_type != 7 {
            return Err("已有字段“今日必完成”不是复选框类型，请手动核对；未新增重复列".into());
        }
    }
    Ok(template_task_fields(true).iter().filter(|(name, aliases)| {
        !std::iter::once(name).chain(aliases.iter()).any(|n| fields.iter().any(|f| &f.field_name==n))
    }).map(|(name, _)| name.clone()).collect())
}

fn required_field_spec(field_name: &str) -> Option<Value> {
    match field_name {
        "任务名称" => Some(json!({
            "field_name": "任务名称",
            "type": 1
        })),
        "任务类型" => Some(json!({
            "field_name": "任务类型",
            "type": 3,
            "property": {
                "options": [
                    { "name": "重要且紧急" },
                    { "name": "重要不紧急" },
                    { "name": "紧急不重要" },
                    { "name": "不重要不紧急" }
                ]
            }
        })),
        "状态" => Some(json!({
            "field_name": "状态",
            "type": 3,
            "property": {
                "options": [
                    { "name": "未开始" },
                    { "name": "进行中" },
                    { "name": "已完成" }
                ]
            }
        })),
        "进度" => Some(json!({
            "field_name": "进度",
            "type": 2,
            "property": { "formatter": "0" }
        })),
        "计划完成日期" => Some(json!({
            "field_name": "计划完成日期", "type": 5,
            "property": { "date_formatter": "yyyy/MM/dd", "auto_fill": false }
        })),
        "开始日期" => Some(json!({
            "field_name": "开始日期", "type": 5,
            "property": { "date_formatter": "yyyy/MM/dd", "auto_fill": false }
        })),
        "今日必完成" => Some(json!({ "field_name": "今日必完成", "type": 7 })),
        _ => None,
    }
}

pub fn inspect_schema_with_access_token(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<FeishuSchemaPlan, String> {
    let items = list_field_items(config, access_token)?;
    Ok(FeishuSchemaPlan {
        missing_required_fields: missing_initialization_fields(&items)?,
        existing_fields: items.into_iter().map(|f| f.field_name).collect(),
    })
}

pub fn create_missing_required_fields_with_access_token(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<FeishuSchemaInitResult, String> {
    create_initialization_fields_at(API_ROOT, config, access_token)
}

fn create_initialization_fields_at(root: &str, config: &FeishuConfig, access_token: &str) -> Result<FeishuSchemaInitResult, String> {
    let before = list_field_items_at(root, &config.app_token, &config.table_id, access_token)?;
    let missing = missing_initialization_fields(&before)?;
    let mut created_fields = Vec::new();

    for field_name in missing {
        let spec = required_field_spec(&field_name)
            .ok_or_else(|| format!("不支持自动创建字段：{field_name}"))?;
        let url = format!(
            "{root}/bitable/v1/apps/{}/tables/{}/fields",
            config.app_token, config.table_id
        );
        let response = client()?
            .post(url)
            .bearer_auth(&access_token)
            .json(&spec)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        if let Err(error) =
            parse_api_response::<Value>(response, &format!("创建字段“{field_name}”"))
        {
            let prefix = if created_fields.is_empty() {
                String::new()
            } else {
                format!("已成功新增 {}；", created_fields.join("、"))
            };
            return Err(format!("{prefix}{error}"));
        }
        created_fields.push(field_name);
    }

    let fields = list_field_items_at(root, &config.app_token, &config.table_id, access_token)?;
    let remaining = missing_initialization_fields(&fields)?;
    if !remaining.is_empty() {
        return Err(format!("字段创建结果未完成回读核实：{}；请重新检查字段，不要重复创建", remaining.join("、")));
    }
    for name in ["计划完成日期", "开始日期"] {
        if created_fields.iter().any(|n| n == name) && field_named(&fields, &[name])
            .and_then(|f| f.property.get("auto_fill")).and_then(Value::as_bool) != Some(false) {
            return Err(format!("{name}列已创建，但未核实自动填入已关闭；请检查字段设置，不要重复建列"));
        }
    }
    Ok(FeishuSchemaInitResult {
        missing_required_fields: remaining,
        existing_fields: fields.into_iter().map(|f| f.field_name).collect(),
        created_fields,
    })
}

/// Runs only for an explicitly created, still-empty copy, outside copy-readiness retries.
pub fn ensure_new_workspace_initialization_fields(app_token: &str, table_id: &str, user_token: &str) -> Result<(), String> {
    ensure_task_table_empty(app_token, table_id, user_token)?;
    let config = FeishuConfig { app_id: String::new(), app_token: app_token.into(), table_id: table_id.into(), secret_configured: false };
    create_missing_required_fields_with_access_token(&config, user_token)?;
    let fields = list_field_items(&config, user_token)?;
    if field_named(&fields, &["任务计划完成日期", "任务计划完成时间", "计划完成日期", "截止日期"])
        .and_then(|f| f.property.get("auto_fill")).and_then(Value::as_bool) == Some(true) {
        return Err("复制后的日期列开启了自动填入创建时间；请先在该副本关闭，再继续初始化，不要重新复制".into());
    }
    Ok(())
}

fn list_views(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<Vec<FeishuWorkspaceItem>, String> {
    let mut views = Vec::new();
    let mut page_token = None::<String>;
    loop {
        let mut url = Url::parse(&format!(
            "{API_ROOT}/bitable/v1/apps/{}/tables/{}/views",
            config.app_token, config.table_id
        ))
        .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "100");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: ViewData = parse_api_response(response, "读取视图")?;
        views.extend(data.items.into_iter().map(|item| FeishuWorkspaceItem {
            name: item.view_name,
            kind: item.view_type,
        }));
        if !data.has_more {
            break;
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书视图分页缺少 page_token".to_string());
        }
    }
    Ok(views)
}

fn list_dashboards(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<Vec<FeishuWorkspaceItem>, String> {
    let mut dashboards = Vec::new();
    let mut page_token = None::<String>;
    loop {
        let mut url = Url::parse(&format!(
            "{API_ROOT}/bitable/v1/apps/{}/dashboards",
            config.app_token
        ))
        .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "100");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: DashboardData = parse_api_response(response, "读取仪表盘")?;
        dashboards.extend(data.dashboards.into_iter().map(|item| FeishuWorkspaceItem {
            name: item.name,
            kind: "dashboard".to_string(),
        }));
        if !data.has_more {
            break;
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书仪表盘分页缺少 page_token".to_string());
        }
    }
    Ok(dashboards)
}

fn list_workflows(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<Vec<FeishuWorkspaceItem>, String> {
    let response = client()?
        .get(format!(
            "{API_ROOT}/bitable/v1/apps/{}/workflows",
            config.app_token
        ))
        .bearer_auth(access_token)
        .send()
        .map_err(|error| format!("无法连接飞书：{error}"))?;
    let data: WorkflowData = parse_api_response(response, "读取自动化")?;
    Ok(data
        .workflows
        .into_iter()
        .map(|item| FeishuWorkspaceItem {
            name: item.title,
            kind: item.status,
        })
        .collect())
}

fn inspect_workspace_with_token(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<FeishuWorkspaceStatus, String> {
    let field_items = list_field_items(config, access_token)?;
    let formula_status = has_formula_status_field(&field_items);
    let initialization_missing = missing_initialization_fields(&field_items)?;
    let fields = field_items
        .into_iter()
        .map(|item| item.field_name)
        .collect::<Vec<_>>();
    let mut warnings = Vec::new();
    let views = match list_views(config, access_token) {
        Ok(items) => items,
        Err(error) => {
            warnings.push(error);
            Vec::new()
        }
    };
    let dashboards = match list_dashboards(config, access_token) {
        Ok(items) => items,
        Err(error) => {
            warnings.push(error);
            Vec::new()
        }
    };
    let workflows =
        match inspect_status_workflows(formula_status, || list_workflows(config, access_token)) {
            Ok(items) => {
                let workflow_issues = status_linkage_issues(
                    formula_status,
                    &items,
                    cfg!(feature = "feishu-template-v2-acceptance"),
                );
                if !workflow_issues.is_empty() {
                    warnings.push(format!(
                        "状态联动自动化未就绪：{}",
                        workflow_issues.join("；")
                    ));
                }
                items
            }
            Err(error) => {
                warnings.push(error);
                Vec::new()
            }
        };
    Ok(FeishuWorkspaceStatus {
        missing_required_fields: initialization_missing,
        existing_fields: fields,
        status_is_formula: formula_status,
        has_kanban: views.iter().any(|item| item.kind == "kanban"),
        views,
        dashboards,
        workflows,
        inspection_warnings: warnings,
    })
}

fn inspect_status_workflows<F>(
    status_is_formula: bool,
    read_workflows: F,
) -> Result<Vec<FeishuWorkspaceItem>, String>
where
    F: FnOnce() -> Result<Vec<FeishuWorkspaceItem>, String>,
{
    if status_is_formula {
        // Formula status is computed from progress; automation configuration is unrelated.
        Ok(Vec::new())
    } else {
        read_workflows()
    }
}

#[cfg(test)]
pub fn inspect_workspace(
    config: &FeishuConfig,
    secret: &str,
) -> Result<FeishuWorkspaceStatus, String> {
    let access_token = tenant_access_token(config, secret)?;
    inspect_workspace_with_token(config, &access_token)
}

pub fn inspect_workspace_with_access_token(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<FeishuWorkspaceStatus, String> {
    inspect_workspace_with_token(config, access_token)
}

pub fn initialize_workspace_with_access_token(
    config: &FeishuConfig,
    access_token: &str,
) -> Result<FeishuWorkspaceInitResult, String> {
    let field_result = create_missing_required_fields_with_access_token(config, access_token)?;
    let before = inspect_workspace_with_token(config, &access_token)?;
    let mut created_views = Vec::new();
    if !before.has_kanban
        && !before
            .inspection_warnings
            .iter()
            .any(|warning| warning.contains("读取视图"))
    {
        let response = client()?
            .post(format!(
                "{API_ROOT}/bitable/v1/apps/{}/tables/{}/views",
                config.app_token, config.table_id
            ))
            .bearer_auth(&access_token)
            .json(&json!({ "view_name": "任务看板", "view_type": "kanban" }))
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        parse_api_response::<Value>(response, "创建任务看板")?;
        created_views.push("任务看板".to_string());
    }
    let status = inspect_workspace_with_token(config, &access_token)?;
    Ok(FeishuWorkspaceInitResult {
        created_fields: field_result.created_fields,
        created_views,
        status,
    })
}

/// Copy the canonical Feishu Bitable template using a user-authorized access token.
/// The caller must first verify that every source table is empty. Keeping content in the
/// copy request is necessary to preserve dashboard components in Feishu's current API.
pub fn copy_task_workspace_template(
    template_app_token: &str,
    user_access_token: &str,
    workspace_name: &str,
    folder_token: Option<&str>,
) -> Result<FeishuTemplateCopyResult, FeishuTemplateCopyError> {
    copy_task_workspace_template_at(
        API_ROOT,
        template_app_token,
        user_access_token,
        workspace_name,
        folder_token,
    )
}

fn copy_task_workspace_template_at(
    api_root: &str,
    template_app_token: &str,
    user_access_token: &str,
    workspace_name: &str,
    folder_token: Option<&str>,
) -> Result<FeishuTemplateCopyResult, FeishuTemplateCopyError> {
    let template_app_token = template_app_token.trim();
    let workspace_name = workspace_name.trim();
    if template_app_token.is_empty()
        || !template_app_token
            .chars()
            .all(|character| character.is_ascii_alphanumeric())
    {
        return Err(FeishuTemplateCopyError::Rejected(
            "飞书模板标识格式不正确".to_string(),
        ));
    }
    if workspace_name.is_empty() {
        return Err(FeishuTemplateCopyError::Rejected(
            "请填写任务空间名称".to_string(),
        ));
    }

    let mut body = json!({
        "name": workspace_name,
        "without_content": false,
        "time_zone": "Asia/Shanghai"
    });
    if let Some(folder_token) = folder_token
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        body["folder_token"] = json!(folder_token);
    }

    let response = client()
        .map_err(FeishuTemplateCopyError::Rejected)?
        .post(format!(
            "{api_root}/bitable/v1/apps/{template_app_token}/copy"
        ))
        .bearer_auth(user_access_token)
        .json(&body)
        .send()
        .map_err(|_| {
            FeishuTemplateCopyError::OutcomeUnknown(
                "飞书复制请求结果未知。请先核对任务空间列表，再继续，避免重复创建。".to_string(),
            )
        })?;

    let status = response.status();
    if !status.is_success() {
        let message = format!("飞书复制任务模板失败（HTTP {}）", status.as_u16());
        return if status.is_server_error() {
            Err(FeishuTemplateCopyError::OutcomeUnknown(format!(
                "{message}；服务端可能已处理请求，请先核对任务空间列表，再继续。"
            )))
        } else {
            Err(FeishuTemplateCopyError::Rejected(message))
        };
    }

    let payload = response.json::<ApiResponse<CopiedAppData>>().map_err(|_| {
        FeishuTemplateCopyError::OutcomeUnknown(
            "飞书已响应复制请求，但返回内容无法确认。请先核对任务空间列表，再继续。".to_string(),
        )
    })?;
    if payload.code != 0 {
        return Err(FeishuTemplateCopyError::Rejected(format_api_error(
            "复制任务模板",
            &payload,
        )));
    }
    let app = payload.data.and_then(|data| data.app).ok_or_else(|| {
        FeishuTemplateCopyError::OutcomeUnknown(
            "飞书已响应复制请求，但没有返回副本信息。请先核对任务空间列表，再继续。".to_string(),
        )
    })?;
    if app.app_token.trim().is_empty() {
        return Err(FeishuTemplateCopyError::OutcomeUnknown(
            "飞书已响应复制请求，但没有返回副本标识。请先核对任务空间列表，再继续。".to_string(),
        ));
    }

    Ok(FeishuTemplateCopyResult {
        app_token: app.app_token,
        name: app.name,
        url: app.url,
    })
}

/// Discover the task table by the versioned field contract instead of relying on table IDs.
pub fn discover_task_table(
    app_token: &str,
    user_access_token: &str,
) -> Result<FeishuTaskTable, String> {
    discover_task_table_at(API_ROOT, app_token, user_access_token)
}

fn discover_task_table_at(
    api_root: &str,
    app_token: &str,
    user_access_token: &str,
) -> Result<FeishuTaskTable, String> {
    let mut tables = Vec::new();
    let mut page_token = None::<String>;
    loop {
        let mut url = Url::parse(&format!("{api_root}/bitable/v1/apps/{app_token}/tables"))
            .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "100");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(user_access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: TableData = parse_api_response(response, "读取数据表列表")?;
        tables.extend(data.items);
        if !data.has_more {
            break;
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书数据表分页缺少 page_token".to_string());
        }
    }

    let mut matches = Vec::new();
    let mut inaccessible_tables = Vec::new();
    for table in tables {
        match list_field_items_at(api_root, app_token, &table.table_id, user_access_token) {
            Ok(fields) => {
                let field_names = fields
                    .iter()
                    .map(|field| field.field_name.clone())
                    .collect::<Vec<_>>();
                if missing_required_fields(&field_names).is_empty() {
                    matches.push(FeishuTaskTable {
                        table_id: table.table_id,
                        name: table.name,
                        fields: field_names,
                    });
                }
            }
            Err(error) => inaccessible_tables.push(format!("{}：{error}", table.name)),
        }
    }

    if matches.len() == 1 {
        return Ok(matches.remove(0));
    }
    if matches.len() > 1 {
        let named_matches = matches
            .iter()
            .filter(|table| table.name == "任务清单")
            .count();
        if named_matches == 1 {
            return Ok(matches
                .into_iter()
                .find(|table| table.name == "任务清单")
                .expect("one exact task table"));
        }
        return Err("发现多个符合任务字段规范的数据表，无法安全自动选择".to_string());
    }
    if inaccessible_tables.is_empty() {
        Err("复制后的多维表格中没有找到符合任务字段规范的数据表".to_string())
    } else {
        Err(format!(
            "未能完整核验复制后的数据表；未读取的数据表：{}",
            inaccessible_tables.join("；")
        ))
    }
}

pub fn inspect_workspace_as_user(
    app_token: &str,
    table_id: &str,
    user_access_token: &str,
) -> Result<FeishuWorkspaceStatus, String> {
    let config = FeishuConfig {
        app_token: app_token.to_string(),
        table_id: table_id.to_string(),
        ..FeishuConfig::default()
    };
    inspect_workspace_with_token(&config, user_access_token)
}

fn field_named<'a>(fields: &'a [FieldItem], names: &[&str]) -> Option<&'a FieldItem> {
    names
        .iter()
        .find_map(|name| fields.iter().find(|field| field.field_name == *name))
}

fn field_formatter(field: &FieldItem) -> String {
    field
        .property
        .get("formatter")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn progress_write_value(field: &FieldItem, value: u8) -> Value {
    let percentage_storage = field.ui_type.to_ascii_lowercase().contains("progress")
        || field_formatter(field).contains('%');
    if percentage_storage {
        json!(f64::from(value) / 100.0)
    } else {
        json!(value)
    }
}

fn quadrant_code_from_label(value: &str) -> &'static str {
    let normalized = value.replace([' ', '🔥', '👇', '📌', '🚀', '⭐', '🎯'], "");
    if normalized.contains("不重要不紧急") {
        "nn"
    } else if normalized.contains("紧急不重要") {
        "un"
    } else if normalized.contains("重要不紧急") {
        "in"
    } else if normalized.contains("重要且紧急") || normalized.contains("重要紧急") {
        "iu"
    } else {
        ""
    }
}

fn canonical_quadrant_label(quadrant: &str) -> &'static str {
    match quadrant {
        "iu" => "重要且紧急",
        "un" => "紧急不重要",
        "in" => "重要不紧急",
        _ => "不重要不紧急",
    }
}

fn quadrant_write_label(field: &FieldItem, quadrant: &str) -> String {
    field
        .property
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|option| option.get("name").and_then(Value::as_str))
        .find(|name| quadrant_code_from_label(name) == quadrant)
        .unwrap_or_else(|| canonical_quadrant_label(quadrant))
        .to_string()
}

fn iso_date_to_unix_ms(value: &str) -> Result<i64, String> {
    let mut parts = value.split('-');
    let year = parts
        .next()
        .and_then(|part| part.parse::<i64>().ok())
        .ok_or_else(|| "飞书计划完成日期格式无效".to_string())?;
    let month = parts
        .next()
        .and_then(|part| part.parse::<i64>().ok())
        .ok_or_else(|| "飞书计划完成日期格式无效".to_string())?;
    let day = parts
        .next()
        .and_then(|part| part.parse::<i64>().ok())
        .ok_or_else(|| "飞书计划完成日期格式无效".to_string())?;
    if parts.next().is_some() || year < 2000 || !(1..=12).contains(&month) {
        return Err("飞书计划完成日期格式无效".to_string());
    }
    let leap_year = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let max_day = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=max_day).contains(&day) {
        return Err("飞书计划完成日期格式无效".to_string());
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year
    } else {
        adjusted_year - 399
    }
    .div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Ok(days * 86_400_000 - 8 * 3_600_000)
}

fn write_fields(
    fields: &[FieldItem],
    request: &FeishuWriteRequest,
) -> Result<serde_json::Map<String, Value>, String> {
    let progress_field =
        field_named(fields, &["进度"]).ok_or_else(|| "飞书数据表缺少“进度”字段".to_string())?;
    let mut values = serde_json::Map::new();
    match request {
        FeishuWriteRequest::Update {
            progress,
            status,
            planned_date,
            ..
        } => {
            values.insert(
                progress_field.field_name.clone(),
                progress_write_value(progress_field, *progress),
            );
            if let Some(status) = status {
                let field = field_named(fields, &["状态"])
                    .ok_or_else(|| "飞书数据表缺少“状态”字段，无法同步任务状态".to_string())?;
                if !is_formula_field(field) {
                    values.insert(field.field_name.clone(), json!(status));
                }
            }
            if let Some(planned_date) = planned_date {
                let field = field_named(
                    fields,
                    &[
                        "任务计划完成日期",
                        "任务计划完成时间",
                        "计划完成日期",
                        "截止日期",
                    ],
                )
                .ok_or_else(|| "飞书数据表缺少计划完成日期字段，无法执行延期".to_string())?;
                let value = planned_date
                    .as_deref()
                    .map(iso_date_to_unix_ms)
                    .transpose()?
                    .map(Value::from)
                    .unwrap_or(Value::Null);
                values.insert(field.field_name.clone(), value);
            }
        }
        FeishuWriteRequest::CreateTemporary {
            title,
            description,
            progress,
            quadrant,
            today,
            planned_date,
        } => {
            let title_field = field_named(fields, &["任务名称"])
                .ok_or_else(|| "飞书数据表缺少“任务名称”字段".to_string())?;
            let status_field = field_named(fields, &["状态"])
                .ok_or_else(|| "飞书数据表缺少“状态”字段".to_string())?;
            values.insert(title_field.field_name.clone(), json!(title));
            if !is_formula_field(status_field) {
                values.insert(
                    status_field.field_name.clone(),
                    json!(match progress {
                        0 => "未开始",
                        100 => "已完成",
                        _ => "进行中",
                    }),
                );
            }
            values.insert(
                progress_field.field_name.clone(),
                progress_write_value(progress_field, *progress),
            );
            let quadrant_field = field_named(fields, &["任务类型", "任务标签", "任务类别"])
                .ok_or_else(|| "飞书数据表缺少任务类型字段".to_string())?;
            values.insert(
                quadrant_field.field_name.clone(),
                json!(quadrant_write_label(quadrant_field, quadrant)),
            );
            if field_named(fields, &["任务详情"]).is_some() {
                values.insert("任务详情".to_string(), json!(description));
            }
            if field_named(fields, &["今日必完成"]).is_some() {
                values.insert("今日必完成".to_string(), json!(today));
            }
            if let Some(planned_date) = planned_date {
                let field = field_named(
                    fields,
                    &[
                        "任务计划完成日期",
                        "任务计划完成时间",
                        "计划完成日期",
                        "截止日期",
                    ],
                )
                .ok_or_else(|| "飞书数据表缺少计划完成日期字段，不能静默丢弃日期".to_string())?;
                values.insert(
                    field.field_name.clone(),
                    json!(iso_date_to_unix_ms(planned_date)?),
                );
            }
        }
        FeishuWriteRequest::Delete { .. } => {}
    }
    Ok(values)
}

#[cfg(test)]
pub fn write_task(
    config: &FeishuConfig,
    secret: &str,
    request: &FeishuWriteRequest,
) -> Result<Option<String>, String> {
    let access_token = tenant_access_token(config, secret)?;
    write_task_with_access_token(config, &access_token, request)
}

pub fn write_task_with_access_token(
    config: &FeishuConfig,
    access_token: &str,
    request: &FeishuWriteRequest,
) -> Result<Option<String>, String> {
    crate::write_acceptance::ensure_task_write(config)?;
    let url = match request {
        FeishuWriteRequest::CreateTemporary { .. } => format!(
            "{API_ROOT}/bitable/v1/apps/{}/tables/{}/records",
            config.app_token, config.table_id
        ),
        FeishuWriteRequest::Update { record_id, .. } | FeishuWriteRequest::Delete { record_id } => {
            format!(
                "{API_ROOT}/bitable/v1/apps/{}/tables/{}/records/{record_id}",
                config.app_token, config.table_id
            )
        }
    };
    if matches!(request, FeishuWriteRequest::Delete { .. }) {
        let response = client()?
            .delete(url)
            .bearer_auth(access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        parse_api_response::<Value>(response, "删除记录")?;
        return Ok(None);
    }
    let fields = list_field_items(config, access_token)?;
    let payload = json!({ "fields": write_fields(&fields, request)? });
    let response = match request {
        FeishuWriteRequest::CreateTemporary { .. } => client()?
            .post(url)
            .bearer_auth(access_token)
            .json(&payload)
            .send(),
        FeishuWriteRequest::Update { .. } => client()?
            .put(url)
            .bearer_auth(access_token)
            .json(&payload)
            .send(),
        FeishuWriteRequest::Delete { .. } => unreachable!(),
    }
    .map_err(|error| format!("无法连接飞书：{error}"))?;
    let data: RecordMutationData = parse_api_response(response, "写入记录")?;
    Ok(data.record.map(|record| record.record_id))
}

// Separate formal floating path. The legacy acceptance writer above stays closed.
pub(crate) struct PreparedFloatingMutation { config:FeishuConfig, token:String, request:FeishuWriteRequest, payload:Value }
pub(crate) fn prepare_floating_mutation(config:&FeishuConfig,token:&str,read:&SyncInspection,request:FeishuWriteRequest)->Result<PreparedFloatingMutation,String>{
    sync_inspect::validate_target(config)?;
    if token.is_empty()||!read.complete||!read.read_only||read.app_id!=config.app_id||read.app_token!=config.app_token||read.table_id!=config.table_id
        || !read.subject_id.strip_prefix(&format!("{}:",config.app_id)).is_some_and(|s|!s.trim().is_empty()) {return Err("飞书账号或目标未核实，未准备写入".into());}
    if let FeishuWriteRequest::Update{record_id,..}|FeishuWriteRequest::Delete{record_id}=&request {
        if record_id.is_empty()||!record_id.chars().all(|c|c.is_ascii_alphanumeric())||!read.records.iter().any(|r|r.record_id==*record_id){return Err("目标记录不在本次已核实的飞书表中".into());}
    }
    let payload=if matches!(request,FeishuWriteRequest::Delete{..}){Value::Null}else{
        let fields=list_field_items(config,token)?;
        if let FeishuWriteRequest::CreateTemporary{description,today,..}=&request{
            if (!description.is_empty()&&field_named(&fields,&["任务详情"]).is_none())||(*today&&field_named(&fields,&["今日必完成"]).is_none()){return Err("飞书表缺少任务详情或今日必完成字段，未发送新增请求".into());}
        }
        json!({"fields":write_fields(&fields,&request)?})
    };
    Ok(PreparedFloatingMutation{config:config.clone(),token:token.into(),request,payload})
}
pub(crate) struct FloatingSendError {pub definite:bool,pub message:String}
pub(crate) fn send_floating_mutation(p:PreparedFloatingMutation)->Result<Option<String>,FloatingSendError>{
    send_floating_mutation_at(API_ROOT,p)
}
fn send_floating_mutation_at(root:&str,p:PreparedFloatingMutation)->Result<Option<String>,FloatingSendError>{
    let base=format!("{root}/bitable/v1/apps/{}/tables/{}/records",p.config.app_token,p.config.table_id);
    let client=client().map_err(|e|FloatingSendError{definite:true,message:e})?;
    let response=match &p.request {
        FeishuWriteRequest::CreateTemporary{..}=>client.post(base).bearer_auth(&p.token).json(&p.payload).send(),
        FeishuWriteRequest::Update{record_id,..}=>client.put(format!("{base}/{record_id}")).bearer_auth(&p.token).json(&p.payload).send(),
        FeishuWriteRequest::Delete{record_id}=>client.delete(format!("{base}/{record_id}")).bearer_auth(&p.token).send(),
    }.map_err(|e|FloatingSendError{definite:e.is_connect(),message:format!("飞书请求结果未确认：{e}")})?;
    let status=response.status();let payload:ApiResponse<Value>=response.json().map_err(|_|FloatingSendError{definite:false,message:"飞书响应无法解析".into()})?;
    if !status.is_success()||payload.code!=0{return Err(FloatingSendError{definite:status.as_u16()==401||status.as_u16()==403||payload.code==91403,message:format_api_error("写入记录",&payload)})}
    if matches!(p.request,FeishuWriteRequest::Delete{..}){return Ok(None)}
    Ok(payload.data.and_then(|v|v.get("record").and_then(|r|r.get("record_id")).and_then(Value::as_str).map(str::to_owned)))
}

#[cfg(test)]
fn list_records(config: &FeishuConfig, access_token: &str) -> Result<Vec<RecordItem>, String> {
    list_records_at(API_ROOT, config, access_token)
}

fn list_records_at(
    api_root: &str,
    config: &FeishuConfig,
    access_token: &str,
) -> Result<Vec<RecordItem>, String> {
    let mut records = Vec::new();
    let mut page_token = None::<String>;
    loop {
        let mut url = Url::parse(&format!(
            "{api_root}/bitable/v1/apps/{}/tables/{}/records",
            config.app_token, config.table_id
        ))
        .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "500");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: RecordData = parse_api_response(response, "读取记录")?;
        records.extend(data.items);
        if !data.has_more {
            break;
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书记录分页缺少 page_token".to_string());
        }
    }
    Ok(records)
}

/// Confirm that the copied task table has no user-visible records before saving it as the
/// active workspace. Request only one row per page and stop immediately if any record exists.
pub fn ensure_task_table_empty(
    app_token: &str,
    table_id: &str,
    user_access_token: &str,
) -> Result<(), String> {
    ensure_empty_task_table_at(API_ROOT, app_token, table_id, user_access_token)
}

/// Fail closed before a full-structure copy if any source table has records, or if
/// Feishu cannot prove that the source is empty with the current authorization.
pub fn ensure_template_tables_empty(
    app_token: &str,
    user_access_token: &str,
) -> Result<(), String> {
    ensure_template_tables_empty_at(API_ROOT, app_token, user_access_token)
}

fn ensure_template_tables_empty_at(
    api_root: &str,
    app_token: &str,
    user_access_token: &str,
) -> Result<(), String> {
    let mut page_token = None::<String>;
    let mut table_count = 0;
    loop {
        let mut url = Url::parse(&format!("{api_root}/bitable/v1/apps/{app_token}/tables"))
            .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "100");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(user_access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: TableData = parse_api_response(response, "读取模板数据表列表")?;
        table_count += data.items.len();
        for table in data.items {
            ensure_empty_task_table_at(api_root, app_token, &table.table_id, user_access_token)
                .map_err(|error| {
                    format!("模板源数据表「{}」未通过空记录检查：{error}", table.name)
                })?;
        }
        if !data.has_more {
            break;
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书模板数据表分页缺少 page_token".to_string());
        }
    }
    if table_count == 0 {
        return Err("飞书模板源没有数据表，无法创建任务空间".to_string());
    }
    Ok(())
}

fn ensure_empty_task_table_at(
    api_root: &str,
    app_token: &str,
    table_id: &str,
    user_access_token: &str,
) -> Result<(), String> {
    let mut page_token = None::<String>;
    loop {
        let mut url = Url::parse(&format!(
            "{api_root}/bitable/v1/apps/{app_token}/tables/{table_id}/records"
        ))
        .map_err(|error| error.to_string())?;
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("page_size", "1");
            if let Some(token) = page_token.as_deref() {
                query.append_pair("page_token", token);
            }
        }
        let response = client()?
            .get(url)
            .bearer_auth(user_access_token)
            .send()
            .map_err(|error| format!("无法连接飞书：{error}"))?;
        let data: RecordData = parse_api_response(response, "核验复制后的任务记录")?;
        if !data.items.is_empty() {
            return Err(
                "复制后的任务清单至少包含 1 条任务记录；为保护数据，尚未连接该空间。请先清空副本中的任务记录，再继续核对。"
                    .to_string(),
            );
        }
        if !data.has_more {
            return Ok(());
        }
        page_token = data.page_token;
        if page_token.is_none() {
            return Err("飞书记录分页缺少 page_token，无法确认任务清单为空".to_string());
        }
    }
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| text(Some(value)))
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("、"),
        Some(Value::Object(value)) => ["text", "value", "name", "label"]
            .iter()
            .filter_map(|key| value.get(*key))
            .map(|value| text(Some(value)))
            .find(|value| !value.is_empty())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn progress(value: Option<&Value>) -> u8 {
    let number = match value {
        Some(Value::Number(value)) => value.as_f64().unwrap_or_default(),
        Some(Value::String(value)) => value.trim_end_matches('%').parse().unwrap_or_default(),
        _ => 0.0,
    };
    let normalized = if number > 0.0 && number <= 1.0 {
        number * 100.0
    } else {
        number
    };
    normalized.round().clamp(0.0, 100.0) as u8
}

fn quadrant(value: Option<&Value>) -> String {
    let normalized = text(value).replace([' ', '🔥', '👇', '📌', '🚀', '⭐', '🎯'], "");
    if normalized.contains("不重要不紧急") {
        "nn"
    } else if normalized.contains("紧急不重要") {
        "un"
    } else if normalized.contains("重要不紧急") {
        "in"
    } else if normalized.contains("重要且紧急") || normalized.contains("重要紧急") {
        "iu"
    } else {
        "nn"
    }
    .to_string()
}

fn first_field<'a>(
    fields: &'a serde_json::Map<String, Value>,
    names: &[&str],
) -> Option<&'a Value> {
    names.iter().find_map(|name| fields.get(*name))
}

fn checkbox(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or_default() != 0,
        Some(Value::String(value)) => matches!(value.as_str(), "1" | "true" | "是" | "已勾选"),
        _ => false,
    }
}

fn civil_date_from_unix_ms(milliseconds: i64) -> String {
    // 飞书日期字段返回毫秒时间戳；本应用面向中国时区，按 UTC+8 还原日期。
    let days = (milliseconds + 8 * 3_600_000).div_euclid(86_400_000);
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02}")
}

fn planned_date(fields: &serde_json::Map<String, Value>) -> Option<String> {
    [
        "任务计划完成日期",
        "任务计划完成时间",
        "计划完成日期",
        "截止日期",
    ]
    .into_iter()
    .filter_map(|name| fields.get(name))
    .find_map(|value| match value {
        Value::Number(number) => number.as_i64().map(civil_date_from_unix_ms),
        Value::String(value) => value
            .parse::<i64>()
            .ok()
            .map(civil_date_from_unix_ms)
            .or_else(|| (!value.trim().is_empty()).then(|| value.trim().replace('/', "-"))),
        _ => {
            let value = text(Some(value));
            (!value.trim().is_empty()).then(|| value.trim().replace('/', "-"))
        }
    })
}

fn parse_tasks(records: &[RecordItem], now: i64) -> Vec<Task> {
    records
        .iter()
        .filter_map(|record| {
            let title = text(record.fields.get("任务名称"));
            if title.trim().is_empty() {
                return None;
            }
            Some(Task {
                id: record.record_id.clone(),
                title,
                detail: text(record.fields.get("任务详情")),
                progress: progress(record.fields.get("进度")),
                quadrant: quadrant(first_field(
                    &record.fields,
                    &["任务类型", "任务标签", "任务类别"],
                )),
                status: text(record.fields.get("状态")),
                today: checkbox(record.fields.get("今日必完成")),
                planned_date: planned_date(&record.fields),
                updated_at: now,
            })
        })
        .collect()
}

#[cfg(test)]
pub fn fetch_tasks(
    config: &FeishuConfig,
    secret: &str,
    now: i64,
) -> Result<(Vec<Task>, FeishuSyncResult), String> {
    let access_token = tenant_access_token(config, secret)?;
    fetch_tasks_with_access_token(config, &access_token, now)
}

pub fn fetch_tasks_with_access_token(
    config: &FeishuConfig,
    access_token: &str,
    now: i64,
) -> Result<(Vec<Task>, FeishuSyncResult), String> {
    fetch_tasks_with_access_token_at(API_ROOT, config, access_token, now)
}

fn fetch_tasks_with_access_token_at(
    api_root: &str,
    config: &FeishuConfig,
    access_token: &str,
    now: i64,
) -> Result<(Vec<Task>, FeishuSyncResult), String> {
    let field_items =
        list_field_items_at(api_root, &config.app_token, &config.table_id, access_token)?;
    let formula_status = has_formula_status_field(&field_items);
    let fields = field_items
        .into_iter()
        .map(|field| field.field_name)
        .collect::<Vec<_>>();
    let missing = missing_required_fields(&fields);
    if !missing.is_empty() {
        return Err(format!(
            "飞书数据表缺少必需字段：{}。未替换本地缓存",
            missing.join("、")
        ));
    }
    let read = || {
        list_records_at(api_root, config, access_token).map(|records| parse_tasks(&records, now))
    };
    let mut tasks = read()?;
    // Feishu can omit a computed field intermittently while returning the other fields.
    // Retry only GETs; never cache an incomplete formula snapshot or resend a mutation.
    if formula_status {
        for delay_ms in [150, 300, 600, 1_000] {
            if tasks.iter().all(|task| !task.status.trim().is_empty()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
            tasks = read()?;
        }
        if tasks.iter().any(|task| task.status.trim().is_empty()) {
            return Err(
                "飞书暂未返回完整的公式状态，请稍后重新同步；未使用空状态替换本地缓存".to_string(),
            );
        }
    }
    let result = FeishuSyncResult {
        task_count: tasks.len(),
        completed_count: tasks.iter().filter(|task| task.status == "已完成").count(),
        table_id: config.table_id.clone(),
        fields,
        write_ready: crate::write_acceptance::allows(config),
        undo_ready: crate::write_acceptance::allows(config),
    };
    Ok((tasks, result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialization_date_field_spec_disables_automatic_values() {
        let date = required_field_spec("计划完成日期").expect("初始化应支持创建计划完成日期列");
        assert_eq!(date["type"], 5);
        assert_eq!(date["property"]["auto_fill"], false);
        let start = required_field_spec("开始日期").expect("同步需要开始日期列");
        assert_eq!(start["type"], 5);
        assert_eq!(start["property"]["auto_fill"], false);
        assert_eq!(required_field_spec("今日必完成").unwrap()["type"], 7);
    }
    #[test]
    fn floating_delete_transport_sends_only_one_exact_delete() {
        let (root,server)=mock_server(vec![mock_json(r#"{"code":0,"msg":"success","data":{"deleted":true}}"#)]);
        let p=PreparedFloatingMutation{config:initialization_config(),token:"synthetic".into(),request:FeishuWriteRequest::Delete{record_id:"recTarget".into()},payload:Value::Null};
        assert!(matches!(send_floating_mutation_at(&root,p),Ok(None)));
        let requests=server.join().unwrap();assert_eq!(requests.len(),1);
        assert_eq!(requests[0].0,"DELETE /open-apis/bitable/v1/apps/base_test/tables/tbl_test/records/recTarget HTTP/1.1");
        assert!(requests[0].1.is_empty());
    }

    fn initialization_test_fields(date_name: Option<&str>, date_type: i64) -> Value {
        let mut fields = vec![json!({"field_name":"任务名称","type":1}),json!({"field_name":"任务标签","type":3}),json!({"field_name":"状态","type":3}),json!({"field_name":"进度","type":2}),
            json!({"field_name":"今日必完成","type":7}),json!({"field_name":"开始日期","type":5,"property":{"auto_fill":false}})];
        if let Some(name) = date_name {fields.push(json!({"field_name":name,"type":date_type,"property":{"auto_fill":false}}));}
        json!(fields)
    }
    fn initialization_fields_response(fields: Value) -> MockResponse {
        mock_json(&json!({"code":0,"msg":"success","data":{"items":fields,"has_more":false}}).to_string())
    }
    fn initialization_config() -> FeishuConfig {
        FeishuConfig {app_id:"cli_test".into(),app_token:"base_test".into(),table_id:"tbl_test".into(),secret_configured:false}
    }
    #[test]
    fn initialization_missing_date_creates_one_empty_date_column_and_reads_back() {
        let before=initialization_test_fields(None,5);
        let names=serde_json::from_value::<Vec<FieldItem>>(before.clone()).unwrap().into_iter().map(|f|f.field_name).collect::<Vec<_>>();
        assert!(missing_required_fields(&names).is_empty(), "旧表读取条件不应改变");
        let (root,server)=mock_server(vec![initialization_fields_response(before),mock_json(r#"{"code":0,"msg":"success","data":{}}"#),initialization_fields_response(initialization_test_fields(Some("计划完成日期"),5))]);
        let result=create_initialization_fields_at(&root,&initialization_config(),"synthetic").unwrap();
        assert_eq!(result.created_fields,vec!["计划完成日期"]);
        assert!(result.missing_required_fields.is_empty());
        let requests=server.join().unwrap();assert_eq!(requests.len(),3);
        assert!(requests[1].0.starts_with("POST /open-apis/bitable/v1/apps/base_test/tables/tbl_test/fields "));
        let body:Value=serde_json::from_str(&requests[1].1).unwrap();
        assert_eq!(body,json!({"field_name":"计划完成日期","type":5,"property":{"date_formatter":"yyyy/MM/dd","auto_fill":false}}));
        assert!(requests.iter().all(|(path,_)|!path.contains("/records")));
    }
    #[test]
    fn initialization_repairs_missing_import_columns_without_touching_records() {
        let before=json!([
            {"field_name":"任务名称","type":1},{"field_name":"任务标签","type":3},
            {"field_name":"状态","type":3},{"field_name":"进度","type":2},
            {"field_name":"任务详情","type":1},{"field_name":"计划完成日期","type":5,"property":{"auto_fill":false}}
        ]);
        let mut after=before.as_array().unwrap().clone();
        after.push(json!({"field_name":"今日必完成","type":7}));
        after.push(json!({"field_name":"开始日期","type":5,"property":{"auto_fill":false}}));
        let (root,server)=mock_server(vec![initialization_fields_response(before),
            mock_json(r#"{"code":0,"msg":"success","data":{}}"#),
            mock_json(r#"{"code":0,"msg":"success","data":{}}"#),
            initialization_fields_response(json!(after))]);
        let result=create_initialization_fields_at(&root,&initialization_config(),"synthetic").unwrap();
        assert_eq!(result.created_fields,vec!["今日必完成","开始日期"]);
        let requests=server.join().unwrap();
        assert_eq!(requests.len(),4);
        assert_eq!(serde_json::from_str::<Value>(&requests[1].1).unwrap(),required_field_spec("今日必完成").unwrap());
        assert_eq!(serde_json::from_str::<Value>(&requests[2].1).unwrap(),required_field_spec("开始日期").unwrap());
        assert!(requests.iter().all(|(path,_)|!path.contains("/records")));
    }
    #[test]
    fn initialization_existing_date_aliases_never_create_duplicates() {
        for alias in ["计划完成日期","任务计划完成日期","任务计划完成时间","截止日期"] {
            let fields=initialization_test_fields(Some(alias),5);
            let (root,server)=mock_server(vec![initialization_fields_response(fields.clone()),initialization_fields_response(fields)]);
            assert!(create_initialization_fields_at(&root,&initialization_config(),"synthetic").unwrap().created_fields.is_empty());
            assert!(server.join().unwrap().iter().all(|(path,_)|path.starts_with("GET ")));
        }
    }
    #[test]
    fn initialization_wrong_date_type_does_not_mutate_or_shadow_the_column() {
        let (root,server)=mock_server(vec![initialization_fields_response(initialization_test_fields(Some("截止日期"),1))]);
        assert!(create_initialization_fields_at(&root,&initialization_config(),"synthetic").unwrap_err().contains("不是日期类型"));
        assert_eq!(server.join().unwrap().len(),1);
    }
    #[test]
    fn initialization_missing_readback_is_not_reported_complete() {
        let fields=initialization_test_fields(None,5);
        let (root,server)=mock_server(vec![initialization_fields_response(fields.clone()),mock_json(r#"{"code":0,"msg":"success","data":{}}"#),initialization_fields_response(fields)]);
        assert!(create_initialization_fields_at(&root,&initialization_config(),"synthetic").unwrap_err().contains("回读核实"));
        assert_eq!(server.join().unwrap().iter().filter(|(p,_)|p.starts_with("POST ")).count(),1);
    }
    #[test]
    fn initialization_ambiguous_post_is_not_retried_and_resume_rechecks_fields() {
        let (root,server)=mock_server(vec![initialization_fields_response(initialization_test_fields(None,5)),MockResponse{status:200,body:String::new(),close_without_response:true}]);
        assert!(create_initialization_fields_at(&root,&initialization_config(),"synthetic").is_err());
        assert_eq!(server.join().unwrap().iter().filter(|(p,_)|p.starts_with("POST ")).count(),1);
        let fields=initialization_test_fields(Some("计划完成日期"),5);
        let (root,server)=mock_server(vec![initialization_fields_response(fields.clone()),initialization_fields_response(fields)]);
        assert!(create_initialization_fields_at(&root,&initialization_config(),"synthetic").unwrap().created_fields.is_empty());
        assert!(server.join().unwrap().iter().all(|(p,_)|p.starts_with("GET ")));
    }

    /// Explicit diagnostic only: fresh saved user token, GET field metadata, no refresh or mutations.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn diagnose_current_floating_date_schema_readonly() {
        assert_eq!(std::env::var("PROJECT_TRACK_READONLY_DATE_SCHEMA").as_deref(), Ok("1"));
        assert_eq!(CREDENTIAL_SERVICE, "app.snaptask.desktop");
        let path = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap())
            .join("app.snaptask.desktop/project-track.sqlite3");
        let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let setting = |key: &str| db.query_row("SELECT value FROM app_settings WHERE key=?1", [key], |r| r.get::<_, String>(0)).unwrap();
        assert_eq!(setting("task_source.active"), "feishu");
        let config = FeishuConfig { app_id: setting("feishu.app_id"), app_token: setting("feishu.app_token"), table_id: setting("feishu.table_id"), secret_configured: false };
        sync_inspect::validate_target(&config).unwrap();
        let session = read_user_session().expect("无法读取当前用户授权；未尝试其他凭据");
        validate_session_app_id(&session, &config.app_id).unwrap();
        assert!(session.access_expires_at > crate::storage::unix_now() + 30, "授权需刷新；此只读诊断不刷新授权");
        let fields = list_field_items(&config, &session.access_token).unwrap();
        println!("READONLY_FIELD_SCHEMA {}", serde_json::to_string(&fields.iter().map(|f| serde_json::json!({"name": f.field_name, "uiType": f.ui_type})).collect::<Vec<_>>()).unwrap());
        let request = FeishuWriteRequest::CreateTemporary { title: "readonly diagnostic - never sent".into(), description: String::new(), progress: 100, quadrant: "iu".into(), today: false, planned_date: Some("2026-10-26".into()) };
        let prepared = write_fields(&fields, &request);
        println!("READONLY_DATE_PAYLOAD {}", prepared.as_ref().map(|_| "日期可写入现有字段".to_string()).unwrap_or_else(|e| e.clone()));
        assert!(prepared.is_ok(), "日期字段诊断未通过；没有发送任务或字段写入请求");
    }

    #[test]
    fn floating_create_date_requires_date_column_but_dateless_create_does_not() {
        let mut fields: Vec<FieldItem> = serde_json::from_value(json!([
            {"field_name":"任务名称","ui_type":"Text"},
            {"field_name":"任务标签","ui_type":"SingleSelect"},
            {"field_name":"状态","ui_type":"SingleSelect"},
            {"field_name":"进度","ui_type":"Progress"}
        ])).unwrap();
        let request = |planned_date| FeishuWriteRequest::CreateTemporary {
            title: "【示例】A技术资料整理完成并提交".into(), description: String::new(),
            progress: 100, quadrant: "iu".into(), today: false, planned_date,
        };
        let missing = write_fields(&fields, &request(Some("2026-10-26".into()))).unwrap_err();
        assert_eq!(missing, "飞书数据表缺少计划完成日期字段，不能静默丢弃日期");
        assert!(write_fields(&fields, &request(None)).is_ok());
        fields.push(serde_json::from_value(json!({"field_name":"计划完成日期","ui_type":"Date"})).unwrap());
        let values = write_fields(&fields, &request(Some("2026-10-26".into()))).unwrap();
        assert_eq!(values.get("计划完成日期"), Some(&json!(iso_date_to_unix_ms("2026-10-26").unwrap())));
        assert_eq!(values.get("状态"), Some(&json!("已完成")));
    }

    /// One-shot, explicitly approved schema repair. Never included in the application.
    #[test]
    #[ignore]
    #[cfg(windows)]
    fn approved_add_only_planned_date_field() {
        assert_eq!(std::env::var("PROJECT_TRACK_APPROVED_DATE_TABLE").as_deref(), Ok("tbl0000000000006"));
        assert_eq!(CREDENTIAL_SERVICE, "app.snaptask.desktop");
        let path = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("app.snaptask.desktop/project-track.sqlite3");
        let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let setting = |key: &str| db.query_row("SELECT value FROM app_settings WHERE key=?1", [key], |r| r.get::<_, String>(0)).unwrap();
        let config = FeishuConfig { app_id: setting("feishu.app_id"), app_token: setting("feishu.app_token"), table_id: setting("feishu.table_id"), secret_configured: false };
        assert_eq!(config.app_id, "cli_0000000000000001");
        assert_eq!(config.table_id, "tbl0000000000006");
        assert_eq!(config.app_token, std::env::var("PROJECT_TRACK_APPROVED_DATE_BASE").unwrap());
        let session = read_user_session().unwrap();
        validate_session_app_id(&session, &config.app_id).unwrap();
        assert!(session.access_expires_at > crate::storage::unix_now() + 30, "授权需刷新；此操作不自动刷新或改用应用身份");
        let fields = list_field_items(&config, &session.access_token).unwrap();
        if let Some(f) = field_named(&fields, &["计划完成日期"]) {
            assert!(f.field_type == 5 || f.ui_type == "Date", "已有同名列但类型不同，未修改");
            println!("APPROVED_DATE_FIELD already_exists; mutation_count=0");
            return;
        }
        assert!(field_named(&fields, &["任务计划完成日期", "任务计划完成时间", "截止日期"]).is_none(), "已存在对应列，未新增重复列");
        let records_before = list_records(&config, &session.access_token).unwrap();
        assert_eq!(setting("feishu.app_id"), config.app_id);
        assert_eq!(setting("feishu.app_token"), config.app_token);
        assert_eq!(setting("feishu.table_id"), config.table_id);
        assert_eq!(setting("task_source.active"), "feishu");
        let response = Client::builder().timeout(Duration::from_secs(25)).redirect(reqwest::redirect::Policy::none()).build().unwrap()
            .post(format!("{API_ROOT}/bitable/v1/apps/{}/tables/{}/fields", config.app_token, config.table_id))
            .bearer_auth(&session.access_token)
            .json(&json!({"field_name":"计划完成日期","type":5,"property":{"date_formatter":"yyyy/MM/dd","auto_fill":false}}))
            .send();
        // Exactly one POST. If it fails or is ambiguous, read metadata; never retry the POST.
        let acknowledged = response.map_err(|_| "字段请求结果不明；禁止重发".to_string()).and_then(|r| parse_api_response::<Value>(r, "创建已确认日期字段"));
        let after = list_field_items(&config, &session.access_token).unwrap();
        let date = field_named(&after, &["计划完成日期"]).unwrap_or_else(|| panic!("日期列未建立，未重发：{}", acknowledged.err().unwrap_or_else(|| "请只读核查".into())));
        assert!(date.field_type == 5 || date.ui_type == "Date");
        assert_eq!(date.property.get("auto_fill").and_then(Value::as_bool), Some(false));
        assert_eq!(after.len(), fields.len()+1);
        for old in &fields {
            let new = field_named(&after, &[&old.field_name]).expect("原字段仍应存在");
            assert!(old.field_type==new.field_type && old.ui_type==new.ui_type && old.property==new.property, "原字段定义有变化；未继续任何写入");
        }
        let records_after = list_records(&config, &session.access_token).unwrap();
        assert_eq!(records_before.len(), records_after.len(), "记录数量发生变化；本操作没有发送记录写入");
        for old in &records_before {
            let new = records_after.iter().find(|r| r.record_id==old.record_id).expect("原记录仍应存在");
            assert!(new.fields.get("计划完成日期").map_or(true, |v| v.is_null() || v==&json!([]) || v==&json!("")), "新列应保持空值");
            for f in &fields {
                if !is_formula_field(f) && f.field_type!=1002 && f.ui_type!="ModifiedTime" {
                    assert!(old.fields.get(&f.field_name)==new.fields.get(&f.field_name), "原记录字段内容发生变化；未覆盖任何任务");
                }
            }
        }
        println!("APPROVED_DATE_FIELD verified; mutation_count=1; field_count={}; records_before={}; records_after={}; original_fields_unchanged=true; original_stored_record_values_unchanged=true; date_column_empty=true; auto_fill=false", after.len(),records_before.len(),records_after.len());
    }

    #[test]
    #[cfg(feature = "feishu-release-ui-acceptance")]
    fn release_fresh_ui_credentials_do_not_share_formal_service() {
        assert_eq!(
            CREDENTIAL_SERVICE,
            "app.snaptask.desktop.release060fresh"
        );
        assert_ne!(CREDENTIAL_SERVICE, "app.snaptask.desktop");
    }

    #[test]
    #[cfg(not(any(feature = "feishu-release-ui-acceptance", feature = "feishu-direct-account-acceptance")))]
    fn formal_credential_service_remains_backward_compatible() {
        assert_eq!(CREDENTIAL_SERVICE, "app.snaptask.desktop");
        assert!(validate_credential_profile("app.snaptask.desktop").is_ok());
    }

    #[test]
    fn direct_account_credentials_require_their_own_profile() {
        let direct = cfg!(feature = "feishu-direct-account-acceptance");
        let ui=cfg!(feature="feishu-direct-sync-ui-acceptance");
        let incompatible = cfg!(feature = "feishu-release-ui-acceptance")
            || cfg!(feature = "feishu-write-acceptance")
            || (!ui&&(cfg!(feature = "local-workbench-acceptance")||cfg!(feature="feishu-template-v2-acceptance")));
        assert_eq!(validate_credential_profile("app.snaptask.desktop.directfeishu20261020").is_ok(), direct && !ui && !incompatible);
        assert_eq!(validate_credential_profile(crate::direct_account_scope::SYNC_IDENTIFIER).is_ok(),direct && ui && !incompatible);
        if direct {
            assert_eq!(CREDENTIAL_SERVICE, "app.snaptask.desktop.directfeishu20261020");
            assert_eq!(USER_SESSION_CREDENTIAL,"feishu-user-session-v1");
            assert!(validate_credential_profile("app.snaptask.desktop").is_err());
            assert!(validate_credential_profile("app.snaptask.desktop.localworkbench.syncs3").is_err());
        }
    }

    #[test]
    #[cfg(feature = "feishu-release-ui-acceptance")]
    fn release_fresh_ui_requires_its_identity_and_read_only_features() {
        assert!(validate_credential_profile("app.snaptask.desktop").is_err());
        let result = validate_credential_profile("app.snaptask.desktop.release060fresh");
        assert_eq!(
            result.is_ok(),
            !cfg!(feature = "feishu-write-acceptance")
                && !cfg!(feature = "feishu-template-v2-acceptance")
        );
    }

    #[test]
    fn formula_workspace_does_not_require_automation_reader_permission() {
        let mut calls = 0;
        let result = inspect_status_workflows(true, || {
            calls += 1;
            Err("飞书读取自动化失败：Forbidden（代码 91403）".to_string())
        });
        assert!(result
            .expect("formula status is independent of automations")
            .is_empty());
        assert_eq!(
            calls, 0,
            "must not request unnecessary automation configuration"
        );
    }

    #[test]
    fn single_select_workspace_keeps_automation_reader_permission_guard() {
        let mut calls = 0;
        let result = inspect_status_workflows(false, || {
            calls += 1;
            Err("飞书读取自动化失败：Forbidden（代码 91403）".to_string())
        });
        assert!(result
            .expect_err("legacy automation error remains visible")
            .contains("91403"));
        assert_eq!(calls, 1);
    }
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    #[cfg(windows)]
    static VAULT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn progress_workflows_must_all_exist_and_be_enabled() {
        assert!(required_progress_workflow_issues(&[]).is_empty());
        assert_eq!(required_progress_workflow_issues_strict(&[]).len(), 3);

        let enabled = REQUIRED_PROGRESS_WORKFLOWS
            .iter()
            .map(|name| FeishuWorkspaceItem {
                name: (*name).to_string(),
                kind: "Enable".to_string(),
            })
            .collect::<Vec<_>>();
        assert!(required_progress_workflow_issues(&enabled).is_empty());

        let mut copied = enabled;
        copied[0].kind = "Disable".to_string();
        copied.pop();
        let issues = required_progress_workflow_issues(&copied);
        assert_eq!(issues.len(), 2);
        assert!(issues.iter().any(|issue| issue.contains("未启用")));
        assert!(issues.iter().any(|issue| issue.contains("缺少")));
    }

    #[test]
    fn formula_status_completes_linkage_without_copied_workflows() {
        assert!(status_linkage_issues(true, &[], true).is_empty());
        assert!(status_linkage_issues(true, &[], false).is_empty());
        assert_eq!(status_linkage_issues(false, &[], true).len(), 3);
        assert!(status_linkage_issues(false, &[], false).is_empty());
    }

    #[test]
    fn task_mutations_fail_before_http_without_an_initialized_acceptance_scope() {
        let config = FeishuConfig {
            app_id: crate::write_acceptance::APP_ID.to_string(),
            app_token: crate::write_acceptance::APP_TOKEN.to_string(),
            table_id: crate::write_acceptance::TABLE_ID.to_string(),
            secret_configured: true,
        };
        for request in [
            FeishuWriteRequest::CreateTemporary {
                title: "【验收】not sent".to_string(),
                description: "fixture".to_string(),
                progress: 20,
                quadrant: "iu".to_string(),
                today: false,
                planned_date: None,
            },
            FeishuWriteRequest::Update {
                record_id: "not-a-real-record".to_string(),
                progress: 70,
                status: None,
                planned_date: None,
            },
            FeishuWriteRequest::Delete {
                record_id: "not-a-real-record".to_string(),
            },
        ] {
            let error = write_task_with_access_token(&config, "not-a-token", &request).unwrap_err();
            assert!(error.contains("本次未发送写入请求"));
        }
    }

    struct MockResponse {
        status: u16,
        body: String,
        close_without_response: bool,
    }

    fn mock_server(
        responses: Vec<MockResponse>,
    ) -> (String, thread::JoinHandle<Vec<(String, String)>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock Feishu API");
        let address = listener.local_addr().expect("mock address");
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (stream, _) = listener.accept().expect("accept request");
                let mut reader = BufReader::new(stream.try_clone().expect("clone mock stream"));
                let mut request_line = String::new();
                reader
                    .read_line(&mut request_line)
                    .expect("read request line");
                let mut content_length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).expect("read request header");
                    if header == "\r\n" || header.is_empty() {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':') {
                        if name.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse().expect("content length");
                        }
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).expect("read request body");
                requests.push((
                    request_line.trim().to_string(),
                    String::from_utf8(body).unwrap(),
                ));

                if !response.close_without_response {
                    let status_text = if response.status == 200 {
                        "OK"
                    } else {
                        "Error"
                    };
                    write!(
                        &stream,
                        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        response.status,
                        status_text,
                        response.body.len(),
                        response.body
                    )
                    .expect("write mock response");
                }
            }
            requests
        });
        (format!("http://{address}/open-apis"), server)
    }

    fn mock_json(body: &str) -> MockResponse {
        MockResponse {
            status: 200,
            body: body.to_string(),
            close_without_response: false,
        }
    }

    #[test]
    fn floating_send_is_once_and_distinguishes_rejection_from_uncertainty(){
        for (response,expected) in [(mock_json(r#"{"code":0,"msg":"ok","data":{"record":{"record_id":"recAck"}}}"#),"ok"),(MockResponse{status:403,body:r#"{"code":91403,"msg":"Forbidden"}"#.into(),close_without_response:false},"rejected"),(MockResponse{status:500,body:"invalid".into(),close_without_response:false},"uncertain")]{
            let (root,server)=mock_server(vec![response]);
            let p=PreparedFloatingMutation{config:FeishuConfig{app_id:"cli_test".into(),app_token:"base_test".into(),table_id:"tbl_test".into(),secret_configured:false},token:"synthetic".into(),request:FeishuWriteRequest::CreateTemporary{title:"fixture".into(),description:"detail".into(),progress:25,quadrant:"iu".into(),today:false,planned_date:None},payload:json!({"fields":{"任务名称":"fixture"}})};
            match (send_floating_mutation_at(&root,p),expected){(Ok(Some(id)),"ok")=>assert_eq!(id,"recAck"),(Err(e),"rejected")=>assert!(e.definite),(Err(e),"uncertain")=>assert!(!e.definite),_=>panic!("unexpected disposition")};
            let requests=server.join().unwrap();assert_eq!(requests.len(),1);assert!(requests[0].0.starts_with("POST /open-apis/bitable/v1/apps/base_test/tables/tbl_test/records"));
        }
    }
    #[test]
    fn floating_prepare_rejects_unverified_identity_and_wrong_target_without_io(){
        let c=FeishuConfig{app_id:"cli_test".into(),app_token:"base_test".into(),table_id:"tbl_test".into(),secret_configured:false};
        let read=SyncInspection{read_only:true,complete:true,account_name:"fictional".into(),subject_id:"different:user".into(),app_id:c.app_id.clone(),app_token:c.app_token.clone(),table_id:c.table_id.clone(),checked_at:0,records:vec![],conditional_write_verified:false,warnings:vec![]};
        assert!(prepare_floating_mutation(&c,"synthetic",&read,FeishuWriteRequest::Delete{record_id:"recTest".into()}).is_err());
    }

    fn formula_task_field_response() -> MockResponse {
        let items = required_task_fields().iter().map(|(name, _)| {
            json!({ "field_name": name, "type": if name == "状态" { 20 } else { 1 } })
        }).collect::<Vec<_>>();
        mock_json(
            &json!({"code":0,"msg":"success","data":{"items":items,"has_more":false}}).to_string(),
        )
    }

    #[test]
    fn formula_record_omission_is_reread_instead_of_cached_as_blank_status() {
        let (api_root, server) = mock_server(vec![
            formula_task_field_response(),
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"record_id":"rec1","fields":{"任务名称":"公式验收任务","进度":"0.57"}}],"has_more":false}}"#,
            ),
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"record_id":"rec1","fields":{"任务名称":"公式验收任务","进度":"0.57","状态":[{"text":"进行中","type":"text"}]}}],"has_more":false}}"#,
            ),
        ]);
        let (tasks, _) = fetch_tasks_with_access_token_at(
            &api_root,
            &FeishuConfig {
                app_token: "test-app".into(),
                table_id: "test-table".into(),
                ..FeishuConfig::default()
            },
            "user-token-test",
            123,
        )
        .expect("formula result becomes readable");
        assert_eq!(tasks[0].status, "进行中");
        assert_eq!(tasks[0].progress, 57);
        assert_eq!(server.join().expect("mock server completes").len(), 3);
    }

    #[test]
    fn persistent_formula_omission_fails_instead_of_returning_an_incomplete_snapshot() {
        let missing = r#"{"code":0,"msg":"success","data":{"items":[{"record_id":"rec1","fields":{"任务名称":"公式验收任务","进度":"0.57"}}],"has_more":false}}"#;
        let mut responses = vec![formula_task_field_response()];
        responses.extend((0..5).map(|_| mock_json(missing)));
        let (api_root, server) = mock_server(responses);
        let error = fetch_tasks_with_access_token_at(
            &api_root,
            &FeishuConfig {
                app_token: "test-app".into(),
                table_id: "test-table".into(),
                ..FeishuConfig::default()
            },
            "user-token-test",
            123,
        )
        .expect_err("blank formula snapshots cannot replace cached tasks");
        assert!(error.contains("公式状态"));
        assert!(error.contains("未使用空状态替换本地缓存"));
        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 6);
        assert!(requests
            .iter()
            .all(|(request, _)| request.starts_with("GET ")));
    }

    #[test]
    fn extracts_folder_token_only_from_secure_feishu_folder_links() {
        assert_eq!(
            folder_token_from_link(
                "https://talen.feishu.cn/drive/folder/fldcnFolder_123?from=copy"
            )
            .expect("Feishu folder link is accepted"),
            "fldcnFolder_123"
        );
        assert_eq!(
            folder_token_from_link("https://tenant.larksuite.com/drive/folder/fldFolder-456")
                .expect("Lark folder link is accepted"),
            "fldFolder-456"
        );
    }

    #[test]
    fn rejects_insecure_or_non_folder_destination_links() {
        for link in [
            "http://tenant.feishu.cn/drive/folder/fldFolder123",
            "https://feishu.cn.evil.example/drive/folder/fldFolder123",
            "https://tenant.feishu.cn/drive/file/fldFolder123",
            "https://tenant.feishu.cn/drive/folder/not_a_valid_token!",
        ] {
            assert!(folder_token_from_link(link).is_err(), "accepted {link}");
        }
    }

    #[test]
    fn accepts_an_empty_task_table_in_a_copied_template() {
        let (api_root, server) = mock_server(vec![mock_json(
            r#"{"code":0,"msg":"success","data":{"items":[],"has_more":false}}"#,
        )]);

        ensure_empty_task_table_at(&api_root, "bascopy01", "tbltask01", "user-token")
            .expect("empty copied task table is accepted");

        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 1);
        assert!(requests[0].0.contains("page_size=1"));
        assert!(requests[0]
            .0
            .starts_with("GET /open-apis/bitable/v1/apps/bascopy01/tables/tbltask01/records?"));
    }

    #[test]
    fn rejects_a_copied_template_that_contains_task_records() {
        let (api_root, server) = mock_server(vec![mock_json(
            r#"{"code":0,"msg":"success","data":{"items":[{"record_id":"rec1","fields":{"任务名称":"示例任务"}}],"has_more":true,"page_token":"next"}}"#,
        )]);

        let error = ensure_empty_task_table_at(&api_root, "bascopy01", "tbltask01", "user-token")
            .expect_err("template records must block connection completion");

        assert!(error.contains("至少包含 1 条任务记录"));
        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 1, "stop reading after the first record");
    }

    #[test]
    fn copies_verified_empty_template_with_dashboard_content_and_parses_result() {
        let (api_root, server) = mock_server(vec![mock_json(
            r#"{"code":0,"msg":"success","data":{"app":{"app_token":"bascopy01","name":"我的任务","url":"https://example.feishu.cn/base/bascopy01"}}}"#,
        )]);
        let copied = copy_task_workspace_template_at(
            &api_root,
            "bastemplate1",
            "user-access-token-test",
            "我的任务",
            Some("fld-folder1"),
        )
        .expect("copy succeeds");

        assert_eq!(copied.app_token, "bascopy01");
        assert_eq!(copied.name, "我的任务");
        assert_eq!(
            copied.url.as_deref(),
            Some("https://example.feishu.cn/base/bascopy01")
        );
        let requests = server.join().expect("mock server completes");
        assert_eq!(
            requests[0].0,
            "POST /open-apis/bitable/v1/apps/bastemplate1/copy HTTP/1.1"
        );
        let body: Value = serde_json::from_str(&requests[0].1).expect("copy body JSON");
        assert_eq!(body["name"], "我的任务");
        assert_eq!(body["folder_token"], "fld-folder1");
        assert_eq!(body["without_content"], false);
        assert_eq!(body["time_zone"], "Asia/Shanghai");
    }

    #[test]
    fn checks_all_source_tables_are_empty_before_full_copy() {
        let (api_root, server) = mock_server(vec![
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"table_id":"tbl-task","name":"任务清单"},{"table_id":"tbl-extra","name":"附表"}],"has_more":false}}"#,
            ),
            mock_json(r#"{"code":0,"msg":"success","data":{"items":[],"has_more":false}}"#),
            mock_json(r#"{"code":0,"msg":"success","data":{"items":[],"has_more":false}}"#),
        ]);
        ensure_template_tables_empty_at(&api_root, "bastemplate1", "user-token")
            .expect("every source table is empty");
        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 3);
        assert!(requests[1].0.contains("tbl-task/records?page_size=1"));
        assert!(requests[2].0.contains("tbl-extra/records?page_size=1"));
    }

    #[test]
    fn blocks_copy_when_any_source_table_contains_records() {
        let (api_root, server) = mock_server(vec![
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"table_id":"tbl-task","name":"任务清单"}],"has_more":false}}"#,
            ),
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"record_id":"rec1","fields":{"任务名称":"私有任务"}}],"has_more":false}}"#,
            ),
        ]);
        let error = ensure_template_tables_empty_at(&api_root, "bastemplate1", "user-token")
            .expect_err("source records must block copy");
        assert!(error.contains("任务清单"));
        assert!(error.contains("至少包含 1 条任务记录"));
        assert_eq!(server.join().expect("mock server completes").len(), 2);
    }

    #[test]
    fn refreshes_user_session_with_feishu_app_access_token_and_rotating_refresh_token() {
        let (api_root, server) = mock_server(vec![
            mock_json(r#"{"code":0,"app_access_token":"app-access-test","expire":7200}"#),
            mock_json(
                r#"{"code":0,"data":{"access_token":"new-user-access","expires_in":7140,"refresh_token":"new-refresh","refresh_token_expires_in":604800}}"#,
            ),
        ]);
        let refreshed = refresh_user_access_token_at(
            &api_root,
            "cli-test",
            "app-secret-test",
            "old-refresh-token",
        )
        .expect("refresh succeeds");

        assert_eq!(refreshed.access_token, "new-user-access");
        assert_eq!(refreshed.refresh_token.as_deref(), Some("new-refresh"));
        assert_eq!(refreshed.refresh_token_expires_in, Some(604800));
        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].0,
            "POST /open-apis/auth/v3/app_access_token/internal HTTP/1.1"
        );
        let app_body: Value = serde_json::from_str(&requests[0].1).expect("app body JSON");
        assert_eq!(app_body["app_id"], "cli-test");
        assert_eq!(app_body["app_secret"], "app-secret-test");
        assert_eq!(
            requests[1].0,
            "POST /open-apis/authen/v1/refresh_access_token HTTP/1.1"
        );
        let refresh_body: Value = serde_json::from_str(&requests[1].1).expect("refresh body JSON");
        assert_eq!(refresh_body["grant_type"], "refresh_token");
        assert_eq!(refresh_body["refresh_token"], "old-refresh-token");
        assert!(refresh_body.get("app_secret").is_none());
    }

    #[test]
    fn refreshes_v2_user_session_with_rotating_refresh_token() {
        let (api_root, server) = mock_server(vec![mock_json(
            r#"{"access_token":"new-v2-access","refresh_token":"new-v2-refresh","expires_in":7100,"refresh_token_expires_in":604800}"#,
        )]);
        let refreshed = refresh_user_access_token_v2_at(
            &api_root,
            "cli-test",
            "app-secret-test",
            "old-v2-refresh",
        )
        .expect("v2 refresh succeeds");

        assert_eq!(refreshed.access_token, "new-v2-access");
        assert_eq!(refreshed.refresh_token.as_deref(), Some("new-v2-refresh"));
        assert_eq!(refreshed.refresh_token_expires_in, Some(604800));
        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].0,
            "POST /open-apis/authen/v2/oauth/token HTTP/1.1"
        );
        assert!(requests[0].1.contains("grant_type=refresh_token"));
        assert!(requests[0].1.contains("client_id=cli-test"));
        assert!(requests[0].1.contains("client_secret=app-secret-test"));
        assert!(requests[0].1.contains("refresh_token=old-v2-refresh"));
    }

    #[test]
    fn old_saved_feishu_sessions_default_to_legacy_refresh_version() {
        let session: FeishuUserSessionRecord = serde_json::from_str(
            r#"{"accessToken":"access","refreshToken":"refresh","accessExpiresAt":10,"refreshExpiresAt":20}"#,
        )
        .expect("legacy session remains readable");
        assert_eq!(session.auth_version, "v1");
        assert!(validate_session_app_id(&session, "cli-new").is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn long_user_session_can_be_saved_without_exposing_tokens() {
        let _guard = VAULT_TEST_LOCK.lock().expect("serialize vault tests");
        let credential_user = format!("feishu-user-session-test-{}", std::process::id());
        let entry = keyring::Entry::new(CREDENTIAL_SERVICE, &credential_user)
            .expect("create isolated test credential");
        let _ = entry.delete_credential();
        let input = SaveFeishuUserSessionInput {
            app_id: Some("cli_test".to_string()),
            access_token: "access".repeat(190),
            refresh_token: "refresh".repeat(110),
            expires_in: 7200,
            refresh_expires_in: Some(604800),
        };
        let result = save_user_session_with_version_at(&input, "v2", &credential_user);
        assert!(result.is_ok(), "long session save failed: {result:?}");
        let saved = read_user_session_at(&credential_user).expect("read long session back");
        assert_eq!(saved.access_token, input.access_token);
        assert_eq!(saved.refresh_token, input.refresh_token);
        let old_manifest =
            parse_session_manifest(&entry.get_password().expect("read test manifest"))
                .expect("parse test manifest")
                .expect("new session uses chunks");
        let rotated = SaveFeishuUserSessionInput {
            access_token: "new-access".repeat(180),
            refresh_token: "new-refresh".repeat(120),
            ..input
        };
        save_user_session_with_version_at(&rotated, "v2", &credential_user)
            .expect("replace long session after token refresh");
        let saved = read_user_session_at(&credential_user).expect("read rotated session");
        assert_eq!(saved.access_token, rotated.access_token);
        assert_eq!(saved.refresh_token, rotated.refresh_token);
        for index in 0..old_manifest.count {
            let chunk = keyring::Entry::new(
                CREDENTIAL_SERVICE,
                &session_chunk_user(&credential_user, &old_manifest.generation, index),
            )
            .expect("open old test chunk");
            assert!(matches!(chunk.get_secret(), Err(keyring::Error::NoEntry)));
        }
        let manifest = parse_session_manifest(&entry.get_password().expect("read test manifest"))
            .expect("parse test manifest")
            .expect("rotated session uses chunks");
        clear_user_session_at(&credential_user).expect("remove isolated test credential");
        for index in 0..manifest.count {
            let chunk = keyring::Entry::new(
                CREDENTIAL_SERVICE,
                &session_chunk_user(&credential_user, &manifest.generation, index),
            )
            .expect("open test chunk");
            assert!(matches!(chunk.get_secret(), Err(keyring::Error::NoEntry)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn multi_chunk_user_session_round_trips_through_windows_vault() {
        let _guard = VAULT_TEST_LOCK.lock().expect("serialize vault tests");
        let credential_user = format!("feishu-user-session-large-test-{}", std::process::id());
        let input = SaveFeishuUserSessionInput {
            app_id: Some("cli_test".to_string()),
            access_token: "access".repeat(800),
            refresh_token: "refresh".repeat(450),
            expires_in: 7200,
            refresh_expires_in: Some(604800),
        };
        save_user_session_with_version_at(&input, "v2", &credential_user)
            .expect("save multi-chunk session");
        let saved = read_user_session_at(&credential_user).expect("read multi-chunk session");
        assert_eq!(saved.access_token, input.access_token);
        assert_eq!(saved.refresh_token, input.refresh_token);
        let entry =
            keyring::Entry::new(CREDENTIAL_SERVICE, &credential_user).expect("open test manifest");
        let manifest = parse_session_manifest(&entry.get_password().expect("read test manifest"))
            .expect("parse test manifest")
            .expect("new session uses chunks");
        clear_user_session_at(&credential_user).expect("remove isolated test credential");
        for index in 0..manifest.count {
            let chunk = keyring::Entry::new(
                CREDENTIAL_SERVICE,
                &session_chunk_user(&credential_user, &manifest.generation, index),
            )
            .expect("open test chunk");
            assert!(matches!(chunk.get_secret(), Err(keyring::Error::NoEntry)));
        }
    }

    #[cfg(windows)]
    #[test]
    fn legacy_single_credential_session_remains_readable() {
        let _guard = VAULT_TEST_LOCK.lock().expect("serialize vault tests");
        let credential_user = format!("feishu-user-session-legacy-test-{}", std::process::id());
        let entry = keyring::Entry::new(CREDENTIAL_SERVICE, &credential_user)
            .expect("create isolated test credential");
        let _ = entry.delete_credential();
        entry
            .set_password(
                r#"{"accessToken":"legacy-access","refreshToken":"legacy-refresh","accessExpiresAt":10,"refreshExpiresAt":20}"#,
            )
            .expect("store legacy session");
        let saved = read_user_session_at(&credential_user).expect("read legacy session");
        assert_eq!(saved.auth_version, "v1");
        assert_eq!(saved.access_token, "legacy-access");
        clear_user_session_at(&credential_user).expect("remove isolated test credential");
    }

    #[test]
    fn v2_feishu_session_cannot_be_reused_with_another_app_id() {
        let session = FeishuUserSessionRecord {
            auth_version: "v2".to_string(),
            auth_app_id: Some("cli-original".to_string()),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            access_expires_at: 10,
            refresh_expires_at: Some(20),
        };
        assert!(validate_session_app_id(&session, "cli-original").is_ok());
        assert!(validate_session_app_id(&session, "cli-other").is_err());
        assert!(session_authorized_for_app(&session, "cli-original", 5));
        assert!(session_authorized_for_app(&session, "cli-original", 15));
        assert!(!session_authorized_for_app(&session, "cli-original", 25));
        assert!(!session_authorized_for_app(&session, "cli-other", 5));
        assert!(!session_authorized_for_app(&session, "", 5));

        let unbound_session = FeishuUserSessionRecord {
            auth_app_id: None,
            ..session
        };
        assert!(validate_session_app_id(&unbound_session, "cli-original").is_err());
        assert!(!session_authorized_for_app(
            &unbound_session,
            "cli-original",
            5
        ));
    }

    #[test]
    fn reports_unknown_copy_delivery_instead_of_suggesting_a_retry() {
        let (api_root, server) = mock_server(vec![MockResponse {
            status: 200,
            body: String::new(),
            close_without_response: true,
        }]);
        let error = copy_task_workspace_template_at(
            &api_root,
            "bastemplate1",
            "user-access-token-test",
            "我的任务",
            None,
        )
        .expect_err("missing response is uncertain");
        assert!(matches!(error, FeishuTemplateCopyError::OutcomeUnknown(_)));
        assert_eq!(server.join().expect("mock server completes").len(), 1);
    }

    #[test]
    fn discovers_task_table_by_manifest_fields_and_accepts_category_alias() {
        let (api_root, server) = mock_server(vec![
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"table_id":"tbl-target","name":"个人事项"}],"has_more":false}}"#,
            ),
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"field_name":"任务名称"},{"field_name":"任务类别"},{"field_name":"状态"},{"field_name":"进度"}],"has_more":false}}"#,
            ),
        ]);
        let table = discover_task_table_at(&api_root, "bascopy01", "user-access-token-test")
            .expect("task table discovered");

        assert_eq!(table.table_id, "tbl-target");
        assert_eq!(table.name, "个人事项");
        assert!(table.fields.contains(&"任务类别".to_string()));
        let requests = server.join().expect("mock server completes");
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0]
                .0
                .starts_with("GET /open-apis/bitable/v1/apps/bascopy01/tables?"),
            "unexpected tables request: {:?}",
            requests[0].0
        );
        assert_eq!(
            requests[1].0,
            "GET /open-apis/bitable/v1/apps/bascopy01/tables/tbl-target/fields?page_size=100 HTTP/1.1"
        );
    }

    #[test]
    fn does_not_guess_between_multiple_matching_task_tables() {
        let (api_root, server) = mock_server(vec![
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"table_id":"tbl-one","name":"任务清单A"},{"table_id":"tbl-two","name":"任务清单B"}],"has_more":false}}"#,
            ),
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"field_name":"任务名称"},{"field_name":"任务类型"},{"field_name":"状态"},{"field_name":"进度"}],"has_more":false}}"#,
            ),
            mock_json(
                r#"{"code":0,"msg":"success","data":{"items":[{"field_name":"任务名称"},{"field_name":"任务标签"},{"field_name":"状态"},{"field_name":"进度"}],"has_more":false}}"#,
            ),
        ]);
        let error = discover_task_table_at(&api_root, "bascopy01", "user-access-token-test")
            .expect_err("ambiguous result must not pick arbitrarily");
        assert!(error.contains("多个"));
        assert_eq!(server.join().expect("mock server completes").len(), 3);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads only fields, views, dashboards and automations of an explicitly selected Feishu template"]
    fn live_official_template_metadata_as_user() {
        assert!(
            user_auth_status().authorized,
            "user session is not authorized"
        );
        let app_id = read_user_session()
            .expect("read saved user session")
            .auth_app_id
            .expect("session is bound to its authorization app");
        let access_token = read_user_access_token(&app_id).expect("read saved user authorization");
        let app_token = std::env::var("PROJECT_TRACK_FEISHU_TEMPLATE_APP_TOKEN")
            .expect("template app token env");
        let response = client()
            .expect("HTTP client")
            .get(format!("{API_ROOT}/bitable/v1/apps/{app_token}/tables"))
            .bearer_auth(&access_token)
            .send()
            .expect("list template tables request");
        let tables: TableData =
            parse_api_response(response, "读取模板数据表列表").expect("list template tables");
        assert!(
            !tables.has_more,
            "template has more than one page of tables"
        );
        println!("Template tables: {}", tables.items.len());
        for table in tables.items {
            let fields = list_field_items_at(API_ROOT, &app_token, &table.table_id, &access_token)
                .expect("list template fields");
            let field_names = fields
                .iter()
                .map(|field| format!("{} ({})", field.field_name, field.ui_type))
                .collect::<Vec<_>>();
            let config = FeishuConfig {
                app_token: app_token.clone(),
                table_id: table.table_id,
                ..FeishuConfig::default()
            };
            let views = list_views(&config, &access_token).expect("list template views");
            let view_names = views
                .iter()
                .map(|view| format!("{} ({})", view.name, view.kind))
                .collect::<Vec<_>>();
            println!(
                "Table {}: fields={field_names:?}; views={view_names:?}",
                table.name
            );
            println!(
                "Table {} empty: {:?}",
                table.name,
                ensure_task_table_empty(&app_token, &config.table_id, &access_token)
            );
        }
        let config = FeishuConfig {
            app_token,
            ..FeishuConfig::default()
        };
        println!("Dashboards: {:?}", list_dashboards(&config, &access_token));
        println!("Automations: {:?}", list_workflows(&config, &access_token));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "prints only non-secret field type metadata for an explicitly selected Feishu task workspace"]
    fn live_formula_field_metadata_as_user() {
        assert!(
            user_auth_status().authorized,
            "user session is not authorized"
        );
        let app_id = read_user_session()
            .expect("read saved user session")
            .auth_app_id
            .expect("session is bound to its authorization app");
        let access_token = read_user_access_token(&app_id).expect("read saved user authorization");
        let app_token = std::env::var("PROJECT_TRACK_FEISHU_TEMPLATE_APP_TOKEN")
            .expect("explicit task workspace app token env");
        let table_id = std::env::var("PROJECT_TRACK_FEISHU_TABLE_ID").unwrap_or_else(|_| {
            discover_task_table(&app_token, &access_token)
                .expect("discover task table")
                .table_id
        });
        println!("task table id: {table_id}");
        let fields = list_field_items_at(API_ROOT, &app_token, &table_id, &access_token)
            .expect("list task fields");
        for field in fields {
            if field.field_name == "状态" {
                let mut property_keys = field
                    .property
                    .as_object()
                    .map(|object| object.keys().cloned().collect::<Vec<_>>())
                    .unwrap_or_default();
                property_keys.sort();
                println!(
                    "status field metadata: type={}; ui_type={:?}; property_keys={property_keys:?}",
                    field.field_type, field.ui_type
                );
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "reads only the explicitly named synthetic promotion acceptance record"]
    fn live_promotion_formula_record_as_user() {
        let app_id = read_user_session()
            .expect("read saved user session")
            .auth_app_id
            .expect("session is bound to its authorization app");
        let access_token = read_user_access_token(&app_id).expect("read saved user authorization");
        let config = FeishuConfig {
            app_token: crate::write_acceptance::TEMPLATE_V2_PROMOTION_APP_TOKEN.to_string(),
            table_id: crate::write_acceptance::TEMPLATE_V2_PROMOTION_TABLE_ID.to_string(),
            ..FeishuConfig::default()
        };
        for attempt in 1..=3 {
            let records = list_records(&config, &access_token).expect("read test records");
            let record = records
                .iter()
                .find(|record| text(record.fields.get("任务名称")) == "【候选源复验】模板联动示例")
                .expect("synthetic example record exists");
            println!(
                "read {attempt}: progress={:?}; status={:?}; parsed_status={:?}",
                record.fields.get("进度"),
                record.fields.get("状态"),
                text(record.fields.get("状态"))
            );
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "read-only empty-source preflight for an explicitly selected Feishu template"]
    fn live_empty_template_preflight_as_user() {
        assert!(
            user_auth_status().authorized,
            "user session is not authorized"
        );
        let app_id = read_user_session()
            .expect("read saved user session")
            .auth_app_id
            .expect("session is bound to its authorization app");
        let access_token = read_user_access_token(&app_id).expect("read saved user authorization");
        let app_token = std::env::var("PROJECT_TRACK_FEISHU_TEMPLATE_APP_TOKEN")
            .expect("explicitly selected template token env");
        ensure_template_tables_empty(&app_token, &access_token)
            .expect("all source tables are empty before copying");
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "read-only structural validation of an explicitly selected empty Feishu task workspace"]
    fn live_validate_empty_task_workspace_as_user() {
        assert!(
            user_auth_status().authorized,
            "user session is not authorized"
        );
        let app_id = read_user_session()
            .expect("read saved user session")
            .auth_app_id
            .expect("session is bound to its authorization app");
        let access_token = read_user_access_token(&app_id).expect("read saved user authorization");
        let app_token = std::env::var("PROJECT_TRACK_FEISHU_TEMPLATE_APP_TOKEN")
            .expect("explicitly selected task workspace token env");
        let table = discover_task_table(&app_token, &access_token).expect("discover task table");
        assert_eq!(table.name, "任务清单");
        ensure_task_table_empty(&app_token, &table.table_id, &access_token)
            .expect("task table has zero records");
        let status = inspect_workspace_as_user(&app_token, &table.table_id, &access_token)
            .expect("inspect copied task workspace");
        assert!(status.missing_required_fields.is_empty());
        assert!(status.inspection_warnings.is_empty());
        let status_board_name = if status.status_is_formula {
            "今日必做看板"
        } else {
            "状态看板"
        };
        for (name, kind) in [
            ("任务总览", "grid"),
            ("四象限看板", "kanban"),
            (status_board_name, "kanban"),
            ("今日任务", "grid"),
        ] {
            assert!(status
                .views
                .iter()
                .any(|view| view.name == name && view.kind == kind));
        }
        assert!(status
            .dashboards
            .iter()
            .any(|dashboard| dashboard.name == "个人任务仪表盘"));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "read-only assertion of the single synthetic record in the final isolated copy"]
    fn live_final_acceptance_snapshot_as_user() {
        let app_id = read_user_session()
            .expect("saved session")
            .auth_app_id
            .expect("bound app");
        assert_eq!(app_id, crate::write_acceptance::TEMPLATE_V2_APP_ID);
        let access_token = read_user_access_token(&app_id).expect("saved authorization");
        let config = FeishuConfig {
            app_id,
            app_token: crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN.to_string(),
            table_id: crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID.to_string(),
            secret_configured: true,
        };
        let (tasks, _) = fetch_tasks_with_access_token_at(API_ROOT, &config, &access_token, 0)
            .expect("complete cloud snapshot");
        assert_eq!(
            tasks.len(),
            1,
            "no duplicate records in the final isolated copy"
        );
        let task = &tasks[0];
        assert_eq!(task.title, "【完整性复验】模板联动示例");
        assert_eq!(
            task.id, "rec0000000000001",
            "the original synthetic record must be updated, not replaced"
        );
        let expected: u8 = std::env::var("PROJECT_TRACK_FEISHU_EXPECTED_PROGRESS")
            .expect("explicit expected progress")
            .parse()
            .expect("integer progress");
        assert!((0..=100).contains(&expected));
        assert_eq!(task.progress, expected);
        let expected_status = if expected == 100 {
            "已完成"
        } else if expected == 0 {
            "未开始"
        } else {
            "进行中"
        };
        assert_eq!(task.status, expected_status);
        let expected_date = std::env::var("PROJECT_TRACK_FEISHU_EXPECTED_PLANNED_DATE")
            .unwrap_or_else(|_| "2026-10-10".to_string());
        assert!(
            matches!(expected_date.as_str(), "2026-10-10" | "2026-10-20"),
            "only the two synthetic acceptance dates are allowed"
        );
        assert_eq!(task.planned_date.as_deref(), Some(expected_date.as_str()));
        println!(
            "FINAL_SNAPSHOT {}",
            serde_json::json!({
                "recordId": task.id, "progress": task.progress,
                "status": task.status, "plannedDate": task.planned_date, "recordCount": tasks.len()
            })
        );
    }

    #[cfg(all(
        windows,
        feature = "feishu-write-acceptance",
        feature = "feishu-template-v2-acceptance"
    ))]
    #[test]
    #[ignore = "changes only the exact final synthetic record date to simulate a concurrent cloud edit; never retries"]
    fn live_final_acceptance_external_date_change() {
        let app_id = read_user_session()
            .expect("saved session")
            .auth_app_id
            .expect("bound app");
        assert_eq!(app_id, crate::write_acceptance::TEMPLATE_V2_APP_ID);
        let access_token = read_user_access_token(&app_id).expect("saved authorization");
        let config = FeishuConfig {
            app_id,
            app_token: crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN.to_string(),
            table_id: crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID.to_string(),
            secret_configured: true,
        };
        let from = std::env::var("PROJECT_TRACK_FEISHU_EXTERNAL_DATE_FROM")
            .expect("explicit starting date");
        let to =
            std::env::var("PROJECT_TRACK_FEISHU_EXTERNAL_DATE_TO").expect("explicit target date");
        assert!(matches!(
            (from.as_str(), to.as_str()),
            ("2026-10-10", "2026-10-20") | ("2026-10-20", "2026-10-10")
        ));
        let (before, _) =
            fetch_tasks_with_access_token(&config, &access_token, 0).expect("preflight");
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].id, "rec0000000000001");
        assert_eq!(before[0].title, "【完整性复验】模板联动示例");
        assert_eq!(before[0].progress, 57);
        assert_eq!(before[0].status, "进行中");
        assert_eq!(
            before[0].planned_date.as_deref(),
            Some(from.as_str()),
            "stop instead of replaying a prior delivery"
        );
        crate::write_acceptance::configure(crate::write_acceptance::TEMPLATE_V2_FINAL_IDENTIFIER)
            .expect("isolated identity");
        let request = FeishuWriteRequest::Update {
            record_id: before[0].id.clone(),
            progress: 57,
            status: None,
            planned_date: Some(Some(to.clone())),
        };
        write_task_with_access_token(&config, &access_token, &request)
            .expect("one external update; inspect cloud if this fails, never retry blindly");
        let (after, _) = fetch_tasks_with_access_token(&config, &access_token, 0)
            .expect("readback; do not replay on failure");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, before[0].id);
        assert_eq!(after[0].progress, 57);
        assert_eq!(after[0].planned_date.as_deref(), Some(to.as_str()));
        println!(
            "EXTERNAL_DATE_SNAPSHOT {}",
            serde_json::json!({
                "recordId": after[0].id, "progress": after[0].progress,
                "plannedDate": after[0].planned_date, "recordCount": after.len(), "mutationCount": 1
            })
        );
    }

    #[test]
    #[ignore = "reads field names from the explicitly configured Feishu table"]
    fn live_test_table_returns_required_field_aliases() {
        let config = FeishuConfig {
            app_id: std::env::var("PROJECT_TRACK_FEISHU_APP_ID").expect("app id env"),
            app_token: std::env::var("PROJECT_TRACK_FEISHU_APP_TOKEN").expect("app token env"),
            table_id: std::env::var("PROJECT_TRACK_FEISHU_TABLE_ID").expect("table id env"),
            secret_configured: true,
        };
        let secret = read_secret().expect("saved app secret");
        let access_token = tenant_access_token(&config, &secret).expect("tenant token");
        let fields = list_fields(&config, &access_token).expect("field list");
        println!("Live Feishu fields: {fields:?}");
        assert!(
            missing_required_fields(&fields).is_empty(),
            "actual field list does not contain required names or aliases: {fields:?}"
        );
    }

    #[test]
    #[ignore = "inspects optional Feishu workspace metadata using the configured test table"]
    fn live_test_table_reports_optional_metadata_permission_gaps() {
        let config = FeishuConfig {
            app_id: std::env::var("PROJECT_TRACK_FEISHU_APP_ID").expect("app id env"),
            app_token: std::env::var("PROJECT_TRACK_FEISHU_APP_TOKEN").expect("app token env"),
            table_id: std::env::var("PROJECT_TRACK_FEISHU_TABLE_ID").expect("table id env"),
            secret_configured: true,
        };
        let secret = read_secret().expect("saved app secret");
        let status = inspect_workspace(&config, &secret).expect("inspect test workspace");
        println!("Live Feishu workspace status: {status:#?}");
        assert!(status.missing_required_fields.is_empty());
    }

    #[test]
    #[ignore = "mutates the explicitly configured Feishu test table and cleans up its probe record"]
    fn live_test_table_can_create_read_back_and_delete_probe_record() {
        let config = FeishuConfig {
            app_id: std::env::var("PROJECT_TRACK_FEISHU_APP_ID").expect("app id env"),
            app_token: std::env::var("PROJECT_TRACK_FEISHU_APP_TOKEN").expect("app token env"),
            table_id: std::env::var("PROJECT_TRACK_FEISHU_TABLE_ID").expect("table id env"),
            secret_configured: true,
        };
        let secret = read_secret().expect("saved app secret");
        let (before, readiness) = fetch_tasks(&config, &secret, 0).expect("read test table");
        assert_eq!(readiness.task_count, before.len());
        assert!(
            !before.is_empty(),
            "test copy should contain its template data"
        );

        let title = format!("Project Track 写入验收 {}", crate::storage::unix_now());
        let create = FeishuWriteRequest::CreateTemporary {
            title: title.clone(),
            description: "自动化写入、回读和清理验证".to_string(),
            progress: 0,
            quadrant: "iu".to_string(),
            today: false,
            planned_date: None,
        };
        let record_id = write_task(&config, &secret, &create)
            .expect("create probe record")
            .expect("created record id");
        let check = (|| {
            let (after_create, _) = fetch_tasks(&config, &secret, 0).expect("read created record");
            let created = after_create
                .iter()
                .find(|task| task.id == record_id)
                .expect("created record exists on readback");
            assert_eq!(created.title, title);
            assert_eq!(created.progress, 0);
            assert_eq!(created.status, "未开始");
            write_task(
                &config,
                &secret,
                &FeishuWriteRequest::Delete {
                    record_id: record_id.clone(),
                },
            )
            .expect("delete probe record");
            let (after_delete, _) = fetch_tasks(&config, &secret, 0).expect("verify deletion");
            assert!(after_delete.iter().all(|task| task.id != record_id));
            Ok::<(), String>(())
        })();
        if check.is_err() {
            let _ = write_task(&config, &secret, &FeishuWriteRequest::Delete { record_id });
        }
        check.expect("probe record round trip");
    }

    #[test]
    fn parses_feishu_rich_text_choices_numbers_and_dates() {
        let records = vec![RecordItem {
            record_id: "rec-1".to_string(),
            fields: serde_json::from_value(json!({
                "任务名称": [{"text": "飞书接入", "type": "text"}],
                "任务详情": "验证真实数据表",
                "任务类型": "重要不紧急",
                "状态": "进行中",
                "进度": 0.4,
                "今日必完成": true,
                "任务计划完成日期": 1790697600000_i64
            }))
            .expect("fields"),
        }];
        let tasks = parse_tasks(&records, 1);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "飞书接入");
        assert_eq!(tasks[0].progress, 40);
        assert_eq!(tasks[0].quadrant, "in");
        assert!(tasks[0].today);
        assert_eq!(tasks[0].planned_date.as_deref(), Some("2026-09-30"));
    }

    #[test]
    fn writes_native_percentage_as_fraction_and_plain_number_as_integer() {
        let percent_field: FieldItem = serde_json::from_value(json!({
            "field_name": "进度",
            "type": 2,
            "ui_type": "Progress",
            "property": { "formatter": "0%" }
        }))
        .expect("percentage field");
        assert_eq!(progress_write_value(&percent_field, 70), json!(0.7));

        let number_field: FieldItem = serde_json::from_value(json!({
            "field_name": "进度",
            "type": 2,
            "ui_type": "Number",
            "property": { "formatter": "0" }
        }))
        .expect("number field");
        assert_eq!(progress_write_value(&number_field, 70), json!(70));
    }

    #[test]
    fn update_only_writes_requested_status_and_planned_date_fields() {
        let fields: Vec<FieldItem> = serde_json::from_value(json!([
            { "field_name": "进度", "ui_type": "Progress", "property": { "formatter": "0%" } },
            { "field_name": "状态" },
            { "field_name": "任务计划完成日期", "ui_type": "Date" }
        ]))
        .expect("fields");
        let values = write_fields(
            &fields,
            &FeishuWriteRequest::Update {
                record_id: "rec-1".to_string(),
                progress: 75,
                status: Some("进行中".to_string()),
                planned_date: Some(Some("2026-10-15".to_string())),
            },
        )
        .expect("update fields");
        assert_eq!(values.get("进度"), Some(&json!(0.75)));
        assert_eq!(values.get("状态"), Some(&json!("进行中")));
        assert_eq!(
            values.get("任务计划完成日期"),
            Some(&json!(iso_date_to_unix_ms("2026-10-15").unwrap()))
        );
    }

    #[test]
    fn formula_status_is_derived_in_feishu_instead_of_written() {
        let fields: Vec<FieldItem> = serde_json::from_value(json!([
            { "field_name": "任务名称", "ui_type": "Text" },
            { "field_name": "进度", "ui_type": "Progress", "property": { "formatter": "0%" } },
            { "field_name": "状态", "ui_type": "Formula" },
            { "field_name": "任务类型", "ui_type": "SingleSelect" }
        ]))
        .expect("fields");

        assert!(has_formula_status_field(&fields));

        let updated = write_fields(
            &fields,
            &FeishuWriteRequest::Update {
                record_id: "rec-1".to_string(),
                progress: 57,
                status: Some("进行中".to_string()),
                planned_date: None,
            },
        )
        .expect("update fields");
        assert_eq!(updated.get("进度"), Some(&json!(0.57)));
        assert!(!updated.contains_key("状态"));

        let created = write_fields(
            &fields,
            &FeishuWriteRequest::CreateTemporary {
                title: "公式状态任务".to_string(),
                description: "".to_string(),
                progress: 100,
                quadrant: "重要且紧急".to_string(),
                today: false,
                planned_date: None,
            },
        )
        .expect("create fields");
        assert_eq!(created.get("进度"), Some(&json!(1.0)));
        assert!(!created.contains_key("状态"));
    }

    #[test]
    fn formula_status_accepts_feishu_numeric_field_type_without_ui_type() {
        let fields = serde_json::from_value::<FieldData>(serde_json::json!({
            "items": [
                { "field_name": "状态", "type": 20, "property": { "formula_expression": "IF([进度]>=1,\"已完成\",\"进行中\")" } },
                { "field_name": "进度", "type": 2, "ui_type": "Progress", "property": { "formatter": "0%" } }
            ]
        }))
        .unwrap()
        .items;

        assert!(has_formula_status_field(&fields));
    }

    #[test]
    fn formula_status_accepts_formula_property_with_text_result_type() {
        let fields = serde_json::from_value::<FieldData>(serde_json::json!({
            "items": [
                { "field_name": "状态", "type": 1, "property": { "formula_expression": "IF([进度]>=1,\"已完成\",\"进行中\")" } }
            ]
        }))
        .unwrap()
        .items;

        assert!(has_formula_status_field(&fields));
    }

    #[test]
    fn validates_planned_dates_before_converting_to_feishu_timestamp() {
        assert!(iso_date_to_unix_ms("2026-02-29").is_err());
        assert!(iso_date_to_unix_ms("2026-04-31").is_err());
        assert!(iso_date_to_unix_ms("2026-02-28").is_ok());
    }

    #[test]
    fn preserves_feishu_permission_diagnostics() {
        let payload: ApiResponse<Value> = serde_json::from_value(json!({
            "code": 91403,
            "msg": "Forbidden",
            "error": {
                "log_id": "20260929-diagnostic",
                "permission_violations": [
                    {
                        "subject": "base:field:read",
                        "type": "action_scope_required"
                    }
                ]
            }
        }))
        .expect("api response");

        let message = format_api_error("读取字段", &payload);
        assert!(message.contains("91403"));
        assert!(message.contains("base:field:read"));
        assert!(message.contains("20260929-diagnostic"));
        assert!(message.contains("文档协作者"));
    }

    #[test]
    fn explains_permission_failures_by_operation_instead_of_claiming_base_read_is_denied() {
        let payload: ApiResponse<Value> = serde_json::from_value(json!({
            "code": 91403,
            "msg": "Forbidden"
        }))
        .expect("api response");
        let automation = format_api_error("读取自动化", &payload);
        assert!(automation.contains("自动化"));
        assert!(!automation.contains("文档协作者"));

        let write = format_api_error("写入记录", &payload);
        assert!(write.contains("写入权限"));
        assert!(!write.contains("至少为可阅读"));
    }

    #[test]
    fn finds_only_missing_required_fields_in_stable_order() {
        let fields = vec![
            "状态".to_string(),
            "任务名称".to_string(),
            "备注".to_string(),
        ];
        assert_eq!(missing_required_fields(&fields), vec!["任务类型", "进度"]);
    }

    #[test]
    fn accepts_native_feishu_task_category_as_task_type_alias() {
        let fields = vec![
            "任务名称".to_string(),
            "任务类别".to_string(),
            "状态".to_string(),
            "进度".to_string(),
        ];
        assert!(missing_required_fields(&fields).is_empty());

        let records = vec![RecordItem {
            record_id: "rec-category-alias".to_string(),
            fields: serde_json::from_value(json!({
                "任务名称": "兼容飞书原生模板",
                "任务类别": { "name": "🔥 重要且紧急" },
                "状态": "进行中",
                "进度": 50
            }))
            .expect("fields"),
        }];
        let tasks = parse_tasks(&records, 1);
        assert_eq!(tasks[0].quadrant, "iu");
    }

    #[test]
    fn prefers_quadrant_task_label_over_general_task_category() {
        let records = vec![RecordItem {
            record_id: "rec-native-template".to_string(),
            fields: serde_json::from_value(json!({
                "任务名称": "飞书原生个人任务",
                "任务标签": "📌 重要不紧急",
                "任务类别": "工作",
                "状态": "进行中",
                "进度": 60
            }))
            .expect("fields"),
        }];
        let tasks = parse_tasks(&records, 1);
        assert_eq!(tasks[0].quadrant, "in");
    }

    #[test]
    fn required_field_specs_use_safe_types_and_expected_choices() {
        let task_type = required_field_spec("任务类型").expect("task type spec");
        assert_eq!(task_type["type"], 3);
        assert_eq!(
            task_type["property"]["options"].as_array().unwrap().len(),
            4
        );
        let status = required_field_spec("状态").expect("status spec");
        assert_eq!(status["property"]["options"][0]["name"], "未开始");
        let progress = required_field_spec("进度").expect("progress spec");
        assert_eq!(progress["type"], 2);
        assert_eq!(progress["property"]["formatter"], "0");
        assert!(required_field_spec("任务详情").is_none());
    }
}
