//! New direct-Feishu identity: fixed app, separate vault, no task/database commands.
use serde_json::{json, Value};
use tauri::Manager;
#[path = "local_workbench_auth.rs"]
mod auth;
const APP_ID: &str = "cli_0000000000000003";

#[tauri::command]
fn direct_account_status() -> Value {
    json!({"appId":APP_ID,
        "secretConfigured":crate::feishu::read_oauth_app_secret(APP_ID).is_ok(),
        "authorized":crate::feishu::user_authorized_for_app(APP_ID)})
}

#[tauri::command]
fn direct_account_save_secret(secret: String, confirmed: bool, state: tauri::State<'_,auth::IsolatedAuthState>) -> Result<(),String> {
    if !confirmed { return Err("请先确认使用飞书身份并同意保存到独立Windows凭据库".into()); }
    // Only this app's dedicated service entry is writable. No shared/global secret.
    state.store_app_secret(&secret)
}

#[tauri::command]
async fn direct_account_check_token() -> Result<Value,String> {
    tauri::async_runtime::spawn_blocking(|| {
        let secret=crate::feishu::read_oauth_app_secret(APP_ID)?;
        let response=reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none()).build().map_err(|_|"无法创建安全请求")?
            .post("https://open.feishu.cn/open-apis/auth/v3/app_access_token/internal")
            .json(&json!({"app_id":APP_ID,"app_secret":secret}))
            .send().map_err(|_|"应用令牌检查网络失败；未重试")?;
        let http=response.status().as_u16();
        let payload=response.json::<Value>().map_err(|_|"应用令牌检查返回格式异常；未重试")?;
        let code=payload.get("code").and_then(Value::as_i64);
        let usable=code==Some(0) && payload.get("app_access_token").and_then(Value::as_str).is_some_and(|s|!s.is_empty());
        Ok(json!({"appId":APP_ID,"httpStatus":http,"code":code,"usable":usable,
            "tokenPersisted":false,"dataApiCalled":false,"retried":false}))
    }).await.map_err(|_|"应用令牌检查中断".to_string())?
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_single_instance::init(|app,_,_| { if let Some(w)=app.get_webview_window("main") {let _=w.set_focus();} }))
        .manage(auth::IsolatedAuthState::default())
        .setup(|app| {crate::feishu::validate_credential_profile(&app.config().identifier).map_err(std::io::Error::other)?;Ok(())})
        .invoke_handler(tauri::generate_handler![direct_account_status,direct_account_save_secret,direct_account_check_token,
            auth::local_start_isolated_feishu_auth,auth::local_open_isolated_feishu_auth,
            auth::local_poll_isolated_feishu_auth,auth::local_cancel_isolated_feishu_auth])
        .run(tauri::generate_context!()).expect("独立飞书身份连接启动失败");
}

#[cfg(all(test,windows))]
#[path = "direct_account_copy_acceptance.rs"]
mod copy_acceptance;

#[cfg(all(test,windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "explicit fixed-new-app read-only source-template check; never copy or write records"]
    fn live_direct_account_template_readonly() {
        assert_eq!(std::env::var("PROJECT_TRACK_DIRECT_READONLY").ok().as_deref(),Some(APP_ID),"fixed-app opt-in required");
        crate::feishu::validate_credential_profile("app.snaptask.desktop.directfeishu20261020").expect("isolated credential service");
        assert!(crate::feishu::user_authorized_for_app(APP_ID),"new application has no authorized isolated session");
        let token=crate::feishu::read_user_access_token(APP_ID).expect("read only new app session");
        let source=crate::bundled_feishu_template_app_token().expect("bundled template source");
        assert_eq!(source,"Example00000000000000000005","do not probe arbitrary cloud workspaces");
        crate::feishu::ensure_template_tables_empty(&source,&token).expect("source must be readable and empty; no copying attempted");
        let table=crate::feishu::discover_task_table(&source,&token).expect("read task schema");
        let config=crate::feishu::FeishuConfig{app_id:APP_ID.into(),app_token:source,table_id:table.table_id.clone(),secret_configured:true};
        let inspection=crate::feishu::inspect_sync_with_user_token(&config,&token,crate::storage::unix_now()*1000).expect("verify identity and read empty source records");
        assert!(inspection.complete && inspection.read_only && inspection.records.is_empty());
        let status=crate::feishu::inspect_workspace_as_user(&config.app_token,&table.table_id,&token).expect("read source views and dashboard metadata");
        assert!(status.missing_required_fields.is_empty());
        assert!(status.status_is_formula,"progress must determine status");
        assert!(status.inspection_warnings.is_empty(),"source structure check has unverified parts");
        for (name,kind) in [("任务总览","grid"),("四象限看板","kanban"),("今日必做看板","kanban"),("今日任务","grid")] {
            assert!(status.views.iter().any(|v|v.name==name&&v.kind==kind),"required source view missing: {name}");
        }
        assert!(status.dashboards.iter().any(|v|v.name=="个人任务仪表盘"));
        println!("DIRECT_ACCOUNT_READONLY {}",json!({"appId":APP_ID,"sourceAppToken":config.app_token,"tableId":table.table_id,"tableName":table.name,
            "accountVerified":!inspection.subject_id.is_empty(),"recordCount":inspection.records.len(),"fieldCount":table.fields.len(),
            "views":status.views,"dashboardCount":status.dashboards.len(),"statusIsFormula":status.status_is_formula,
            "cloudMutation":false,"copied":false,"databaseOpened":false}));
    }
}
