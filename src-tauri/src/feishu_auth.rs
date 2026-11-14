use std::time::Duration;

use reqwest::{blocking::Client, Url};
use serde::Serialize;
use serde_json::Value;

use crate::{feishu, storage::Storage};

const FEISHU_ACCOUNTS_ROOT: &str = "https://accounts.feishu.cn";
const FEISHU_OPEN_ROOT: &str = "https://open.feishu.cn";
const APP_REGISTRATION_PATH: &str = "/oauth/v1/app/registration";
const DEVICE_AUTHORIZATION_PATH: &str = "/oauth/v1/device_authorization";
const TOKEN_PATH: &str = "/open-apis/authen/v2/oauth/token";
const REQUIRED_USER_SCOPES: &str = "offline_access base:app:copy base:table:read base:field:read base:view:read base:dashboard:read base:workflow:read base:record:retrieve base:record:create base:record:update base:record:delete";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeishuAuthProgress {
    pub state: String,
    pub message: String,
    pub verification_url: Option<String>,
    pub user_code: Option<String>,
    pub retry_after_seconds: u64,
    pub done: bool,
}

enum PendingStep {
    Idle,
    AppRegistration {
        device_code: String,
        user_code: String,
        verification_url: String,
        expires_at: i64,
        interval_seconds: u64,
        next_poll_at: i64,
    },
    UserAuthorization {
        app_id: String,
        device_code: String,
        user_code: String,
        verification_url: String,
        expires_at: i64,
        interval_seconds: u64,
        next_poll_at: i64,
    },
    SaveUserSession {
        app_id: String,
        tokens: UserTokens,
    },
}

pub struct FeishuAuthFlow {
    step: PendingStep,
}

enum PollOutcome<T> {
    Pending { slow_down: bool },
    Denied,
    Expired,
    Complete(T),
}

struct DeviceCode {
    device_code: String,
    user_code: String,
    verification_url: String,
    expires_in: u64,
    interval_seconds: u64,
}

struct RegisteredApp {
    app_id: String,
    app_secret: String,
}

struct UserTokens {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    refresh_expires_in: Option<u64>,
}

impl FeishuAuthFlow {
    // Reauthorize an existing application only; never fall back to registration.
    pub fn start_existing(app_id: &str) -> Result<(Self, FeishuAuthProgress), String> {
        let secret = feishu::read_oauth_app_secret(app_id)
            .map_err(|_| "隔离连接应用凭据不可用；未创建新应用，也未改用正式连接".to_string())?;
        let device = begin_user_authorization(app_id, &secret)?;
        let progress = authorization_progress(&device, "请使用此应用所属的飞书身份确认授权；完成后仅保存隔离会话，不执行同步。");
        Ok((Self::from_authorization(app_id.into(), device), progress))
    }

    pub fn poll_existing(&mut self, expected_app: &str) -> Result<FeishuAuthProgress, String> {
        match &self.step {
            PendingStep::UserAuthorization { app_id, .. } | PendingStep::SaveUserSession { app_id, .. }
                if app_id == expected_app => {}
            _ => return Err("隔离授权流程与指定应用不匹配，已停止".into()),
        }
        // Existing-user branches do not write settings. The legacy flow requires
        // Storage, so provide memory-only storage, never a formal/profile database.
        let storage = Storage::open(std::path::PathBuf::from(":memory:"))
            .map_err(|_| "无法初始化临时授权状态".to_string())?;
        self.poll(&storage)
    }

    pub fn start(storage: &Storage) -> Result<(Self, FeishuAuthProgress), String> {
        if let Some((app_id, app_secret)) = saved_app_credentials(storage)? {
            let device = begin_user_authorization(&app_id, &app_secret)?;
            let progress = authorization_progress(&device, "请在飞书页面授权任务读取与同步权限。");
            return Ok((Self::from_authorization(app_id, device), progress));
        }

        let device = begin_app_registration()?;
        let progress = registration_progress(&device);
        Ok((Self::from_registration(device), progress))
    }

    pub fn poll(&mut self, storage: &Storage) -> Result<FeishuAuthProgress, String> {
        let step = std::mem::replace(&mut self.step, PendingStep::Idle);
        match step {
            PendingStep::Idle => Ok(done_progress("idle", "当前没有进行中的飞书授权。")),
            PendingStep::SaveUserSession { app_id, tokens } => {
                let save_result =
                    feishu::save_user_session_v2(&feishu::SaveFeishuUserSessionInput {
                        app_id: Some(app_id.clone()),
                        access_token: tokens.access_token.clone(),
                        refresh_token: tokens.refresh_token.clone(),
                        expires_in: tokens.expires_in,
                        refresh_expires_in: tokens.refresh_expires_in,
                    });
                match save_result {
                    Ok(()) => Ok(done_progress(
                        "authorized",
                        "飞书账号已连接，访问凭据已保存到 Windows 凭据库。",
                    )),
                    Err(error) => {
                        self.step = PendingStep::SaveUserSession { app_id, tokens };
                        Ok(FeishuAuthProgress {
                            state: "saving".to_string(),
                            message: format!(
                                "飞书授权已完成，但保存失败：{error}。应用会自动重试。"
                            ),
                            verification_url: None,
                            user_code: None,
                            retry_after_seconds: 5,
                            done: false,
                        })
                    }
                }
            }
            PendingStep::AppRegistration {
                device_code,
                user_code,
                verification_url,
                expires_at,
                interval_seconds,
                next_poll_at,
            } => {
                let now = crate::storage::unix_now();
                if now >= expires_at {
                    return Ok(done_progress(
                        "expired",
                        "飞书应用授权请求已过期，请重新开始。",
                    ));
                }
                if now < next_poll_at {
                    let retry_after = next_poll_at.saturating_sub(now).max(1) as u64;
                    self.step = PendingStep::AppRegistration {
                        device_code,
                        user_code: user_code.clone(),
                        verification_url: verification_url.clone(),
                        expires_at,
                        interval_seconds,
                        next_poll_at,
                    };
                    return Ok(FeishuAuthProgress {
                        state: "registering".to_string(),
                        message: "等待你在飞书确认创建本机连接应用。".to_string(),
                        verification_url: Some(verification_url),
                        user_code: Some(user_code),
                        retry_after_seconds: retry_after,
                        done: false,
                    });
                }
                match poll_app_registration(&device_code) {
                    Ok(PollOutcome::Pending { slow_down }) => {
                        let interval_seconds = if slow_down {
                            interval_seconds.saturating_add(5).min(60)
                        } else {
                            interval_seconds
                        };
                        self.step = PendingStep::AppRegistration {
                            device_code,
                            user_code: user_code.clone(),
                            verification_url: verification_url.clone(),
                            expires_at,
                            interval_seconds,
                            next_poll_at: now.saturating_add(interval_seconds as i64),
                        };
                        Ok(FeishuAuthProgress {
                            state: "registering".to_string(),
                            message: "等待你在飞书确认创建本机连接应用。".to_string(),
                            verification_url: Some(verification_url),
                            user_code: Some(user_code),
                            retry_after_seconds: interval_seconds,
                            done: false,
                        })
                    }
                    Ok(PollOutcome::Denied) => Ok(done_progress(
                        "denied",
                        "你已取消飞书应用创建；本机没有保存新凭据。",
                    )),
                    Ok(PollOutcome::Expired) => Ok(done_progress(
                        "expired",
                        "飞书应用授权请求已过期，请重新开始。",
                    )),
                    Ok(PollOutcome::Complete(app)) => {
                        if !app.app_id.starts_with("cli_") || app.app_secret.trim().is_empty() {
                            return Err("飞书未返回可用的应用凭据，请重新发起授权。".to_string());
                        }
                        storage.set_setting("feishu.auth_app_id", &app.app_id)?;
                        storage.set_setting("feishu.app_id", &app.app_id)?;
                        feishu::save_oauth_app_credentials(&app.app_id, &app.app_secret)?;
                        let device = begin_user_authorization(&app.app_id, &app.app_secret)?;
                        let progress = authorization_progress(
                            &device,
                            "连接应用已安全保存。接下来请在飞书页面确认多维表格权限。",
                        );
                        self.step = Self::authorization_step(app.app_id, device);
                        Ok(progress)
                    }
                    Err(_) => {
                        let retry_interval = interval_seconds.max(5);
                        self.step = PendingStep::AppRegistration {
                            device_code,
                            user_code: user_code.clone(),
                            verification_url: verification_url.clone(),
                            expires_at,
                            interval_seconds: retry_interval,
                            next_poll_at: now.saturating_add(retry_interval as i64),
                        };
                        Ok(FeishuAuthProgress {
                            state: "registering".to_string(),
                            message: "飞书暂时没有响应；授权请求仍有效，稍后会继续检查。"
                                .to_string(),
                            verification_url: Some(verification_url),
                            user_code: Some(user_code),
                            retry_after_seconds: retry_interval,
                            done: false,
                        })
                    }
                }
            }
            PendingStep::UserAuthorization {
                app_id,
                device_code,
                user_code,
                verification_url,
                expires_at,
                interval_seconds,
                next_poll_at,
            } => {
                let now = crate::storage::unix_now();
                if now >= expires_at {
                    return Ok(done_progress(
                        "expired",
                        "飞书账号授权请求已过期，请重新开始。",
                    ));
                }
                if now < next_poll_at {
                    let retry_after = next_poll_at.saturating_sub(now).max(1) as u64;
                    self.step = PendingStep::UserAuthorization {
                        app_id,
                        device_code,
                        user_code: user_code.clone(),
                        verification_url: verification_url.clone(),
                        expires_at,
                        interval_seconds,
                        next_poll_at,
                    };
                    return Ok(FeishuAuthProgress {
                        state: "authorizing".to_string(),
                        message: "请在飞书页面确认多维表格权限。".to_string(),
                        verification_url: Some(verification_url),
                        user_code: Some(user_code),
                        retry_after_seconds: retry_after,
                        done: false,
                    });
                }
                let app_secret = feishu::read_oauth_app_secret(&app_id)?;
                match poll_user_authorization(&app_id, &app_secret, &device_code) {
                    Ok(PollOutcome::Pending { slow_down }) => {
                        let interval_seconds = if slow_down {
                            interval_seconds.saturating_add(5).min(60)
                        } else {
                            interval_seconds
                        };
                        self.step = PendingStep::UserAuthorization {
                            app_id,
                            device_code,
                            user_code: user_code.clone(),
                            verification_url: verification_url.clone(),
                            expires_at,
                            interval_seconds,
                            next_poll_at: now.saturating_add(interval_seconds as i64),
                        };
                        Ok(FeishuAuthProgress {
                            state: "authorizing".to_string(),
                            message: "请在飞书页面确认多维表格权限。".to_string(),
                            verification_url: Some(verification_url),
                            user_code: Some(user_code),
                            retry_after_seconds: interval_seconds,
                            done: false,
                        })
                    }
                    Ok(PollOutcome::Denied) => Ok(done_progress(
                        "denied",
                        "你已拒绝飞书权限请求；本机没有保存用户访问令牌。",
                    )),
                    Ok(PollOutcome::Expired) => Ok(done_progress(
                        "expired",
                        "飞书账号授权请求已过期，请重新开始。",
                    )),
                    Ok(PollOutcome::Complete(tokens)) => {
                        self.step = PendingStep::SaveUserSession { app_id, tokens };
                        self.poll(storage)
                    }
                    Err(_) => {
                        let retry_interval = interval_seconds.max(5);
                        self.step = PendingStep::UserAuthorization {
                            app_id,
                            device_code,
                            user_code: user_code.clone(),
                            verification_url: verification_url.clone(),
                            expires_at,
                            interval_seconds: retry_interval,
                            next_poll_at: now.saturating_add(retry_interval as i64),
                        };
                        Ok(FeishuAuthProgress {
                            state: "authorizing".to_string(),
                            message: "飞书暂时没有响应；授权请求仍有效，稍后会继续检查。"
                                .to_string(),
                            verification_url: Some(verification_url),
                            user_code: Some(user_code),
                            retry_after_seconds: retry_interval,
                            done: false,
                        })
                    }
                }
            }
        }
    }

    fn from_registration(device: DeviceCode) -> Self {
        Self {
            step: PendingStep::AppRegistration {
                device_code: device.device_code,
                user_code: device.user_code,
                verification_url: device.verification_url,
                expires_at: crate::storage::unix_now()
                    .saturating_add(device.expires_in.min(i64::MAX as u64) as i64),
                interval_seconds: device.interval_seconds.max(1),
                next_poll_at: crate::storage::unix_now()
                    .saturating_add(device.interval_seconds.max(1) as i64),
            },
        }
    }

    fn from_authorization(app_id: String, device: DeviceCode) -> Self {
        Self {
            step: PendingStep::UserAuthorization {
                app_id,
                device_code: device.device_code,
                user_code: device.user_code,
                verification_url: device.verification_url,
                expires_at: crate::storage::unix_now()
                    .saturating_add(device.expires_in.min(i64::MAX as u64) as i64),
                interval_seconds: device.interval_seconds.max(1),
                next_poll_at: crate::storage::unix_now()
                    .saturating_add(device.interval_seconds.max(1) as i64),
            },
        }
    }

    fn authorization_step(app_id: String, device: DeviceCode) -> PendingStep {
        let now = crate::storage::unix_now();
        let interval_seconds = device.interval_seconds.max(1);
        PendingStep::UserAuthorization {
            app_id,
            device_code: device.device_code,
            user_code: device.user_code,
            verification_url: device.verification_url,
            expires_at: now.saturating_add(device.expires_in.min(i64::MAX as u64) as i64),
            interval_seconds,
            next_poll_at: now.saturating_add(interval_seconds as i64),
        }
    }
}

fn saved_app_credentials(storage: &Storage) -> Result<Option<(String, String)>, String> {
    if let Some(app_id) = storage.get_setting("feishu.auth_app_id")? {
        let secret = feishu::read_oauth_app_secret(&app_id).map_err(|_| {
            "检测到已创建的飞书连接应用，但本机凭据暂时不可用。为避免重复创建，请检查 Windows 凭据库后重试。".to_string()
        })?;
        return Ok(Some((app_id, secret)));
    }
    let Some(app_id) = storage
        .get_setting("feishu.app_id")?
        .filter(|value| value.starts_with("cli_"))
    else {
        return Ok(None);
    };
    let Ok(secret) = feishu::read_secret() else {
        return Ok(None);
    };
    feishu::save_oauth_app_credentials(&app_id, &secret)?;
    Ok(Some((app_id, secret)))
}

fn begin_app_registration() -> Result<DeviceCode, String> {
    begin_app_registration_at(FEISHU_ACCOUNTS_ROOT, FEISHU_OPEN_ROOT)
}

fn begin_app_registration_at(accounts_root: &str, open_root: &str) -> Result<DeviceCode, String> {
    let client = http_client()?;
    let response = client
        .post(format!("{accounts_root}{APP_REGISTRATION_PATH}"))
        .form(&[
            ("action", "begin"),
            ("archetype", "PersonalAgent"),
            ("auth_method", "client_secret"),
            ("request_user_info", "open_id tenant_brand"),
        ])
        .send()
        .map_err(|_| "无法连接飞书，请检查网络后重试".to_string())?;
    let status = response.status();
    let payload = response
        .json::<Value>()
        .map_err(|_| "飞书没有返回有效的授权信息，请稍后重试".to_string())?;
    if !status.is_success() || payload.get("error").is_some() {
        return Err("飞书暂时无法发起安全应用创建，请稍后重试。".to_string());
    }
    parse_device_code(&payload, open_root, true)
}

fn begin_user_authorization(app_id: &str, app_secret: &str) -> Result<DeviceCode, String> {
    begin_user_authorization_at(
        FEISHU_ACCOUNTS_ROOT,
        app_id,
        app_secret,
        REQUIRED_USER_SCOPES,
    )
}

fn begin_user_authorization_at(
    accounts_root: &str,
    app_id: &str,
    app_secret: &str,
    scopes: &str,
) -> Result<DeviceCode, String> {
    let response = http_client()?
        .post(format!("{accounts_root}{DEVICE_AUTHORIZATION_PATH}"))
        .basic_auth(app_id, Some(app_secret))
        .form(&[("client_id", app_id), ("scope", scopes)])
        .send()
        .map_err(|_| "无法连接飞书，请检查网络后重试".to_string())?;
    let status = response.status();
    let payload = response
        .json::<Value>()
        .map_err(|_| "飞书没有返回有效的授权信息，请稍后重试".to_string())?;
    if !status.is_success() || payload.get("error").is_some() {
        return Err("飞书暂时无法发起账号授权，请稍后重试。".to_string());
    }
    parse_device_code(&payload, FEISHU_OPEN_ROOT, false)
}

fn parse_device_code(
    payload: &Value,
    open_root: &str,
    registration: bool,
) -> Result<DeviceCode, String> {
    let device_code = string_field(payload, "device_code");
    let user_code = string_field(payload, "user_code");
    if device_code.is_empty() || user_code.is_empty() {
        return Err("飞书授权响应缺少必要信息，请重新发起。".to_string());
    }
    let expires_in = number_field(payload, "expires_in")
        .or_else(|| number_field(payload, "expire_in"))
        .unwrap_or(if registration { 600 } else { 240 });
    let interval_seconds = number_field(payload, "interval").unwrap_or(5).max(1);
    let supplied_url = string_field(payload, "verification_uri_complete");
    let verification_url = safe_verification_url(&supplied_url).unwrap_or_else(|| {
        let mut url = Url::parse(&format!("{open_root}/page/cli")).expect("static Feishu URL");
        url.query_pairs_mut().append_pair("user_code", &user_code);
        url.into()
    });
    Ok(DeviceCode {
        device_code,
        user_code,
        verification_url,
        expires_in,
        interval_seconds,
    })
}

fn safe_verification_url(value: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?;
    let allowed = host == "feishu.cn"
        || host.ends_with(".feishu.cn")
        || host == "larksuite.com"
        || host.ends_with(".larksuite.com");
    (url.scheme() == "https" && allowed).then(|| url.to_string())
}

fn poll_app_registration(device_code: &str) -> Result<PollOutcome<RegisteredApp>, String> {
    poll_app_registration_at(FEISHU_ACCOUNTS_ROOT, device_code)
}

fn poll_app_registration_at(
    accounts_root: &str,
    device_code: &str,
) -> Result<PollOutcome<RegisteredApp>, String> {
    let response = http_client()?
        .post(format!("{accounts_root}{APP_REGISTRATION_PATH}"))
        .form(&[("action", "poll"), ("device_code", device_code)])
        .send()
        .map_err(|_| "network".to_string())?;
    let status = response.status();
    let payload = response
        .json::<Value>()
        .map_err(|_| "response".to_string())?;
    if let Some(outcome) = poll_error_outcome(&payload) {
        return Ok(outcome);
    }
    let app_id = string_field(&payload, "client_id");
    let app_secret = string_field(&payload, "client_secret");
    if status.is_success() && !app_id.is_empty() && !app_secret.is_empty() {
        return Ok(PollOutcome::Complete(RegisteredApp { app_id, app_secret }));
    }
    Err("response".to_string())
}

fn poll_user_authorization(
    app_id: &str,
    app_secret: &str,
    device_code: &str,
) -> Result<PollOutcome<UserTokens>, String> {
    poll_user_authorization_at(FEISHU_OPEN_ROOT, app_id, app_secret, device_code)
}

fn poll_user_authorization_at(
    open_root: &str,
    app_id: &str,
    app_secret: &str,
    device_code: &str,
) -> Result<PollOutcome<UserTokens>, String> {
    let response = http_client()?
        .post(format!("{open_root}{TOKEN_PATH}"))
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code),
            ("client_id", app_id),
            ("client_secret", app_secret),
        ])
        .send()
        .map_err(|_| "network".to_string())?;
    let status = response.status();
    let payload = response
        .json::<Value>()
        .map_err(|_| "response".to_string())?;
    if let Some(outcome) = poll_error_outcome(&payload) {
        return Ok(outcome);
    }
    let access_token = string_field(&payload, "access_token");
    let refresh_token = string_field(&payload, "refresh_token");
    let expires_in = number_field(&payload, "expires_in").unwrap_or_default();
    if status.is_success()
        && !access_token.is_empty()
        && !refresh_token.is_empty()
        && expires_in > 0
    {
        return Ok(PollOutcome::Complete(UserTokens {
            access_token,
            refresh_token,
            expires_in,
            refresh_expires_in: number_field(&payload, "refresh_token_expires_in"),
        }));
    }
    Err("response".to_string())
}

fn poll_error_outcome<T>(payload: &Value) -> Option<PollOutcome<T>> {
    match string_field(payload, "error").as_str() {
        "authorization_pending" => Some(PollOutcome::Pending { slow_down: false }),
        "slow_down" => Some(PollOutcome::Pending { slow_down: true }),
        "access_denied" => Some(PollOutcome::Denied),
        "expired_token" | "invalid_grant" => Some(PollOutcome::Expired),
        _ => None,
    }
}

fn string_field(value: &Value, field: &str) -> String {
    value
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn number_field(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn http_client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("project-track/0.1")
        .build()
        .map_err(|_| "无法初始化飞书连接，请重启应用后重试".to_string())
}

fn registration_progress(device: &DeviceCode) -> FeishuAuthProgress {
    FeishuAuthProgress {
        state: "registering".to_string(),
        message: "请在飞书确认创建本机专用连接应用；App Secret 只会保存在 Windows 凭据库。"
            .to_string(),
        verification_url: Some(device.verification_url.clone()),
        user_code: Some(device.user_code.clone()),
        retry_after_seconds: device.interval_seconds.max(1),
        done: false,
    }
}

fn authorization_progress(device: &DeviceCode, message: &str) -> FeishuAuthProgress {
    FeishuAuthProgress {
        state: "authorizing".to_string(),
        message: message.to_string(),
        verification_url: Some(device.verification_url.clone()),
        user_code: Some(device.user_code.clone()),
        retry_after_seconds: device.interval_seconds.max(1),
        done: false,
    }
}

fn done_progress(state: &str, message: &str) -> FeishuAuthProgress {
    FeishuAuthProgress {
        state: state.to_string(),
        message: message.to_string(),
        verification_url: None,
        user_code: None,
        retry_after_seconds: 0,
        done: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    fn mock_server(body: &'static str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let address = listener.local_addr().expect("mock address");
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept request");
            let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
            let mut request = String::new();
            let mut content_length = 0;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("read header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse().expect("content length");
                    }
                }
                request.push_str(&line);
            }
            let mut request_body = vec![0; content_length];
            reader
                .read_exact(&mut request_body)
                .expect("read request body");
            request.push_str(&String::from_utf8(request_body).expect("UTF-8 request"));
            write!(
                &stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .expect("write response");
            request
        });
        (format!("http://{address}"), server)
    }

    #[test]
    fn app_registration_uses_official_personal_agent_device_flow() {
        let (accounts_root, server) = mock_server(
            r#"{"device_code":"private-device-code","user_code":"A1B2-C3D4","expires_in":600,"interval":5}"#,
        );
        let device = begin_app_registration_at(&accounts_root, "https://open.feishu.cn")
            .expect("registration starts");
        assert_eq!(device.user_code, "A1B2-C3D4");
        assert!(device.verification_url.contains("A1B2-C3D4"));
        let request = server.join().expect("mock server completes");
        assert!(request.contains("POST /oauth/v1/app/registration HTTP/1.1"));
        assert!(request.contains("action=begin"));
        assert!(request.contains("archetype=PersonalAgent"));
        assert!(request.contains("auth_method=client_secret"));
        assert!(request.contains("request_user_info=open_id+tenant_brand"));
        assert!(!request.contains("private-device-code"));
    }

    #[test]
    fn device_authorization_requests_only_the_configured_user_scopes() {
        assert!(REQUIRED_USER_SCOPES
            .split_whitespace()
            .any(|scope| scope == "base:record:retrieve"));
        assert!(!REQUIRED_USER_SCOPES
            .split_whitespace()
            .any(|scope| scope == "base:record:read"));
        let (accounts_root, server) = mock_server(
            r#"{"device_code":"device-2","user_code":"Q2W3-E4R5","verification_uri_complete":"https://open.feishu.cn/page/cli?user_code=Q2W3-E4R5","expires_in":240,"interval":5}"#,
        );
        let device = begin_user_authorization_at(
            &accounts_root,
            "cli_public-id",
            "app-secret-never-logged",
            "offline_access base:app:copy base:record:retrieve",
        )
        .expect("user authorization starts");
        assert_eq!(
            device.verification_url,
            "https://open.feishu.cn/page/cli?user_code=Q2W3-E4R5"
        );
        let request = server.join().expect("mock server completes");
        let request_lower = request.to_ascii_lowercase();
        assert!(request_lower.contains("authorization: basic "));
        assert!(request.contains("client_id=cli_public-id"));
        assert!(request.contains("scope=offline_access+base%3Aapp%3Acopy+base%3Arecord%3Aretrieve"));
        assert!(!request.contains("app-secret-never-logged"));
    }

    #[test]
    fn existing_flow_rejects_registration_and_wrong_app_before_any_poll() {
        let device = || DeviceCode {device_code:"private-test".into(),user_code:"TEST".into(),
            verification_url:"https://open.feishu.cn/page/cli".into(),expires_in:0,interval_seconds:5};
        let mut registration=FeishuAuthFlow::from_registration(device());
        assert!(registration.poll_existing("cli_expected").is_err());
        let mut authorization=FeishuAuthFlow::from_authorization("cli_other".into(),device());
        assert!(authorization.poll_existing("cli_expected").is_err());
        let mut expired=FeishuAuthFlow::from_authorization("cli_expected".into(),device());
        let progress=expired.poll_existing("cli_expected").unwrap();
        assert!(progress.done);assert_eq!(progress.state,"expired");
        assert!(progress.verification_url.is_none());
    }

    #[test]
    fn registration_poll_returns_credentials_only_after_approval() {
        let (accounts_root, server) =
            mock_server(r#"{"client_id":"cli_created","client_secret":"secret-created"}"#);
        let result =
            poll_app_registration_at(&accounts_root, "private-device-code").expect("poll succeeds");
        let PollOutcome::Complete(app) = result else {
            panic!("approved registration should return credentials");
        };
        assert_eq!(app.app_id, "cli_created");
        assert_eq!(app.app_secret, "secret-created");
        let request = server.join().expect("mock server completes");
        assert!(request.contains("action=poll"));
        assert!(request.contains("device_code=private-device-code"));
    }

    #[test]
    fn user_token_poll_uses_v2_device_grant_and_keeps_tokens_private() {
        let (open_root, server) = mock_server(
            r#"{"access_token":"user-access-test","refresh_token":"refresh-test","expires_in":7200,"refresh_token_expires_in":604800}"#,
        );
        let result = poll_user_authorization_at(
            &open_root,
            "cli_test",
            "app-secret-test",
            "private-device-code",
        )
        .expect("poll succeeds");
        let PollOutcome::Complete(tokens) = result else {
            panic!("approved authorization should return tokens");
        };
        assert_eq!(tokens.access_token, "user-access-test");
        assert_eq!(tokens.refresh_token, "refresh-test");
        assert_eq!(tokens.expires_in, 7200);
        let request = server.join().expect("mock server completes");
        assert!(request.contains("POST /open-apis/authen/v2/oauth/token HTTP/1.1"));
        assert!(
            request.contains("grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code")
        );
        assert!(request.contains("device_code=private-device-code"));
        assert!(request.contains("client_secret=app-secret-test"));
        assert!(!request.contains("user-access-test"));
    }

    #[test]
    fn verification_links_reject_non_feishu_destinations() {
        assert!(safe_verification_url("https://open.feishu.cn/page/cli?user_code=ABCD").is_some());
        assert!(safe_verification_url("http://open.feishu.cn/page/cli").is_none());
        assert!(safe_verification_url("https://feishu.example.org/page/cli").is_none());
    }
}
