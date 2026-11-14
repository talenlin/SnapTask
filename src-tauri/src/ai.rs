use std::time::Duration;

use reqwest::{blocking::Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    retrieval::HabitContext,
    storage::{AnalysisResult, Task},
};

const CREDENTIAL_SERVICE: &str = "app.snaptask.desktop";
const CREDENTIAL_USER: &str = "model-api-key";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub enabled: bool,
    pub api_key_configured: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveAiConfigInput {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub enabled: bool,
    pub api_key: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListModelsInput {
    pub provider: String,
    pub endpoint: String,
    pub api_key: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelListResult {
    pub models: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelDescriptor>,
}

#[derive(Debug, Deserialize)]
struct ModelDescriptor {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelDecision {
    kind: String,
    task_id: Option<String>,
    candidate_ids: Option<Vec<String>>,
    suggested_title: Option<String>,
    suggested_detail: Option<String>,
    confidence: Option<f64>,
}

pub fn provider_endpoint(provider: &str, custom_endpoint: &str) -> Result<String, String> {
    let endpoint = match provider {
        "deepseek" => "https://api.deepseek.com",
        "mimo" => "https://api.xiaomimimo.com/v1",
        "qwen" => "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "kimi" => "https://api.moonshot.cn/v1",
        "glm" => "https://open.bigmodel.cn/api/paas/v4",
        "gemini" => "https://generativelanguage.googleapis.com/v1beta/openai",
        "openai" => "https://api.openai.com/v1",
        "grok" => "https://api.x.ai/v1",
        "generic" => custom_endpoint,
        _ => return Err("不支持的模型提供商".to_string()),
    };
    normalize_endpoint(endpoint)
}

pub fn normalize_endpoint(value: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    let mut url = Url::parse(trimmed).map_err(|_| "模型接口地址格式不正确".to_string())?;
    let local_http =
        url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.scheme() != "https" && !local_http {
        return Err("模型接口必须使用 HTTPS；本机 localhost 接口可使用 HTTP".to_string());
    }
    if !url
        .path()
        .trim_end_matches('/')
        .ends_with("/chat/completions")
    {
        let path = url.path().trim_end_matches('/');
        let next = if path.is_empty() || path == "/" {
            "/chat/completions".to_string()
        } else {
            format!("{path}/chat/completions")
        };
        url.set_path(&next);
    }
    Ok(url.to_string())
}

fn models_endpoint(chat_endpoint: &str) -> Result<String, String> {
    let mut url = Url::parse(chat_endpoint).map_err(|_| "模型接口地址格式不正确".to_string())?;
    let path = url.path().trim_end_matches('/');
    let base = path
        .strip_suffix("/chat/completions")
        .ok_or_else(|| "模型接口缺少 chat/completions 路径".to_string())?;
    url.set_path(&format!("{base}/models"));
    Ok(url.to_string())
}

fn model_matches_provider(provider: &str, id: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    match provider {
        "deepseek" => lower.starts_with("deepseek"),
        "mimo" => lower.contains("mimo") && !lower.contains("tts") && !lower.contains("asr"),
        "qwen" => lower.starts_with("qwen"),
        "kimi" => lower.contains("kimi") || lower.contains("moonshot"),
        "glm" => lower.starts_with("glm"),
        "gemini" => lower.starts_with("gemini"),
        "openai" => {
            (lower.starts_with("gpt-") || lower.starts_with('o'))
                && ![
                    "audio",
                    "realtime",
                    "transcribe",
                    "tts",
                    "image",
                    "search",
                    "moderation",
                    "embedding",
                ]
                .iter()
                .any(|term| lower.contains(term))
        }
        "grok" => lower.starts_with("grok"),
        "generic" => true,
        _ => false,
    }
}

pub fn list_models(input: &ListModelsInput) -> Result<ModelListResult, String> {
    let chat_endpoint = provider_endpoint(&input.provider, &input.endpoint)?;
    let api_key = match input
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(value) => value.to_string(),
        None => read_api_key()?,
    };
    let mut request = Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| format!("无法初始化模型连接：{error}"))?
        .get(models_endpoint(&chat_endpoint)?)
        .bearer_auth(api_key);
    if input.provider == "gemini" {
        request = request.header("x-goog-api-client", "project-track/0.1.14");
    }
    let response = request
        .send()
        .map_err(|error| format!("无法读取模型列表：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "读取模型失败（HTTP {}），请检查提供商、接口地址和 API Key",
            status.as_u16()
        ));
    }
    let body: ModelsResponse = response
        .json()
        .map_err(|_| "提供商未返回 OpenAI 兼容的模型列表".to_string())?;
    let mut models = body
        .data
        .into_iter()
        .map(|model| model.id)
        .filter(|id| model_matches_provider(&input.provider, id))
        .collect::<Vec<_>>();
    models.sort_by_key(|left| left.to_ascii_lowercase());
    models.dedup();
    if models.is_empty() {
        return Err("该 Key 没有返回可用的对话模型".to_string());
    }
    Ok(ModelListResult { models })
}

#[cfg(windows)]
pub fn save_api_key(api_key: &str) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("模型 API Key 不能为空".to_string());
    }
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .and_then(|entry| entry.set_password(api_key))
        .map_err(|error| format!("无法保存模型密钥到 Windows 凭据库：{error}"))
}

#[cfg(not(windows))]
pub fn save_api_key(_api_key: &str) -> Result<(), String> {
    Err("当前版本仅支持在 Windows 凭据库保存模型密钥".to_string())
}

#[cfg(windows)]
pub fn read_api_key() -> Result<String, String> {
    keyring::Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER)
        .and_then(|entry| entry.get_password())
        .map_err(|_| "尚未在 Windows 凭据库中保存模型 API Key".to_string())
}

#[cfg(not(windows))]
pub fn read_api_key() -> Result<String, String> {
    Err("当前版本仅支持从 Windows 凭据库读取模型密钥".to_string())
}

fn request_content(config: &AiConfig, api_key: &str, messages: Value) -> Result<String, String> {
    let endpoint = normalize_endpoint(&config.endpoint)?;
    if config.model.trim().is_empty() {
        return Err("请填写模型名称".to_string());
    }
    let response = Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| format!("无法初始化模型连接：{error}"))?
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&json!({
            "model": config.model.trim(),
            "messages": messages
        }))
        .send()
        .map_err(|error| format!("无法连接模型接口：{error}"))?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "模型接口返回 HTTP {}，请检查地址、模型名称和 API Key",
            status.as_u16()
        ));
    }
    let body = response
        .text()
        .map_err(|error| format!("无法读取模型响应：{error}"))?;
    parse_chat_content(&body)
}

fn response_text(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Array(items) => items
            .iter()
            .map(response_text)
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join(""),
        Value::Object(object) => ["text", "content", "output_text"]
            .iter()
            .filter_map(|key| object.get(*key))
            .map(response_text)
            .find(|part| !part.trim().is_empty())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn safe_provider_message(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn parse_chat_content(body: &str) -> Result<String, String> {
    let value: Value = serde_json::from_str(body)
        .map_err(|_| "模型接口返回的不是有效 JSON，请稍后重试或更换模型".to_string())?;
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
    {
        return Err(format!(
            "模型提供商返回错误：{}",
            safe_provider_message(message)
        ));
    }
    let message = value
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .and_then(|choice| choice.get("message"));
    if let Some(message) = message {
        let content = response_text(message.get("content").unwrap_or(&Value::Null));
        if !content.trim().is_empty() {
            return Ok(content);
        }
        let reasoning = response_text(message.get("reasoning_content").unwrap_or(&Value::Null));
        if !reasoning.trim().is_empty() {
            return Err("模型只返回了推理过程，没有最终回答；请重试或换用普通对话模型".to_string());
        }
    }
    let output_text = response_text(value.get("output_text").unwrap_or(&Value::Null));
    if !output_text.trim().is_empty() {
        return Ok(output_text);
    }
    let output = response_text(value.get("output").unwrap_or(&Value::Null));
    if !output.trim().is_empty() {
        return Ok(output);
    }
    Err("模型已响应，但没有返回可用的文字内容；请重试或换用普通对话模型".to_string())
}

fn extract_json(content: &str) -> Result<&str, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "模型没有返回 JSON 对象".to_string())?;
    let end = content
        .rfind('}')
        .ok_or_else(|| "模型返回的 JSON 不完整".to_string())?;
    if end < start {
        return Err("模型返回的 JSON 不完整".to_string());
    }
    Ok(&content[start..=end])
}

fn confidence_percent(value: Option<f64>) -> u8 {
    let value = value.unwrap_or_default();
    let normalized = if value <= 1.0 { value * 100.0 } else { value };
    normalized.round().clamp(0.0, 100.0) as u8
}

fn map_decision(
    content: &str,
    description: &str,
    progress: u8,
    tasks: &[Task],
) -> Result<AnalysisResult, String> {
    let decision: ModelDecision = serde_json::from_str(extract_json(content)?)
        .map_err(|_| "模型返回的判断 JSON 无法解析".to_string())?;
    let confidence = confidence_percent(decision.confidence);
    match decision.kind.as_str() {
        "match" => {
            let task_id = decision
                .task_id
                .ok_or_else(|| "模型未返回 taskId".to_string())?;
            let task = tasks
                .iter()
                .find(|task| task.id == task_id)
                .cloned()
                .ok_or_else(|| "模型返回了不存在的任务 ID".to_string())?;
            Ok(AnalysisResult::Match {
                task,
                next_progress: progress,
                analysis_source: "ai".to_string(),
                confidence: Some(confidence),
            })
        }
        "ambiguous" => {
            let ids = decision.candidate_ids.unwrap_or_default();
            let candidates = ids
                .iter()
                .filter_map(|id| tasks.iter().find(|task| task.id == *id).cloned())
                .take(3)
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err("模型未返回有效候选任务".to_string());
            }
            Ok(AnalysisResult::Ambiguous {
                candidates,
                next_progress: progress,
                analysis_source: "ai".to_string(),
                confidence: Some(confidence),
            })
        }
        "temporary" => Ok(AnalysisResult::Temporary {
            suggested_title: decision
                .suggested_title
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| description.chars().take(24).collect()),
            suggested_detail: decision
                .suggested_detail
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| description.chars().take(120).collect()),
            suggested_quadrant: None,
            suggested_phrase: None,
            next_progress: progress,
            analysis_source: "ai".to_string(),
            confidence: Some(confidence),
        }),
        _ => Err("模型返回了不支持的判断类型".to_string()),
    }
}

pub fn test_connection(config: &AiConfig, api_key: &str) -> Result<(), String> {
    let content = request_content(
        config,
        api_key,
        json!([
            {"role": "system", "content": "只回答 OK。"},
            {"role": "user", "content": "连接测试"}
        ]),
    )?;
    if content.trim().is_empty() {
        Err("模型连接成功，但没有返回内容".to_string())
    } else {
        Ok(())
    }
}

pub fn analyze(
    config: &AiConfig,
    api_key: &str,
    description: &str,
    progress: u8,
    tasks: &[Task],
    habits: &[HabitContext],
    intent: &str,
) -> Result<AnalysisResult, String> {
    let task_data = tasks
        .iter()
        .map(|task| {
            json!({
                "id": task.id,
                "title": task.title,
                "detail": task.detail,
                "progress": task.progress,
                "quadrant": task.quadrant
            })
        })
        .collect::<Vec<_>>();
    let content = request_content(
        config,
        api_key,
        json!([
            {
                "role": "system",
                "content": "你是个人任务整理器。只根据给定的候选任务和用户习惯判断，不执行任何写入。intent=create 时必须返回 temporary，不能匹配已有任务；intent=update 时只能返回 match 或 ambiguous，不能建议新增。习惯是用户确认过的表达线索，应优先参考，但只能返回候选任务中存在的 ID。只返回单个 JSON：kind 为 match、ambiguous 或 temporary；match 必须返回 taskId；ambiguous 返回最多 3 个 candidateIds；temporary 必须返回 suggestedTitle 和 suggestedDetail；confidence 返回 0 到 1。suggestedTitle 要概括为可复用的固定任务类型，优先使用 2–10 个汉字的“对象+动作”，删除“新建/完成”等指令、业务编号、日期、完成状态、人员和递交渠道，例如“完成2026-ABC1234资料核对并提交项目组”应命名为“资料核对”。suggestedDetail 用一句自然、简洁的话保留工作范围或交付物，不复述任务名称，不包含已经单独保存的计划完成日期。不得编造任务 ID。"
            },
            {
                "role": "user",
                "content": serde_json::to_string(&json!({
                    "description": description,
                    "intent": intent,
                    "authoritativeProgress": progress,
                    "tasks": task_data,
                    "habits": habits
                })).unwrap_or_default()
            }
        ]),
    )?;
    map_decision(&content, description, progress, tasks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str) -> Task {
        Task {
            id: id.to_string(),
            title: "E技术报告".to_string(),
            detail: String::new(),
            progress: 40,
            quadrant: "in".to_string(),
            status: "进行中".to_string(),
            today: false,
            planned_date: None,
            updated_at: 0,
        }
    }

    #[test]
    fn accepts_base_or_full_chat_endpoint() {
        assert_eq!(
            normalize_endpoint("https://api.example.com/v1").unwrap(),
            "https://api.example.com/v1/chat/completions"
        );
        assert_eq!(
            normalize_endpoint("http://127.0.0.1:11434/v1/chat/completions").unwrap(),
            "http://127.0.0.1:11434/v1/chat/completions"
        );
        assert!(normalize_endpoint("http://api.example.com/v1").is_err());
    }

    #[test]
    fn built_in_providers_use_expected_openai_compatible_paths() {
        let expected = [
            ("deepseek", "https://api.deepseek.com/chat/completions"),
            ("mimo", "https://api.xiaomimimo.com/v1/chat/completions"),
            (
                "qwen",
                "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
            ),
            ("kimi", "https://api.moonshot.cn/v1/chat/completions"),
            (
                "glm",
                "https://open.bigmodel.cn/api/paas/v4/chat/completions",
            ),
            (
                "gemini",
                "https://generativelanguage.googleapis.com/v1beta/openai/chat/completions",
            ),
            ("openai", "https://api.openai.com/v1/chat/completions"),
            ("grok", "https://api.x.ai/v1/chat/completions"),
        ];
        for (provider, endpoint) in expected {
            assert_eq!(provider_endpoint(provider, "").unwrap(), endpoint);
        }
    }

    #[test]
    fn filters_non_chat_models_from_provider_lists() {
        assert!(model_matches_provider("mimo", "mimo-v2.5-pro"));
        assert!(!model_matches_provider("mimo", "mimo-v2.5-tts"));
        assert!(model_matches_provider("openai", "gpt-5.6-luna"));
        assert!(!model_matches_provider("openai", "gpt-image-2"));
        assert!(!model_matches_provider("openai", "omni-moderation-latest"));
        assert!(model_matches_provider("gemini", "gemini-3.8-flash"));
    }

    #[test]
    fn maps_only_existing_task_ids() {
        let tasks = vec![task("rec-1")];
        let result = map_decision(
            "```json\n{\"kind\":\"match\",\"taskId\":\"rec-1\",\"confidence\":0.91}\n```",
            "完成 E技术 报告",
            65,
            &tasks,
        )
        .unwrap();
        match result {
            AnalysisResult::Match {
                task, confidence, ..
            } => {
                assert_eq!(task.id, "rec-1");
                assert_eq!(confidence, Some(91));
            }
            _ => panic!("expected match"),
        }
        assert!(map_decision(
            "{\"kind\":\"match\",\"taskId\":\"invented\",\"confidence\":1}",
            "test",
            10,
            &tasks
        )
        .is_err());
    }

    #[test]
    fn accepts_openai_compatible_content_variants() {
        assert_eq!(
            parse_chat_content(
                r#"{"choices":[{"message":{"content":"{\"kind\":\"temporary\"}"}}]}"#
            )
            .unwrap(),
            r#"{"kind":"temporary"}"#
        );
        assert_eq!(
            parse_chat_content(
                r#"{"choices":[{"message":{"content":[{"type":"text","text":"{\"kind\":\"temporary\"}"}]}}]}"#
            )
            .unwrap(),
            r#"{"kind":"temporary"}"#
        );
        let reasoning_only = parse_chat_content(
            r#"{"choices":[{"message":{"content":null,"reasoning_content":"{\"kind\":\"temporary\"}"}}]}"#,
        )
        .unwrap_err();
        assert!(reasoning_only.contains("没有最终回答"));
    }

    #[test]
    fn reports_http_200_provider_errors_without_exposing_response_body() {
        let error = parse_chat_content(
            r#"{"error":{"message":"model is not available","type":"invalid_request_error"}}"#,
        )
        .unwrap_err();
        assert!(error.contains("模型提供商返回错误"));
        assert!(error.contains("model is not available"));
        assert!(!error.contains("invalid_request_error"));
    }
}
