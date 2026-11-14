//! Explicit one-copy acceptance, not registered as a desktop command.
//! A persisted intent without a receipt blocks resending after uncertain outcomes.
use super::*;
use std::{fs::{self,OpenOptions},io::Write,path::Path};
const SOURCE: &str="Example00000000000000000005";
const NAME: &str="任务工作台-飞书身份空白验收-20261020";

fn should_send_copy(intent_exists:bool,receipt_exists:bool)->Result<bool,&'static str>{
    if receipt_exists {Ok(false)}
    else if intent_exists {Err("copy may already have been sent; manual reconciliation required, never resend")}
    else {Ok(true)}
}

#[test]
fn copy_checkpoint_blocks_unknown_outcomes_and_reuses_receipts(){
    assert_eq!(should_send_copy(false,false),Ok(true));
    assert!(should_send_copy(true,false).is_err());
    assert_eq!(should_send_copy(true,true),Ok(false));
    assert_eq!(should_send_copy(false,true),Ok(false));
}

fn write_new_json(path:&Path,value:&Value)->Result<(),String>{
    let mut file=OpenOptions::new().write(true).create_new(true).open(path).map_err(|_|"检查点已存在或无法创建，已停止，未覆盖")?;
    file.write_all(&serde_json::to_vec_pretty(value).map_err(|_|"检查点编码失败")?).map_err(|_|"检查点写入失败，停止操作")?;
    file.sync_all().map_err(|_|"检查点落盘失败，停止操作".to_string())
}

#[test]
#[ignore="explicit fixed-app empty-template copy exactly once; no task record writes or formal DB"]
fn live_direct_account_create_empty_copy_once(){
    assert_eq!(std::env::var("PROJECT_TRACK_DIRECT_COPY_ONCE").ok().as_deref(),Some(APP_ID),"fixed-app copy opt-in required");
    crate::feishu::validate_credential_profile("app.snaptask.desktop.directfeishu20261020").expect("independent credential profile");
    assert_eq!(crate::bundled_feishu_template_app_token().expect("bundled template"),SOURCE);
    assert!(crate::feishu::user_authorized_for_app(APP_ID));
    let token=crate::feishu::read_user_access_token(APP_ID).expect("new app isolated user authorization");
    let dir=Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("workspace").join("artifacts");
    fs::create_dir_all(&dir).expect("acceptance evidence directory");
    let intent=dir.join("feishu-direct-copy-20261020.intent.json");
    let receipt=dir.join("feishu-direct-copy-20261020.receipt.json");
    let copied:crate::feishu::FeishuTemplateCopyResult;
    let sent;
    let send=should_send_copy(intent.exists(),receipt.exists()).expect("existing unknown copy cannot be resent");
    if !send{
        let saved:Value=serde_json::from_slice(&fs::read(&receipt).expect("read receipt")).expect("valid receipt");
        assert_eq!(saved["appId"],APP_ID);assert_eq!(saved["source"],SOURCE);assert_eq!(saved["name"],NAME);
        copied=serde_json::from_value(saved["copy"].clone()).expect("receipt contains copy metadata");
        sent=false;
    }else{
        crate::feishu::ensure_template_tables_empty(SOURCE,&token).expect("all source tables must be empty before copy");
        write_new_json(&intent,&json!({"appId":APP_ID,"source":SOURCE,"name":NAME,"state":"intent_before_request","createdAt":crate::storage::unix_now()})).expect("persist intent before network mutation");
        copied=crate::feishu::copy_task_workspace_template(SOURCE,&token,NAME,None).expect("one copy request failed or outcome unknown; intent retained, no automatic resend");
        write_new_json(&receipt,&json!({"appId":APP_ID,"source":SOURCE,"name":NAME,"state":"copy_receipt","copy":copied})).expect("copy accepted but receipt unavailable; retain intent and reconcile manually");
        sent=true;
    }
    assert_ne!(copied.app_token,SOURCE,"never activate the source");
    let table={
        let mut attempt=0;
        loop{
            match crate::feishu::discover_task_table(&copied.app_token,&token){
                Ok(table)=>break table,
                Err(message) if message.contains("1254036") && attempt<3=>{attempt+=1;std::thread::sleep(std::time::Duration::from_secs(attempt*2));}
                Err(message)=>panic!("copy receipt saved; readiness read failed without repeating copy: {message}"),
            }
        }
    };
    crate::feishu::ensure_template_tables_empty(&copied.app_token,&token).expect("every copied table is empty");
    let config=crate::feishu::FeishuConfig{app_id:APP_ID.into(),app_token:copied.app_token.clone(),table_id:table.table_id.clone(),secret_configured:true};
    let snapshot=crate::feishu::inspect_sync_with_user_token(&config,&token,crate::storage::unix_now()*1000).expect("verify account and read copied records");
    assert!(snapshot.complete && snapshot.read_only && snapshot.records.is_empty());
    let status=crate::feishu::inspect_workspace_as_user(&copied.app_token,&table.table_id,&token).expect("inspect copied structure");
    assert!(status.missing_required_fields.is_empty() && status.inspection_warnings.is_empty());
    assert!(status.status_is_formula);
    for(name,kind)in[("任务总览","grid"),("今日任务","grid"),("四象限看板","kanban"),("今日必做看板","kanban"),("进展甘特图","gantt")]{
        assert!(status.views.iter().any(|v|v.name==name && v.kind==kind),"missing copied view {name}");
    }
    assert!(status.dashboards.iter().any(|d|d.name=="个人任务仪表盘"));
    println!("DIRECT_ACCOUNT_COPY {}",json!({"appId":APP_ID,"workspaceName":NAME,"appToken":copied.app_token,"tableId":table.table_id,
        "workspaceUrl":format!("https://my.feishu.cn/base/{}?table={}",copied.app_token,table.table_id),"accountVerified":!snapshot.subject_id.is_empty(),
        "recordCount":0,"fieldCount":table.fields.len(),"views":status.views,"dashboardCount":status.dashboards.len(),"statusIsFormula":true,
        "copyRequestSentThisRun":sent,"recordMutation":false,"formalDatabaseOpened":false}));
}
