//! Scoped updates only; never creates records or changes table structure.
use super::*;

// Non-serializable, non-cloneable, single-use prepared request. Browser input cannot
// supply its URL or payload; preparation requires a complete authenticated inspection.
pub(crate) struct PreparedUserSyncUpdate {
    root: String,
    config: FeishuConfig,
    token: String,
    record_id: String,
    payload: Value,
}
pub(crate) fn prepare_user_sync_update(config:&FeishuConfig,token:&str,read:&SyncInspection,record_id:&str,content:&Value,status:&str)->Result<PreparedUserSyncUpdate,String> {
    prepare_user_sync_update_at(API_ROOT,config,token,read,record_id,content,status)
}
fn prepare_user_sync_update_at(root:&str,config:&FeishuConfig,token:&str,read:&SyncInspection,record_id:&str,content:&Value,status:&str)->Result<PreparedUserSyncUpdate,String> {
    prepare_verified_sync_request_at(root,config,token,read,record_id,content,status,false)
}
pub(crate) fn prepare_user_sync_restore(config:&FeishuConfig,token:&str,read:&SyncInspection,record_id:&str,content:&Value,status:&str)->Result<PreparedUserSyncUpdate,String> {
    prepare_user_sync_restore_at(API_ROOT,config,token,read,record_id,content,status)
}
fn prepare_user_sync_restore_at(root:&str,config:&FeishuConfig,token:&str,read:&SyncInspection,record_id:&str,content:&Value,status:&str)->Result<PreparedUserSyncUpdate,String> {
    prepare_verified_sync_request_at(root,config,token,read,record_id,content,status,true)
}
fn prepare_verified_sync_request_at(root:&str,config:&FeishuConfig,token:&str,read:&SyncInspection,record_id:&str,content:&Value,status:&str,restore:bool)->Result<PreparedUserSyncUpdate,String> {
    sync_inspect::validate_target(config)?;
    let prefix=format!("{}:",config.app_id);
    if token.is_empty()||!read.complete||!read.read_only||read.app_id!=config.app_id||read.app_token!=config.app_token||read.table_id!=config.table_id
        ||!read.subject_id.strip_prefix(&prefix).is_some_and(|id|!id.trim().is_empty()) {
        return Err("用户身份或目标未核实，不能准备上传".into());
    }
    if record_id.is_empty()||record_id.len()>128||!record_id.chars().all(|c|c.is_ascii_alphanumeric())||!read.records.iter().any(|r|r.record_id==record_id) {
        return Err("所选记录不在本次已核实的目标表中".into());
    }
    let expected_status=match content.get("progress").and_then(Value::as_u64) {Some(0)=>"未开始",Some(100)=>"已完成",Some(1..=99)=>"进行中",_=>return Err("进度无效".into())};
    if !restore&&status!=expected_status {return Err("状态与进度不一致，不能上传".into());}
    // Restoration receives only the immutable journal's explicitly previewed status,
    // which may be a prior manual label. Ordinary uploads remain progress-derived.
    if status.chars().count()>200||status.chars().any(char::is_control) {return Err("恢复状态无效".into());}
    let schema=list_field_items_at(root,&config.app_token,&config.table_id,token)?;
    Ok(PreparedUserSyncUpdate{root:root.into(),config:config.clone(),token:token.into(),record_id:record_id.into(),payload:sync_payload(&schema,content,status)?})
}
pub(crate) fn send_user_sync_update(prepared:PreparedUserSyncUpdate)->Result<(),String> {
    send_record_update_at(&prepared.root,&prepared.config,&prepared.token,&prepared.record_id,&prepared.payload)
}

fn allowed_target(config:&FeishuConfig)->bool {
    #[cfg(any(feature="feishu-direct-sync-ui-acceptance",all(test,feature="feishu-direct-account-acceptance")))]
    {return config.app_id=="cli_0000000000000003" && config.app_token=="Example00000000000000000007" && config.table_id=="tbl0000000000001";}
    #[cfg(not(any(feature="feishu-direct-sync-ui-acceptance",all(test,feature="feishu-direct-account-acceptance"))))]
    {config.app_id==crate::write_acceptance::TEMPLATE_V2_APP_ID
        && config.app_token==crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN
        && config.table_id==crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID}
}
fn allowed_record(record_id:&str,preparing:bool)->bool{
    if cfg!(any(feature="feishu-direct-sync-ui-acceptance",all(test,feature="feishu-direct-account-acceptance"))){
        return crate::direct_account_scope::RECORD_IDS.contains(&record_id)
            || (cfg!(test)&&preparing&&record_id=="seed");
    }
    !record_id.is_empty()&&record_id.chars().all(|c|c.is_ascii_alphanumeric())
}

pub fn prepare_sync_update(
    config: &FeishuConfig,
    token: &str,
    record_id: &str,
    content: &Value,
    status: &str,
) -> Result<Value, String> {
    prepare_sync_update_at(API_ROOT, config, token, record_id, content, status)
}
fn prepare_sync_update_at(
    root: &str, config: &FeishuConfig, token: &str, record_id: &str,
    content: &Value, status: &str,
) -> Result<Value, String> {
    if !allowed_target(config) {
        return Err("同步写入仅允许第二账号隔离表".into());
    }
    if !allowed_record(record_id,true) {
        return Err("记录编号无效".into());
    }
    let schema = list_field_items_at(root, &config.app_token, &config.table_id, token)?;
    sync_payload(&schema, content, status)
}

fn sync_payload(schema: &[FieldItem], content: &Value, status: &str) -> Result<Value, String> {
    let mut payload = serde_json::Map::new();
    for (key, aliases, field_type) in [
        ("title", vec!["任务名称"], 1),
        ("detail", vec!["任务详情"], 1),
        ("progress", vec!["进度"], 2),
        ("quadrant", vec!["任务类型", "任务标签", "任务类别"], 3),
        ("today", vec!["今日必完成"], 7),
        ("startDate", vec!["开始日期", "任务开始日期"], 5),
        (
            "plannedDate",
            vec![
                "任务计划完成日期",
                "任务计划完成时间",
                "计划完成日期",
                "截止日期",
            ],
            5,
        ),
    ] {
        let field = field_named(schema, &aliases)
            .filter(|f| f.field_type == field_type && !is_formula_field(f))
            .ok_or_else(|| format!("{key}字段缺失、类型不支持或不可写"))?;
        let value = content.get(key).ok_or("提交内容不完整")?;
        let value = match key {
            "progress" => progress_write_value(
                field,
                u8::try_from(value.as_u64().filter(|n| *n <= 100).ok_or("进度无效")?)
                    .map_err(|_| "进度无效")?,
            ),
            "quadrant" => {
                let v = value
                    .as_str()
                    .filter(|v| ["iu", "in", "un", "nn"].contains(v))
                    .ok_or("四象限无效")?;
                json!(quadrant_write_label(field, v))
            }
            "today" => json!(value.as_bool().ok_or("今日待办无效")?),
            "startDate" | "plannedDate" => {
                if value.is_null() {
                    Value::Null
                } else {
                    json!(iso_date_to_unix_ms(value.as_str().ok_or("日期无效")?)?)
                }
            }
            _ => json!(value.as_str().ok_or("文本字段无效")?),
        };
        payload.insert(field.field_name.clone(), value);
    }
    if let Some(field) = field_named(schema, &["状态"]) {
        if !is_formula_field(field) {
            if field.field_type != 3 {
                return Err("状态字段不可写".into());
            }
            payload.insert(field.field_name.clone(), json!(status));
        }
    }
    Ok(json!({"fields":payload}))
}

pub fn send_sync_update(
    config: &FeishuConfig,
    token: &str,
    record_id: &str,
    payload: &Value,
) -> Result<(), String> {
    send_sync_update_at(API_ROOT, config, token, record_id, payload)
}
fn send_sync_update_at(
    root: &str, config: &FeishuConfig, token: &str, record_id: &str, payload: &Value,
) -> Result<(), String> {
    if !allowed_target(config) || !allowed_record(record_id,false)
    {
        return Err("同步写入目标无效".into());
    }
    send_record_update_at(root,config,token,record_id,payload)
}
fn send_record_update_at(root:&str,config:&FeishuConfig,token:&str,record_id:&str,payload:&Value)->Result<(),String> {
    let response = client()?
        .put(format!(
            "{root}/bitable/v1/apps/{}/tables/{}/records/{record_id}",
            config.app_token, config.table_id
        ))
        .bearer_auth(token)
        .json(payload)
        .send()
        .map_err(|_| "写入响应未确认，需要只读核实结果")?;
    parse_api_response::<Value>(response, "同步更新记录")?;
    Ok(())
}

/// An intent already exists before this operation. A failed send is not proof of non-delivery.
pub struct SyncUpdateAttempt {
    pub unsent: Option<String>,
    pub inspection: Option<SyncInspection>,
}
pub fn attempt_sync_update(
    config: &FeishuConfig, token: &str, record_id: &str, content: &Value, status: &str,
    now: impl FnOnce() -> Option<i64>,
) -> SyncUpdateAttempt {
    attempt_sync_update_at(API_ROOT, config, token, record_id, content, status, now)
}
pub(super) fn attempt_sync_update_at(
    root: &str, config: &FeishuConfig, token: &str, record_id: &str, content: &Value,
    status: &str, now: impl FnOnce() -> Option<i64>,
) -> SyncUpdateAttempt {
    let payload = match prepare_sync_update_at(root, config, token, record_id, content, status) {
        Ok(payload) => payload,
        Err(reason) => return SyncUpdateAttempt {unsent:Some(reason),inspection:None},
    };
    // Exactly one mutation; errors (including explicit rejection) are followed by readback only.
    let _ = send_sync_update_at(root, config, token, record_id, &payload);
    let inspection = now().and_then(|time| sync_inspect::inspect_at(root,config,token,time).ok());
    SyncUpdateAttempt {unsent:None,inspection}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formal_restore_preserves_previewed_manual_status_without_loosening_normal_upload_status() {
        let config=FeishuConfig{app_id:"cli_restore_transport".into(),app_token:"base_restore".into(),table_id:"tbl_restore".into(),secret_configured:false};
        let content=json!({"title":"restore test","detail":"synthetic","progress":60,"quadrant":"in","today":false,"startDate":null,"plannedDate":null});
        let read=SyncInspection{read_only:true,complete:true,account_name:"synthetic".into(),subject_id:"cli_restore_transport:user_a".into(),
            app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1800000000000,conditional_write_verified:false,warnings:vec![],
            records:vec![SyncRecordTime{record_id:"recRestore".into(),title:"restore test".into(),progress:25,status:"进行中".into(),planned_date:None,modified_at:Some(1799900000000),time_source:"platform".into(),shared_fields:content.clone()}]};
        let schema=json!({"code":0,"msg":"ok","data":{"has_more":false,"items":[{"field_name":"任务名称","type":1},{"field_name":"任务详情","type":1},
            {"field_name":"进度","type":2},{"field_name":"任务类别","type":3},{"field_name":"今日必完成","type":7},{"field_name":"开始日期","type":5},{"field_name":"计划完成日期","type":5},{"field_name":"状态","type":3}]}});
        let (root,server)=sync_inspect::tests::fault_server(vec![(200,Some(schema.to_string())),(200,Some(json!({"code":0,"msg":"ok","data":{}}).to_string()))]);
        assert!(prepare_user_sync_update_at(&root,&config,"synthetic-token",&read,"recRestore",&content,"暂停").is_err(),"ordinary upload must still derive status from progress");
        let prepared=prepare_user_sync_restore_at(&root,&config,"synthetic-token",&read,"recRestore",&content,"暂停").unwrap();
        send_user_sync_update(prepared).unwrap();
        let requests=server.join().unwrap();assert_eq!(requests.len(),2);
        assert!(requests[1].starts_with("PUT "));assert!(requests[1].contains("\"状态\":\"暂停\""));
    }
    #[test]
    fn formal_update_is_bound_to_verified_user_target_and_record_and_sends_once_on_lost_response() {
        let config=FeishuConfig{app_id:"cli_formal_transport".into(),app_token:"base_formal_transport".into(),table_id:"tbl_formal_transport".into(),secret_configured:false};
        assert!(!allowed_target(&config),"isolated target restriction must remain intact");
        let content=json!({"title":"formal test","detail":"synthetic","progress":25,"quadrant":"in","today":true,"startDate":null,"plannedDate":null});
        let read=SyncInspection{read_only:true,complete:true,account_name:"synthetic".into(),subject_id:"cli_formal_transport:user_a".into(),
            app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1800000000000,
            conditional_write_verified:false,warnings:vec![],records:vec![SyncRecordTime{record_id:"recFormal".into(),title:"formal test".into(),progress:60,status:"进行中".into(),planned_date:None,
                modified_at:Some(1799900000000),time_source:"platform".into(),shared_fields:content.clone()}]};
        let schema=json!({"code":0,"msg":"ok","data":{"has_more":false,"items":[{"field_name":"任务名称","type":1},{"field_name":"任务详情","type":1},
            {"field_name":"进度","type":2,"property":{"formatter":"0%"}},{"field_name":"任务类别","type":3},{"field_name":"今日必完成","type":7},
            {"field_name":"开始日期","type":5},{"field_name":"计划完成日期","type":5},{"field_name":"状态","type":3},{"field_name":"附件","type":17}]}});
        let (root,server)=sync_inspect::tests::fault_server(vec![(200,Some(schema.to_string())),(200,None)]);
        let missing=prepare_user_sync_update_at(&root,&config,"synthetic-token",&read,"recOther",&content,"进行中");
        assert!(missing.err().unwrap().contains("记录"));
        let prepared=prepare_user_sync_update_at(&root,&config,"synthetic-token",&read,"recFormal",&content,"进行中").unwrap();
        assert!(send_user_sync_update(prepared).is_err());
        let requests=server.join().unwrap();
        assert_eq!(requests.len(),2);assert!(requests[0].starts_with("GET "));
        assert!(requests[1].starts_with("PUT /bitable/v1/apps/base_formal_transport/tables/tbl_formal_transport/records/recFormal "));
        assert!(!requests[1].contains("附件"));assert!(requests[1].contains("\"进度\":0.25"));
    }
    #[test]
    fn target_gate_rejects_other_accounts_and_tables_before_network(){
        let direct=cfg!(feature="feishu-direct-account-acceptance");
        let mut config=FeishuConfig{app_id:if direct{"cli_0000000000000003"}else{crate::write_acceptance::TEMPLATE_V2_APP_ID}.into(),
            app_token:if direct{"Example00000000000000000007"}else{crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN}.into(),
            table_id:if direct{"tbl0000000000001"}else{crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID}.into(),secret_configured:false};
        assert!(allowed_target(&config));
        for key in ["app","base","table"] {
            let mut wrong=config.clone(); match key{"app"=>wrong.app_id.push('x'),"base"=>wrong.app_token.push('x'),_=>wrong.table_id.push('x')};
            assert!(!allowed_target(&wrong));assert!(prepare_sync_update(&wrong,"unused","rec1",&json!({}),"进行中").is_err());
        }
        if direct {config.app_id=crate::write_acceptance::TEMPLATE_V2_APP_ID.into();assert!(!allowed_target(&config));}
        if direct {
            assert!(!allowed_record("seed",false));assert!(!allowed_record("recOther",false));
            for id in crate::direct_account_scope::RECORD_IDS{assert!(allowed_record(id,false));}
        }
    }
    #[test]
    fn payload_preserves_unmapped_fields_and_uses_date_percent_and_labels() {
        let schema: Vec<FieldItem> = serde_json::from_value(json!([
            {"field_name":"任务名称","type":1},{"field_name":"任务详情","type":1},
            {"field_name":"进度","type":2,"property":{"formatter":"0%"}},
            {"field_name":"任务类型","type":3},{"field_name":"今日必完成","type":7},
            {"field_name":"开始日期","type":5},{"field_name":"计划完成日期","type":5},
            {"field_name":"状态","type":20},{"field_name":"附件","type":17}
        ]))
        .unwrap();
        let content = json!({"title":"test","detail":"text","progress":42,"quadrant":"in","today":true,"startDate":null,"plannedDate":"2026-10-20"});
        let p = sync_payload(&schema, &content, "进行中").unwrap();
        assert_eq!(p["fields"]["进度"], json!(0.42));
        assert_eq!(p["fields"]["开始日期"], Value::Null);
        assert!(p["fields"].get("附件").is_none());
        assert!(p["fields"].get("状态").is_none());
        assert_eq!(
            civil_date_from_unix_ms(p["fields"]["计划完成日期"].as_i64().unwrap()),
            "2026-10-20"
        );
    }
}
