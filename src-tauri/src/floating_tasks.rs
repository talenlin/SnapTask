//! Floating-list preferences are local only and never delete cached or cloud tasks.
use super::*;
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct Context {
    key: String,
    context_version: u64,
    source: String,
    target_label: String,
    cloud_delete_enabled: bool,
}
#[derive(serde::Serialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct View {
    tasks: Vec<Task>, context: Context, hidden_count: i64,
}
fn context(s:&Storage, version:u64, identifier:&str)->Result<Context,String> {
    let source=active_task_source(s)?;
    let (key,target_label)=if source=="feishu" {
        let c=local_feishu_target(s)?;
        (local_feishu_target_key(&c),format!("飞书 · 应用 {} · 空间 {} · 表 {}",c.app_id,c.app_token,c.table_id))
    } else {
        let c=load_wps_config(s)?;
        (serde_json::json!(["wps",c.webhook_url,c.sheet_name]).to_string(),format!("WPS · {}",c.sheet_name))
    };
    Ok(Context{key,context_version:version,cloud_delete_enabled:source=="feishu"&&floating_feishu::enabled(identifier),source,target_label})
}
#[tauri::command]
pub(crate) fn get_floating_task_view(app:AppHandle)->Result<View,String> {
    let local=app.state::<local_workbench::LocalWorkspaceState>();
    let version=local.feishu_context_version()?;
    local.with_feishu_context(version,||{
        let state=app.state::<AppState>();let s=state.storage.lock().map_err(|_|"本地设置繁忙")?;
        let context=context(&s,version,&app.config().identifier)?;
        Ok(View{tasks:s.list_tasks_for_floating(&context.key)?,hidden_count:s.floating_hidden_count(&context.key)?,context})
    })
}
fn check_context(actual:&Context,key:&str,version:u64)->Result<(),String>{
    if actual.key!=key||actual.context_version!=version {return Err("连接已变化，旧列表操作失效，请刷新后重试".into());}Ok(())
}
#[tauri::command]
pub(crate) fn hide_floating_task(window:WebviewWindow,app:AppHandle,expected_target_key:String,context_version:u64,task:Task)->Result<(),String>{
    if window.label()!="main"{return Err("请从任务浮窗选择隐藏".into());}
    app.state::<local_workbench::LocalWorkspaceState>().with_feishu_context(context_version,||{
        let state=app.state::<AppState>();let s=state.storage.lock().map_err(|_|"本地设置繁忙")?;
        check_context(&context(&s,context_version,&app.config().identifier)?,&expected_target_key,context_version)?;
        if !s.list_tasks_for_floating(&expected_target_key)?.iter().any(|t|t.id==task.id&&t.title==task.title) {return Err("列表已变化，请重新选择任务".into());}
        s.hide_floating_task(&expected_target_key,&task.id)
    })
}
#[tauri::command]
pub(crate) fn show_hidden_floating_tasks(window:WebviewWindow,app:AppHandle,expected_target_key:String,context_version:u64)->Result<(),String>{
    if window.label()!="main"{return Err("请从任务浮窗恢复显示".into());}
    app.state::<local_workbench::LocalWorkspaceState>().with_feishu_context(context_version,||{
        let state=app.state::<AppState>();let s=state.storage.lock().map_err(|_|"本地设置繁忙")?;
        check_context(&context(&s,context_version,&app.config().identifier)?,&expected_target_key,context_version)?;
        s.show_hidden_floating_tasks(&expected_target_key)
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floating_list_context_rejects_old_connection_and_old_revision(){
        let c=Context{key:"a".into(),context_version:5,source:"feishu".into(),target_label:"fixture".into(),cloud_delete_enabled:true};
        assert!(check_context(&c,"a",5).is_ok());
        assert!(check_context(&c,"b",5).is_err());
        assert!(check_context(&c,"a",4).is_err());
    }
}
