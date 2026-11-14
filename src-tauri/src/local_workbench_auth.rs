//! Fixed-app, acceptance-only reauthorization. No record mutations or app creation.
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;
use crate::feishu_auth::{FeishuAuthFlow, FeishuAuthProgress};

#[derive(Clone, Default)]
pub struct IsolatedAuthState(Arc<Mutex<AuthSession>>);
#[derive(Default)]
struct AuthSession { flow: Option<FeishuAuthFlow>, url: Option<String> }

#[cfg(feature="feishu-direct-account-acceptance")]
impl IsolatedAuthState {
    pub fn store_app_secret(&self, secret: &str) -> Result<(),String> {
        let session=self.0.lock().map_err(|_|"隔离授权状态繁忙")?;
        if session.flow.is_some() {return Err("请先停止等待，再修改新应用密钥".into());}
        crate::feishu::save_oauth_app_credentials(expected_app(),secret)
    }
}

fn allowed(identifier: &str) -> bool {
    if cfg!(feature="feishu-direct-account-acceptance") {
        return crate::feishu::validate_credential_profile(identifier).is_ok();
    }
    cfg!(feature="feishu-template-v2-acceptance")
        && identifier=="app.snaptask.desktop.localworkbench.syncs3"
}
fn expected_app() -> &'static str {
    if cfg!(feature="feishu-direct-account-acceptance") { "cli_0000000000000003" }
    else { crate::write_acceptance::TEMPLATE_V2_APP_ID }
}
fn guard(window: &tauri::WebviewWindow) -> Result<(),String> {
    if !allowed(&window.app_handle().config().identifier) { return Err("仅第二账号隔离验收允许重新授权".into()); }
    Ok(())
}
fn verified_url(value: &str) -> bool {
    reqwest::Url::parse(value).is_ok_and(|url|url.scheme()=="https"
        && url.username().is_empty() && url.password().is_none()
        && matches!(url.host_str(),Some("open.feishu.cn"|"accounts.feishu.cn"))
        && url.port_or_known_default()==Some(443))
}

#[tauri::command]
pub async fn local_start_isolated_feishu_auth(window: tauri::WebviewWindow, state: State<'_,IsolatedAuthState>) -> Result<FeishuAuthProgress,String> {
    guard(&window)?;
    let state=state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut session=state.0.lock().map_err(|_|"隔离授权状态繁忙")?;
        if session.flow.is_some(){return Err("已有隔离授权请求；请检查结果或停止等待后重新发起".into());}
        let (flow,progress)=FeishuAuthFlow::start_existing(expected_app())?;
        let url=progress.verification_url.as_deref().filter(|s|verified_url(s))
            .ok_or("飞书授权地址未通过安全检查")?.to_string();
        session.url=Some(url);session.flow=Some(flow);Ok(progress)
    }).await.map_err(|_|"隔离授权请求未完成".to_string())?
}

#[tauri::command]
pub async fn local_poll_isolated_feishu_auth(window: tauri::WebviewWindow, state: State<'_,IsolatedAuthState>) -> Result<FeishuAuthProgress,String> {
    guard(&window)?;let state=state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut session=state.0.lock().map_err(|_|"隔离授权状态繁忙")?;
        let progress=session.flow.as_mut().ok_or("没有进行中的隔离授权，请重新发起")?
            .poll_existing(expected_app())?;
        if progress.done {session.flow=None;session.url=None;}
        Ok(progress)
    }).await.map_err(|_|"隔离授权检查未完成".to_string())?
}

#[tauri::command]
pub fn local_cancel_isolated_feishu_auth(window: tauri::WebviewWindow,state: State<'_,IsolatedAuthState>) -> Result<(),String> {
    guard(&window)?;let mut session=state.0.lock().map_err(|_|"隔离授权状态繁忙")?;
    *session=AuthSession::default();Ok(())
}

#[tauri::command]
pub fn local_open_isolated_feishu_auth(window: tauri::WebviewWindow,state: State<'_,IsolatedAuthState>) -> Result<(),String> {
    guard(&window)?;
    let url=state.0.lock().map_err(|_|"隔离授权状态繁忙")?.url.clone().ok_or("请先发起隔离重新授权")?;
    if !verified_url(&url){return Err("授权地址未通过安全检查".into());}
    window.app_handle().opener().open_url(url,None::<&str>).map_err(|_|"无法打开授权页面，请重新发起后重试".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_formal_identity_and_untrusted_urls(){
        assert!(!allowed("app.snaptask.desktop"));
        assert!(!allowed("app.snaptask.desktop.release060fresh"));
        assert_eq!(allowed("app.snaptask.desktop.localworkbench.syncs3"),cfg!(feature="feishu-template-v2-acceptance") && !cfg!(feature="feishu-direct-account-acceptance"));
        assert_ne!(expected_app(), "");
        assert!(verified_url("https://open.feishu.cn/page/cli?user_code=TEST"));
        for url in ["http://open.feishu.cn/x","https://open.feishu.cn.evil.test/","https://evil.test/","https://name@open.feishu.cn/x","https://open.feishu.cn:444/x","file:///x"] { assert!(!verified_url(url)); }
    }
}
