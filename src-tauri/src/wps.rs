use std::time::Duration;

use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::storage::Task;

const CREDENTIAL_SERVICE: &str = "app.snaptask.desktop";
const CREDENTIAL_USER: &str = "wps-airscript-token";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WpsConfig {
    pub webhook_url: String,
    pub sheet_name: String,
    pub token_configured: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveWpsConfigInput {
    pub webhook_url: String,
    pub sheet_name: String,
    pub token: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WpsSyncResult {
    pub task_count: usize,
    pub completed_count: usize,
    pub sheet_name: String,
    pub fields: Vec<String>,
    pub write_ready: bool,
    pub undo_ready: bool,
    pub script_version: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum WpsWriteRequest {
    Update {
        record_id: String,
        progress: u8,
        status: Option<String>,
        planned_date: Option<Option<String>>,
        restore: bool,
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

pub fn is_connection_error(error: &str) -> bool {
    error.starts_with("无法连接 WPS：")
}

#[derive(Debug, Deserialize)]
struct AirScriptEnvelope {
    status: Option<String>,
    error: Option<String>,
    error_details: Option<AirScriptErrorDetails>,
    data: Option<AirScriptData>,
}

#[derive(Debug, Deserialize)]
struct AirScriptErrorDetails {
    name: Option<String>,
    msg: Option<String>,
    stack: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct AirScriptData {
    result: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListPayload {
    ok: bool,
    sheet_name: String,
    fields: Vec<FieldPayload>,
    records: Vec<RecordPayload>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    script_version: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct FieldPayload {
    name: String,
}

#[derive(Debug, Deserialize)]
struct RecordPayload {
    id: String,
    fields: Value,
}

fn parse_envelope_response(response: reqwest::blocking::Response) -> Result<Value, String> {
    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("未提供 Content-Type")
        .to_string();
    let content_encoding = response
        .headers()
        .get(reqwest::header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("identity")
        .to_string();
    let final_host = response.url().host_str().unwrap_or("未知域名").to_string();
    let body = response
        .text()
        .map_err(|error| format!("无法读取 WPS 响应：{error}"))?;
    if !status.is_success() {
        return Err(format!(
            "WPS 返回 HTTP {}，请检查 Webhook 和令牌",
            status.as_u16()
        ));
    }
    let normalized_body = body.trim_start_matches('\u{feff}');
    let envelope: AirScriptEnvelope = serde_json::from_str(normalized_body).map_err(|error| {
        format!(
            "WPS 响应不是有效 JSON（HTTP {}，{}，编码 {}，{} 字节，{}，解析位置 {}:{}，最终域名 {}）",
            status.as_u16(),
            content_type,
            content_encoding,
            body.len(),
            non_json_kind(&body),
            error.line(),
            error.column(),
            final_host
        )
    })?;
    if envelope.status.as_deref() != Some("finished")
        || envelope
            .error
            .as_deref()
            .is_some_and(|value| !value.is_empty())
    {
        let mut parts = Vec::new();
        if let Some(error) = envelope.error.filter(|value| !value.is_empty()) {
            parts.push(error);
        }
        if let Some(details) = envelope.error_details {
            if let Some(name) = details.name.filter(|value| !value.is_empty()) {
                parts.push(name);
            }
            if let Some(message) = details.msg.filter(|value| !value.is_empty()) {
                parts.push(message);
            }
            if let Some(location) = details
                .stack
                .unwrap_or_default()
                .into_iter()
                .find(|line| line.contains(':'))
            {
                parts.push(format!("脚本位置：{location}"));
            }
        }
        parts.dedup();
        return Err(if parts.is_empty() {
            "WPS 脚本执行未完成".to_string()
        } else {
            parts.join("；")
        });
    }
    envelope
        .data
        .map(|data| data.result)
        .ok_or_else(|| "WPS 响应缺少执行结果".to_string())
}

fn post_action(config: &WpsConfig, token: &str, argv: Value) -> Result<Value, String> {
    let webhook_url = validate_webhook_url(&config.webhook_url)?;
    let response = Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("无法初始化 WPS 连接：{error}"))?
        .post(webhook_url)
        .header("AirScript-Token", token)
        .json(&json!({ "Context": { "argv": argv } }))
        .send()
        .map_err(|error| format!("无法连接 WPS：{error}"))?;
    parse_envelope_response(response)
}

fn parse_list_payload(result: &str) -> Result<ListPayload, String> {
    let trimmed = result.trim();
    if trimmed.is_empty()
        || matches!(
            trimmed,
            "undefined" | "null" | "\"\"" | "\"undefined\"" | "\"null\""
        )
    {
        return Err(
            "连接凭据有效，但 AirScript 脚本为空或没有返回任务数据；请粘贴并保存项目提供的 airscript-project-track.js"
                .to_string(),
        );
    }
    serde_json::from_str(trimmed).map_err(|_| {
        "连接凭据有效，但 AirScript 未返回项目任务数据；脚本可能为空、仍是默认示例或尚未保存，请粘贴并保存项目提供的 airscript-project-track.js"
            .to_string()
    })
}

fn parse_result_payload(result: Value) -> Result<ListPayload, String> {
    match result {
        Value::String(value) => parse_list_payload(&value),
        Value::Null => parse_list_payload("null"),
        value @ Value::Object(_) => serde_json::from_value(value).map_err(|_| {
            "连接凭据有效，但 AirScript 返回的任务对象格式不正确；请使用项目提供的最新版脚本"
                .to_string()
        }),
        _ => Err(
            "连接凭据有效，但 AirScript 返回值不是任务对象或 JSON 字符串；请使用项目提供的最新版脚本"
                .to_string(),
        ),
    }
}

fn non_json_kind(body: &str) -> &'static str {
    let trimmed = body.trim_start_matches('\u{feff}').trim_start();
    if trimmed.is_empty() {
        "空响应"
    } else if trimmed.starts_with('<') {
        "HTML 页面"
    } else {
        "文本响应"
    }
}

pub fn validate_webhook_url(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    let url = Url::parse(trimmed).map_err(|_| "Webhook 地址格式不正确".to_string())?;
    if url.scheme() != "https" {
        return Err("Webhook 必须使用 HTTPS".to_string());
    }
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    if host != "www.kdocs.cn" && host != "kdocs.cn" && !host.ends_with(".wps.cn") {
        return Err("Webhook 必须来自金山文档或 WPS 官方域名".to_string());
    }
    if !url.path().contains("/api/v3/ide/file/") || !url.path().ends_with("/sync_task") {
        return Err("请粘贴文档共享脚本的同步 Webhook 地址".to_string());
    }
    Ok(trimmed.to_string())
}

#[cfg(windows)]
pub fn save_token(token: &str) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("脚本令牌不能为空".to_string());
    }
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .and_then(|entry| entry.set_password(token))
        .map_err(|error| format!("无法保存到 Windows 凭据库：{error}"))
}

#[cfg(not(windows))]
pub fn save_token(_token: &str) -> Result<(), String> {
    Err("当前版本仅支持在 Windows 凭据库保存令牌".to_string())
}

#[cfg(windows)]
pub fn read_token() -> Result<String, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .and_then(|entry| entry.get_password())
        .map_err(|_| "尚未在 Windows 凭据库中保存 WPS 脚本令牌".to_string())
}

#[cfg(not(windows))]
pub fn read_token() -> Result<String, String> {
    Err("当前版本仅支持从 Windows 凭据库读取令牌".to_string())
}

fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) if value == "[object Object]" => String::new(),
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        Some(Value::Object(value)) => [
            "value", "text", "name", "label", "str", "Value", "Text", "Name", "Label",
        ]
        .iter()
        .filter_map(|key| value.get(*key))
        .map(|candidate| text(Some(candidate)))
        .find(|candidate| !candidate.is_empty())
        .unwrap_or_default(),
        Some(Value::Array(value)) => value
            .iter()
            .map(|item| text(Some(item)))
            .filter(|item| !item.is_empty())
            .collect::<Vec<_>>()
            .join("、"),
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
    match text(value).as_str() {
        "重要且紧急" => "iu",
        "紧急不重要" => "un",
        "重要不紧急" => "in",
        "不重要不紧急" => "nn",
        _ => "nn",
    }
    .to_string()
}

fn checkbox(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_i64().unwrap_or_default() != 0,
        Some(Value::String(value)) => matches!(value.as_str(), "1" | "true" | "是" | "已勾选"),
        _ => false,
    }
}

fn planned_date(fields: &serde_json::Map<String, Value>) -> Option<String> {
    [
        "任务计划完成日期",
        "任务计划完成时间",
        "计划完成日期",
        "截止日期",
    ]
    .into_iter()
    .map(|name| text(fields.get(name)))
    .find(|value| !value.trim().is_empty())
    .map(|value| value.trim().replace('/', "-"))
}

fn parse_tasks(payload: &ListPayload, now: i64) -> Vec<Task> {
    payload
        .records
        .iter()
        .filter_map(|record| {
            let fields = record.fields.as_object()?;
            let title = text(fields.get("任务名称"));
            if title.trim().is_empty() {
                return None;
            }
            Some(Task {
                id: record.id.clone(),
                title,
                detail: text(fields.get("任务详情")),
                progress: progress(fields.get("进度")),
                quadrant: quadrant(fields.get("任务类型")),
                status: text(fields.get("状态")),
                today: checkbox(fields.get("今日必完成")),
                planned_date: planned_date(fields),
                updated_at: now,
            })
        })
        .collect()
}

pub fn fetch_tasks(
    config: &WpsConfig,
    token: &str,
    now: i64,
) -> Result<(Vec<Task>, WpsSyncResult), String> {
    let result = post_action(
        config,
        token,
        json!({ "action": "list_tasks", "sheetName": config.sheet_name }),
    )?;
    let payload = parse_result_payload(result)?;
    if !payload.ok {
        return Err("WPS 脚本报告读取失败".to_string());
    }
    let field_names = payload
        .fields
        .iter()
        .map(|field| field.name.as_str())
        .collect::<Vec<_>>();
    let missing = ["任务名称", "任务类型", "状态", "进度"]
        .into_iter()
        .filter(|required| !field_names.contains(required))
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(format!(
            "目标数据表缺少必需字段：{}。未替换本地缓存",
            missing.join("、")
        ));
    }
    let tasks = parse_tasks(&payload, now);
    let sync = WpsSyncResult {
        task_count: tasks.len(),
        completed_count: tasks.iter().filter(|task| task.status == "已完成").count(),
        sheet_name: payload.sheet_name,
        fields: payload.fields.into_iter().map(|field| field.name).collect(),
        write_ready: payload.script_version.is_some_and(|version| version >= 17)
            && [
                "update_task",
                "create_temporary_task",
                "safe_optional_fields",
                "typed_choice_fields",
                "global_record_writes",
                "sparse_status_updates",
                "record_range_writes",
                "normalized_choice_reads",
                "isolated_choice_range_writes",
                "planned_date_writes",
                "primitive_choice_writes",
                "derived_status_writes",
            ]
            .iter()
            .all(|action| payload.capabilities.iter().any(|value| value == action)),
        undo_ready: payload.script_version.is_some_and(|version| version >= 17)
            && [
                "restore_task",
                "delete_task",
                "typed_choice_fields",
                "global_record_writes",
                "sparse_status_updates",
                "record_range_writes",
                "normalized_choice_reads",
                "blank_status_restore",
                "isolated_choice_range_writes",
                "planned_date_writes",
                "primitive_choice_writes",
                "derived_status_writes",
            ]
            .iter()
            .all(|action| payload.capabilities.iter().any(|value| value == action)),
        script_version: payload.script_version,
    };
    Ok((tasks, sync))
}

fn available_sheet_names(error: &str) -> Vec<String> {
    let Some((_, remainder)) = error.split_once("可用数据表：") else {
        return Vec::new();
    };
    remainder
        .split('；')
        .next()
        .unwrap_or_default()
        .split(['、', ',', '，'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect()
}

fn fetch_tasks_with_sheet_recovery<F>(
    config: &WpsConfig,
    mut fetch: F,
) -> Result<(Vec<Task>, WpsSyncResult), String>
where
    F: FnMut(&WpsConfig) -> Result<(Vec<Task>, WpsSyncResult), String>,
{
    let original_error = match fetch(config) {
        Ok(result) => return Ok(result),
        Err(error) => error,
    };
    let candidates = available_sheet_names(&original_error);
    if candidates.is_empty() {
        return Err(original_error);
    }

    let mut matches = Vec::new();
    for sheet_name in candidates {
        if sheet_name == config.sheet_name {
            continue;
        }
        let mut candidate = config.clone();
        candidate.sheet_name = sheet_name;
        if let Ok(result) = fetch(&candidate) {
            matches.push(result);
        }
    }

    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(original_error),
        _ => Err(format!(
            "检测到多个符合任务字段结构的数据表：{}；请在设置中选择需要使用的表",
            matches
                .iter()
                .map(|(_, sync)| sync.sheet_name.as_str())
                .collect::<Vec<_>>()
                .join("、")
        )),
    }
}

pub fn fetch_tasks_resolving_sheet(
    config: &WpsConfig,
    token: &str,
    now: i64,
) -> Result<(Vec<Task>, WpsSyncResult), String> {
    fetch_tasks_with_sheet_recovery(config, |candidate| fetch_tasks(candidate, token, now))
}

pub fn write_task(
    config: &WpsConfig,
    token: &str,
    request: &WpsWriteRequest,
) -> Result<(), String> {
    let argv = match request {
        WpsWriteRequest::Update {
            record_id,
            progress,
            status,
            planned_date,
            restore,
        } => json!({
            "action": if *restore { "restore_task" } else { "update_task" },
            "sheetName": config.sheet_name,
            "recordId": record_id,
            "progress": progress,
            "status": status,
            "changePlannedDate": planned_date.is_some(),
            "plannedDate": planned_date.as_ref().and_then(|value| value.as_deref())
        }),
        WpsWriteRequest::CreateTemporary {
            title,
            description,
            progress,
            quadrant,
            today,
            planned_date,
        } => json!({
            "action": "create_temporary_task",
            "sheetName": config.sheet_name,
            "title": title,
            "description": description,
            "progress": progress,
            "quadrant": quadrant,
            "today": today,
            "changePlannedDate": planned_date.is_some(),
            "plannedDate": planned_date
        }),
        WpsWriteRequest::Delete { record_id } => json!({
            "action": "delete_task",
            "sheetName": config.sheet_name,
            "recordId": record_id
        }),
    };
    let result = post_action(config, token, argv)?;
    let payload = match result {
        Value::String(value) => serde_json::from_str::<Value>(&value)
            .map_err(|_| "WPS 写入脚本返回格式不正确；请更新 AirScript 脚本".to_string())?,
        value => value,
    };
    if payload.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err("WPS 脚本没有确认写入成功；未标记为已同步".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_preflight_connection_failures_are_safe_to_queue() {
        assert!(is_connection_error("无法连接 WPS：operation timed out"));
        assert!(!is_connection_error("WPS 返回 HTTP 401，请检查令牌"));
        assert!(!is_connection_error("WPS 脚本执行未完成"));
    }

    #[test]
    fn only_accepts_official_sync_webhooks() {
        assert!(
            validate_webhook_url("https://www.kdocs.cn/api/v3/ide/file/f/script/s/sync_task")
                .is_ok()
        );
        assert!(
            validate_webhook_url("http://www.kdocs.cn/api/v3/ide/file/f/script/s/sync_task")
                .is_err()
        );
        assert!(
            validate_webhook_url("https://example.com/api/v3/ide/file/f/script/s/sync_task")
                .is_err()
        );
    }

    #[test]
    fn maps_wps_fields_without_guessing_missing_values() {
        let payload = ListPayload {
            ok: true,
            sheet_name: "任务清单".to_string(),
            fields: vec![],
            records: vec![RecordPayload {
                id: "rec-A".to_string(),
                fields: json!({
                    "任务名称": "E技术报告",
                    "任务类型": {"value": "重要不紧急"},
                    "状态": {"value": "进行中"},
                    "进度": 0.4,
                    "任务详情": "输出需求文档",
                    "今日必完成": true,
                    "任务计划完成日期": "2026/10/20"
                }),
            }],
            capabilities: vec![],
            script_version: None,
        };
        let tasks = parse_tasks(&payload, 42);
        assert_eq!(tasks[0].progress, 40);
        assert_eq!(tasks[0].quadrant, "in");
        assert!(tasks[0].today);
        assert_eq!(tasks[0].planned_date.as_deref(), Some("2026-10-20"));
    }

    #[test]
    fn unwraps_nested_runtime_choice_values() {
        assert_eq!(
            text(Some(&json!({"value": {"Text": "进行中", "id": "C"}}))),
            "进行中"
        );
        assert_eq!(text(Some(&json!("[object Object]"))), "");
    }

    #[test]
    fn empty_and_fractional_progress_are_normalized() {
        assert_eq!(progress(None), 0);
        assert_eq!(progress(Some(&json!(0.57))), 57);
        assert_eq!(progress(Some(&json!(88))), 88);
        assert_eq!(progress(Some(&json!("100%"))), 100);
    }

    #[test]
    fn empty_script_result_is_reported_after_credentials_succeed() {
        for result in ["", "undefined", "null", "\"\""] {
            let error = parse_list_payload(result).unwrap_err();
            assert!(error.contains("连接凭据有效"));
            assert!(error.contains("脚本为空"));
            assert!(!error.contains("令牌"));
        }
    }

    #[test]
    fn malformed_script_result_is_distinguished_from_authentication_failure() {
        let error = parse_list_payload("not-json").unwrap_err();
        assert!(error.contains("连接凭据有效"));
        assert!(error.contains("未返回项目任务数据"));
        assert!(error.contains("默认示例"));
    }

    #[test]
    fn non_json_responses_are_classified_without_exposing_the_body() {
        assert_eq!(non_json_kind(""), "空响应");
        assert_eq!(non_json_kind("\u{feff}  <html>error</html>"), "HTML 页面");
        assert_eq!(non_json_kind("gateway failure"), "文本响应");
    }

    #[test]
    fn accepts_wps_result_as_object_or_json_string() {
        let object = json!({
            "ok": true,
            "sheetName": "任务清单",
            "fields": [],
            "records": []
        });
        let from_object = parse_result_payload(object.clone()).unwrap();
        let from_string = parse_result_payload(Value::String(object.to_string())).unwrap();
        assert_eq!(from_object.sheet_name, "任务清单");
        assert_eq!(from_string.sheet_name, "任务清单");
    }

    #[test]
    fn recovers_when_saved_sheet_was_renamed() {
        let config = WpsConfig {
            webhook_url: "https://www.kdocs.cn/api/v3/ide/file/f/script/s/sync_task".to_string(),
            sheet_name: "任务清单".to_string(),
            token_configured: true,
        };
        let mut attempted = Vec::new();
        let (_, sync) = fetch_tasks_with_sheet_recovery(&config, |candidate| {
            attempted.push(candidate.sheet_name.clone());
            match candidate.sheet_name.as_str() {
                "任务清单" => Err(
                    "找不到数据表：任务清单；可用数据表：仪表盘、任务列表、模板使用说明、智能文档；Error"
                        .to_string(),
                ),
                "任务列表" => Ok((
                    Vec::new(),
                    WpsSyncResult {
                        task_count: 0,
                        completed_count: 0,
                        sheet_name: "任务列表".to_string(),
                        fields: vec!["任务名称".to_string(), "任务类型".to_string(), "状态".to_string(), "进度".to_string()],
                        write_ready: true,
                        undo_ready: true,
                        script_version: Some(11),
                    },
                )),
                _ => Err("目标数据表缺少必需字段".to_string()),
            }
        })
        .unwrap();

        assert_eq!(sync.sheet_name, "任务列表");
        assert_eq!(
            attempted,
            vec!["任务清单", "仪表盘", "任务列表", "模板使用说明", "智能文档"]
        );
    }
}
