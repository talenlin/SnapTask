//! Read-only Feishu adapter. POST /search is a query, never a record mutation.
//! No local storage, links, baselines or sync-success timestamps are changed here.
use super::*;
use std::collections::HashSet;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncInspection {
    pub read_only: bool,
    pub complete: bool,
    pub account_name: String,
    pub subject_id: String,
    pub app_id: String,
    pub app_token: String,
    pub table_id: String,
    pub checked_at: i64,
    pub records: Vec<SyncRecordTime>,
    pub conditional_write_verified: bool,
    pub warnings: Vec<String>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncRecordTime {
    pub shared_fields: Value,
    pub record_id: String,
    pub title: String,
    pub progress: u8,
    pub status: String,
    pub planned_date: Option<String>,
    pub modified_at: Option<i64>,
    pub time_source: String,
}
#[derive(Deserialize)]
struct Identity {
    open_id: String,
    #[serde(default)]
    name: String,
}
#[derive(Deserialize)]
struct Page {
    items: Vec<TimedRecord>,
    has_more: bool,
    page_token: Option<String>,
}
#[derive(Deserialize)]
struct TimedRecord {
    record_id: String,
    fields: serde_json::Map<String, Value>,
    #[serde(default)]
    last_modified_time: Option<Value>,
}
// The official SDK's last_modified_time example is a millisecond int64.
// Do not guess seconds or convert strings; unexpected formats stay unknown.
fn modified_time(raw: Option<&Value>, now: i64) -> Option<i64> {
    raw.and_then(Value::as_i64).filter(|v| {
        *v >= 1_000_000_000_000 && *v <= 253_402_300_799_999 && *v <= now.saturating_add(300_000)
    })
}
pub(super) fn validate_target(config: &FeishuConfig) -> Result<(), String> {
    for value in [&config.app_id, &config.app_token, &config.table_id] {
        if value.is_empty()
            || value.len() > 128
            || !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err("飞书应用或目标表格编号无效，请在设置中重新确认".into());
        }
    }
    Ok(())
}
pub fn inspect(
    config: &FeishuConfig,
    user_token: &str,
    now: i64,
) -> Result<SyncInspection, String> {
    inspect_at(API_ROOT, config, user_token, now)
}
pub(super) fn inspect_at(
    root: &str,
    config: &FeishuConfig,
    user_token: &str,
    now: i64,
) -> Result<SyncInspection, String> {
    validate_target(config)?;
    let client = client()?;
    let identity: Identity = parse_api_response(
        client
            .get(format!("{root}/authen/v1/user_info"))
            .bearer_auth(user_token)
            .send()
            .map_err(|_| "无法核实飞书授权账号，请检查网络或重新授权".to_string())?,
        "核实授权账号",
    )?;
    if identity.open_id.trim().is_empty() {
        return Err("飞书未返回可核实的用户身份，停止检查".into());
    }
    let schema = read_schema(root, config, user_token)?;
    let mut records = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut seen_pages = HashSet::new();
    let mut page_token: Option<String> = None;
    loop {
        if seen_pages.len() >= 100 || records.len() >= 50_000 {
            return Err("飞书只读检查超过安全上限，未将部分结果标为完整".into());
        }
        let mut url = Url::parse(&format!(
            "{root}/bitable/v1/apps/{}/tables/{}/records/search",
            config.app_token, config.table_id
        ))
        .map_err(|_| "飞书目标地址无效".to_string())?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("page_size", "500");
            if let Some(p) = &page_token {
                q.append_pair("page_token", p);
            }
        }
        let response = client
            .post(url)
            .bearer_auth(user_token)
            .json(&json!({"automatic_fields":true}))
            .send()
            .map_err(|_| "飞书只读检索失败；未执行同步".to_string())?;
        let page: Page = parse_api_response(response, "只读检索记录")?;
        for r in page.items {
            if r.record_id.trim().is_empty() || !seen_ids.insert(r.record_id.clone()) {
                return Err("飞书记录编号为空或重复，停止比较；未执行同步".into());
            }
            let modified_at = modified_time(r.last_modified_time.as_ref(), now);
            records.push(SyncRecordTime {
                shared_fields: shared_fields(&r.fields, &schema),
                record_id: r.record_id,
                title: text(r.fields.get("任务名称")),
                progress: progress(r.fields.get("进度")),
                status: text(r.fields.get("状态")),
                planned_date: planned_date(&r.fields),
                modified_at,
                time_source: if modified_at.is_some() {
                    "platformSystem"
                } else {
                    "unknown"
                }
                .into(),
            });
        }
        if !page.has_more {
            break;
        }
        let next = page
            .page_token
            .filter(|p| !p.trim().is_empty())
            .ok_or("飞书分页未返回有效游标，读取不完整")?;
        if !seen_pages.insert(next.clone()) {
            return Err("飞书分页游标重复，读取不完整".into());
        }
        page_token = Some(next);
    }
    let unknown = records.iter().filter(|r| r.modified_at.is_none()).count();
    let mut warnings = vec![
        "只读检查已核实用户授权、字段结构和记录时间；检查本身不修改任何任务。".into(),
        "条件写入能力尚未验证；不保证已消除并发覆盖窗口。".into(),
    ];
    if unknown > 0 {
        warnings.push(format!(
            "{unknown} 条记录缺少有效平台修改时间；保持未知，不以检查时间代替。"
        ));
    }
    Ok(SyncInspection {
        read_only: true,
        complete: true,
        account_name: if identity.name.trim().is_empty() {
            "飞书用户（未返回昵称）".into()
        } else {
            identity.name
        },
        subject_id: format!("{}:{}", config.app_id, identity.open_id),
        app_id: config.app_id.clone(),
        app_token: config.app_token.clone(),
        table_id: config.table_id.clone(),
        checked_at: now,
        records,
        conditional_write_verified: false,
        warnings,
    })
}

fn read_schema(root: &str, config: &FeishuConfig, token: &str) -> Result<Vec<FieldItem>, String> {
    let mut result = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen = HashSet::new();
    loop {
        if seen.len() >= 100 {
            return Err("飞书字段分页超过上限，无法核实字段".into());
        }
        let mut url = Url::parse(&format!(
            "{root}/bitable/v1/apps/{}/tables/{}/fields",
            config.app_token, config.table_id
        ))
        .map_err(|_| "字段地址无效")?;
        url.query_pairs_mut().append_pair("page_size", "100");
        if let Some(value) = &cursor {
            url.query_pairs_mut().append_pair("page_token", value);
        }
        let response = client()?
            .get(url)
            .bearer_auth(token)
            .send()
            .map_err(|_| "无法核实飞书字段结构")?;
        let data: FieldData = parse_api_response(response, "核实同步字段")?;
        result.extend(data.items);
        if !data.has_more {
            break;
        }
        let next = data
            .page_token
            .filter(|v| !v.is_empty())
            .ok_or("字段分页不完整")?;
        if !seen.insert(next.clone()) {
            return Err("字段分页游标重复".into());
        }
        cursor = Some(next);
    }
    let mut names = HashSet::new();
    if result.iter().any(|f| !names.insert(&f.field_name)) {
        return Err("字段名称重复，无法核实同步映射".into());
    }
    Ok(result)
}

fn shared_fields(fields: &serde_json::Map<String, Value>, schema: &[FieldItem]) -> Value {
    let mut result = serde_json::Map::new();
    for (key, aliases) in [
        ("title", vec!["任务名称"]),
        ("detail", vec!["任务详情"]),
        ("progress", vec!["进度"]),
        ("quadrant", vec!["任务类型", "任务标签", "任务类别"]),
        ("today", vec!["今日必完成"]),
        ("startDate", vec!["开始日期", "任务开始日期"]),
        (
            "plannedDate",
            vec![
                "任务计划完成日期",
                "任务计划完成时间",
                "计划完成日期",
                "截止日期",
            ],
        ),
    ] {
        let Some(field) = aliases
            .iter()
            .find_map(|name| schema.iter().find(|f| f.field_name == *name))
        else {
            continue;
        };
        let expected_type = match key {
            "progress" => 2,
            "quadrant" => 3,
            "today" => 7,
            "startDate" | "plannedDate" => 5,
            _ => 1,
        };
        if field.field_type != expected_type {
            continue;
        }
        let blank = Value::Null;
        let raw = fields.get(&field.field_name).unwrap_or(&blank);
        if key == "quadrant" {
            let label = text(Some(raw));
            if ![
                "不重要不紧急",
                "紧急不重要",
                "重要不紧急",
                "重要且紧急",
                "重要紧急",
            ]
            .iter()
            .any(|v| label.contains(v))
            {
                continue;
            }
        }
        let value = match key {
            "progress" => json!(progress(Some(raw))),
            "quadrant" => json!(quadrant(Some(raw))),
            "today" => json!(checkbox(Some(raw))),
            "plannedDate" => {
                let mut selected = serde_json::Map::new();
                selected.insert("计划完成日期".into(), raw.clone());
                json!(planned_date(&selected))
            }
            "startDate" => match raw {
                Value::Number(n) => json!(n.as_i64().map(civil_date_from_unix_ms)),
                Value::Null => Value::Null,
                _ => json!(text(Some(raw)).replace('/', "-")),
            },
            _ => json!(text(Some(raw))),
        };
        result.insert(key.into(), value);
    }
    Value::Object(result)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn schema_distinguishes_blank_dates_from_missing_and_wrong_types() {
        let schema: Vec<FieldItem> = serde_json::from_value(json!([
            {"field_name":"开始日期","type":5}, {"field_name":"任务类别","type":3}, {"field_name":"任务详情","type":1}, {"field_name":"进度","type":1}
        ])).unwrap();
        let mut fields = serde_json::Map::new();
        fields.insert("任务类别".into(), json!("重要不紧急"));
        let parsed = shared_fields(&fields, &schema);
        assert_eq!(parsed["startDate"], Value::Null);
        assert_eq!(parsed["detail"], "");
        assert_eq!(parsed["quadrant"], "in");
        assert!(parsed.get("progress").is_none());
        assert!(parsed.get("plannedDate").is_none());
    }

    #[test]
    #[cfg(all(
        windows,
        not(feature = "feishu-template-v2-acceptance"),
        not(feature = "feishu-release-ui-acceptance"),
        not(feature = "feishu-write-acceptance"),
        not(feature = "local-workbench-acceptance")
    ))]
    #[ignore = "explicit opt-in, read-only inspection of the formal app's saved Feishu user connection"]
    fn live_saved_user_connection_readonly_metadata() {
        assert_eq!(
            std::env::var("PROJECT_TRACK_FEISHU_SYNC_READONLY_PROBE").as_deref(),
            Ok("1"),
            "explicit read-only opt-in required"
        );
        let path = std::path::PathBuf::from(
            std::env::var_os("APPDATA").expect("Windows app-data location"),
        )
        .join("app.snaptask.desktop")
        .join("project-track.sqlite3");
        let db =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("read-only existing connection settings");
        let setting = |key: &str| {
            db.query_row("SELECT value FROM app_settings WHERE key=?1", [key], |r| {
                r.get::<_, String>(0)
            })
            .expect("saved Feishu target required")
        };
        let config = FeishuConfig {
            app_id: setting("feishu.app_id"),
            app_token: setting("feishu.app_token"),
            table_id: setting("feishu.table_id"),
            secret_configured: false,
        };
        validate_target(&config).expect("saved target identifiers");
        println!(
            "S1_READONLY_TARGET {}",
            json!({"appId":config.app_id,"appToken":config.app_token,"tableId":config.table_id})
        );
        assert!(user_authorized_for_app(&config.app_id),"Saved user authorization is absent, expired, or bound to another app; no app-identity fallback");
        let token = read_user_access_token(&config.app_id).expect("valid saved user session");
        let result = inspect(&config, &token, crate::storage::unix_now() * 1000)
            .expect("read-only identity and full record metadata inspection");
        assert!(result.read_only && result.complete && !result.conditional_write_verified);
        println!(
            "S1_READONLY_RESULT {}",
            json!({"accountName":result.account_name,"appId":result.app_id,"tableId":result.table_id,"recordCount":result.records.len(),"knownTimeCount":result.records.iter().filter(|r|r.modified_at.is_some()).count(),"readOnly":result.read_only,"complete":result.complete,"conditionalWriteVerified":false})
        );
    }
    #[test]
    #[cfg(all(
        windows,
        feature = "feishu-template-v2-acceptance",
        not(feature = "feishu-write-acceptance"),
        not(feature = "local-workbench-acceptance")
    ))]
    #[ignore = "explicit opt-in, read-only metadata check of the user-selected second-account isolated table"]
    fn live_second_account_isolated_readonly_metadata() {
        assert_eq!(
            std::env::var("PROJECT_TRACK_FEISHU_SYNC_READONLY_PROBE").as_deref(),
            Ok("1"),
            "explicit read-only opt-in required"
        );
        let config = FeishuConfig {
            app_id: crate::write_acceptance::TEMPLATE_V2_APP_ID.into(),
            app_token: crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN.into(),
            table_id: crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID.into(),
            secret_configured: false,
        };
        let session = read_user_session().expect("Second-account isolated authorization unavailable; reconnect this isolated app, never fall back to the formal account");
        println!(
            "S1_ISOLATED_AUTH {}",
            json!({"boundAppId":session.auth_app_id,"accessExpiresAt":session.access_expires_at,"refreshExpiresAt":session.refresh_expires_at,"expectedAppId":config.app_id})
        );
        assert!(user_authorized_for_app(&config.app_id), "Isolated user authorization expired or bound to a different app; no cloud requests made");
        let token =
            read_user_access_token(&config.app_id).expect("valid isolated user authorization");
        let result = inspect(&config, &token, crate::storage::unix_now() * 1000)
            .expect("read-only identity and complete metadata check");
        assert!(result.read_only && result.complete && !result.conditional_write_verified);
        println!(
            "S1_ISOLATED_RESULT {}",
            json!({"accountName":result.account_name,"appId":result.app_id,"appToken":result.app_token,"tableId":result.table_id,"recordCount":result.records.len(),"knownTimeCount":result.records.iter().filter(|r|r.modified_at.is_some()).count(),"recordMetadata":result.records.iter().map(|record|json!({"recordId":record.record_id,"modifiedAt":record.modified_at,"timeSource":record.time_source})).collect::<Vec<_>>(),"readOnly":true,"complete":true,"conditionalWriteVerified":false})
        );
    }

    fn server(responses: Vec<Value>) -> (String, thread::JoinHandle<Vec<String>>) {
        fault_server(responses.into_iter().map(|body| (200, Some(body.to_string()))).collect())
    }
    // Local transport only: real adapter, synthetic responses, no saved token or cloud access.
    // None closes the connection after reading a request, modelling an interrupted response.
    pub(crate) fn fault_server(responses: Vec<(u16, Option<String>)>) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            let expected = responses.len();
            let mut responses = responses.into_iter();
            let mut deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline { break; }
                        thread::sleep(Duration::from_millis(5));
                        continue;
                    }
                    Err(e) => panic!("fixture accept: {e}"),
                };
                // Windows accepted sockets can inherit the listener's nonblocking mode.
                // Poll accept with a deadline, but read each request in bounded blocking mode.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                loop {
                    let mut b = [0; 4096];
                    let n = stream.read(&mut b).unwrap();
                    assert!(n > 0, "fixture request ended before headers/body");
                    bytes.extend_from_slice(&b[..n]);
                    if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&bytes[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if bytes.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                requests.push(String::from_utf8(bytes).unwrap());
                let (status, body) = responses.next().unwrap_or((500, Some("unexpected retry".into())));
                if let Some(body) = body {
                    write!(stream,"HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
                }
                // Observe unexpected retries/fallbacks briefly without a hanging accept.
                deadline = std::time::Instant::now() + if requests.len() >= expected {
                    Duration::from_millis(150)
                } else { Duration::from_secs(5) };
            }
            assert!(requests.len() >= expected, "adapter stopped before fixture's expected stage");
            requests
        });
        (root, handle)
    }
    fn identity() -> Value {
        json!({"code":0,"msg":"ok","data":{"open_id":"user-a","name":"验收账号"}})
    }
    #[test]
    fn loopback_server_accepts_delayed_request_headers() {
        let (root,server)=fault_server(vec![(200,Some(identity().to_string()))]);
        let mut stream=std::net::TcpStream::connect(root.trim_start_matches("http://")).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        // A valid client may connect before it sends any bytes. The accepted socket must wait.
        thread::sleep(Duration::from_millis(40));
        stream.write_all(b"GET /authen/v1/user_info HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").unwrap();
        let mut response=String::new();
        let read=stream.read_to_string(&mut response);
        let requests=server.join();
        assert!(read.is_ok() && requests.is_ok(),"loopback fixture must wait for valid delayed headers");
        assert!(response.starts_with("HTTP/1.1 200"));
        assert_eq!(requests.unwrap().len(),1);
    }
    fn config() -> FeishuConfig {
        FeishuConfig {
            app_id: "cli_demo".into(),
            app_token: "base_demo".into(),
            table_id: "tbl_demo".into(),
            secret_configured: false,
        }
    }
    fn page(items: Value, more: bool, token: Option<&str>) -> Value {
        json!({"code":0,"msg":"ok","data":{"items":items,"has_more":more,"page_token":token}})
    }
    #[test]
    fn readonly_query_uses_system_metadata_and_complete_pages_not_read_time() {
        let n = 1_800_000_000_000;
        let (root, handle) = server(vec![
            identity(),
            page(json!([{"field_name":"任务名称","type":1}]), false, None),
            page(
                json!([{"record_id":"a","fields":{"任务名称":"任务类别兼容"},"last_modified_time":n-12345}]),
                true,
                Some("next"),
            ),
            page(
                json!([{"record_id":"b","fields":{"任务名称":"历史记录"}}]),
                false,
                None,
            ),
        ]);
        let p = inspect_at(&root, &config(), "synthetic-user-token", n).unwrap();
        let requests = handle.join().unwrap();
        assert_eq!(p.records.len(), 2);
        assert_eq!(p.records[0].modified_at, Some(n - 12345));
        assert_eq!(p.records[0].progress, 0);
        assert_eq!(p.records[1].modified_at, None);
        assert_eq!(p.subject_id, "cli_demo:user-a");
        assert!(p.read_only && p.complete && !p.conditional_write_verified);
        assert!(requests[0].starts_with("GET /authen/v1/user_info "));
        assert!(requests[1].starts_with("GET /bitable/v1/apps/base_demo/tables/tbl_demo/fields?"));
        assert!(requests[2]
            .starts_with("POST /bitable/v1/apps/base_demo/tables/tbl_demo/records/search?"));
        assert!(requests[2].contains("\"automatic_fields\":true"));
        assert!(requests[3].contains("page_token=next"));
    }
    #[test]
    fn failed_identity_prevents_any_record_read() {
        let (root, h) = server(vec![json!({"code":999,"msg":"not authorized"})]);
        assert!(inspect_at(&root, &config(), "synthetic", 1_800_000_000_000).is_err());
        assert_eq!(h.join().unwrap().len(), 1);
    }
    #[test]
    fn sync_boundary_http_errors_stop_at_identity_schema_or_later_page_without_fallback() {
        // Deliberately also use code=0 on HTTP errors: status alone must fail closed.
        for status in [401, 403, 429, 503] {
            for stage in 0..4 {
                let mut replies = vec![
                    (200, Some(identity().to_string())),
                    (200, Some(page(json!([]), false, None).to_string())),
                    (200, Some(page(json!([{"record_id":"a","fields":{},"last_modified_time":1_799_999_999_000_i64}]), true, Some("next")).to_string())),
                    (200, Some(page(json!([]), false, None).to_string())),
                ];
                replies[stage] = (status, Some(json!({"code":0,"msg":"synthetic HTTP failure"}).to_string()));
                replies.truncate(stage + 1);
                let (root, h) = fault_server(replies);
                assert!(inspect_at(&root, &config(), "synthetic-only", 1_800_000_000_000).is_err(), "status {status}, stage {stage}");
                let requests = h.join().unwrap();
                assert_eq!(requests.len(), stage + 1, "no retry, token fallback or later query");
                assert!(requests.iter().all(|r| r.starts_with("GET /authen/v1/user_info ")
                    || r.starts_with("GET /bitable/v1/apps/base_demo/tables/tbl_demo/fields?")
                    || r.starts_with("POST /bitable/v1/apps/base_demo/tables/tbl_demo/records/search?")));
            }
        }
    }
    #[test]
    fn sync_boundary_expired_or_forbidden_business_codes_never_return_partial_success() {
        for (stage, code, message) in [(0,99991663,"synthetic expired"),(1,91403,"Forbidden"),(3,91403,"Forbidden")] {
            let mut replies = vec![identity(), page(json!([]), false, None),
                page(json!([{"record_id":"a","fields":{}}]), true, Some("next")),
                page(json!([]), false, None)];
            replies[stage] = json!({"code":code,"msg":message});
            replies.truncate(stage + 1);
            let (root, h) = server(replies);
            let error = inspect_at(&root, &config(), "synthetic-only", 1_800_000_000_000).unwrap_err();
            assert!(error.contains(&code.to_string()));
            assert_eq!(h.join().unwrap().len(), stage + 1);
        }
    }
    #[test]
    fn sync_boundary_interrupted_or_malformed_response_never_retries_or_returns_partial_pages() {
        for stage in [0,3] {
            for body in [None, Some("not-json".to_string())] {
                let mut replies = vec![(200,Some(identity().to_string())),
                    (200,Some(page(json!([]),false,None).to_string())),
                    (200,Some(page(json!([{"record_id":"a","fields":{}}]),true,Some("next")).to_string())),
                    (200,Some(page(json!([]),false,None).to_string()))];
                replies[stage] = (200,body);
                replies.truncate(stage + 1);
                let (root,h) = fault_server(replies);
                assert!(inspect_at(&root,&config(),"synthetic-only",1_800_000_000_000).is_err());
                assert_eq!(h.join().unwrap().len(),stage + 1);
            }
        }
        // An actual refused loopback connection exercises the adapter's network error path.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let root = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let error = inspect_at(&root,&config(),"synthetic-only",1_800_000_000_000).unwrap_err();
        assert!(error.contains("无法核实飞书授权账号"));
        assert!(!error.contains("synthetic-only"));
    }
    #[test]
    fn missing_or_repeating_cursor_duplicates_and_missing_completeness_fail_closed() {
        for pages in [
            vec![page(json!([]), true, None)],
            vec![
                page(json!([]), true, Some("x")),
                page(json!([]), true, Some("x")),
            ],
            vec![page(
                json!([{"record_id":"a","fields":{}},{"record_id":"a","fields":{}}]),
                false,
                None,
            )],
            vec![json!({"code":0,"data":{"items":[]}})],
        ] {
            let mut responses = vec![identity()];
            responses.push(page(json!([]), false, None));
            responses.extend(pages);
            let (root, h) = server(responses);
            assert!(inspect_at(&root, &config(), "synthetic", 1_800_000_000_000).is_err());
            h.join().unwrap();
        }
    }
    #[test]
    fn timestamps_do_not_guess_units_strings_null_or_future() {
        let n = 1_800_000_000_000;
        for raw in [
            Value::Null,
            json!(n / 1000),
            json!(n.to_string()),
            json!(-1),
            json!(n + 300001),
        ] {
            assert!(modified_time(Some(&raw), n).is_none());
        }
        assert_eq!(modified_time(Some(&json!(n - 1)), n), Some(n - 1));
        let mut bad = config();
        bad.app_token = "../other".into();
        assert!(validate_target(&bad).is_err());
    }
}
