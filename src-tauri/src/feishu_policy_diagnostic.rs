//! Explicit one-shot token-endpoint diagnostic. Never stores tokens or calls data APIs.
use super::*;

#[test]
#[ignore = "one request to the fixed second-account app token endpoint; no data API or persistence"]
fn live_secondary_app_policy_probe() {
    let app_id = crate::write_acceptance::TEMPLATE_V2_APP_ID;
    assert_eq!(std::env::var("PROJECT_TRACK_FEISHU_POLICY_PROBE").ok().as_deref(), Some(app_id),
        "explicit fixed-app opt-in is required");
    let secret = read_oauth_app_secret(app_id).expect("fixed-app credential unavailable; do not fall back");
    let response = client().expect("HTTP client")
        .post(format!("{API_ROOT}/auth/v3/app_access_token/internal"))
        .json(&json!({"app_id":app_id,"app_secret":secret}))
        .send().map_err(|_| "one-shot token endpoint transport failed; no retry").unwrap();
    let status=response.status().as_u16();
    let log_id=response.headers().get("x-tt-logid").and_then(|v|v.to_str().ok())
        .filter(|s|s.len()<=160 && s.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'||b==b'_'))
        .map(str::to_string);
    let payload=response.json::<Value>().map_err(|_|"token endpoint returned non-JSON; no retry").unwrap();
    let code=payload.get("code").and_then(Value::as_i64);
    let policy_text=payload.get("msg").and_then(Value::as_str).unwrap_or("").to_ascii_lowercase();
    println!("SECONDARY_POLICY_DIAGNOSTIC {}",json!({
        "appId":app_id,"endpoint":"auth/v3/app_access_token/internal","httpStatus":status,
        "code":code,"policyDenied":policy_text.contains("access control policy")||policy_text.contains("data security policy"),
        "logId":log_id,"tokenPersisted":false,"dataApiCalled":false,"retried":false
    }));
    assert_eq!(code,Some(0),"fixed secondary app token endpoint rejected; inspect sanitized diagnostic, never retry automatically");
    assert!(payload.get("app_access_token").and_then(Value::as_str).is_some_and(|s|!s.is_empty()),
        "no usable token returned; token was not printed or persisted");
}
