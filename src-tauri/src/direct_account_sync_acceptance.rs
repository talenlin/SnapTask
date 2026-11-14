//! Fixed-new-space live acceptance. Not a desktop command or production path.
use super::*;
use serde_json::{json,Value};
use std::{fs::{self,OpenOptions},io::Write,path::PathBuf};
const APP:&str="cli_0000000000000003";
const BASE:&str="Example00000000000000000007";
const TABLE:&str="tbl0000000000001";
fn config()->crate::feishu::FeishuConfig {crate::feishu::FeishuConfig{app_id:APP.into(),app_token:BASE.into(),table_id:TABLE.into(),secret_configured:true}}
fn dir()->PathBuf{PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join("artifacts")}
fn fixture(i:usize)->Value{json!({"title":if i==0{"【同步验收A】进度与撤销-20261020"}else{"【同步验收B】冲突与下载-20261020"},
    "detail":format!("仅新飞书身份隔离测试；验收编号DIRECT-20261020-{}；不是个人真实任务，测试后保留供核对。",if i==0{"A"}else{"B"}),
    "progress":if i==0{25}else{75},"quadrant":if i==0{"iu"}else{"in"},"today":i==0,"startDate":null,"plannedDate":"2026-10-25"})}
fn opt_in(name:&str)->String{
    assert_eq!(std::env::var(name).ok().as_deref(),Some(APP),"explicit fixed-new-app opt-in required");
    crate::feishu::validate_credential_profile("app.snaptask.desktop.directfeishu20261020").expect("isolated credential profile");
    let receipt:Value=serde_json::from_slice(&fs::read(dir().join("feishu-direct-copy-20261020.receipt.json")).expect("copy receipt")).expect("receipt JSON");
    assert_eq!(receipt["appId"],APP);assert_eq!(receipt["copy"]["appToken"],BASE);
    crate::feishu::read_user_access_token(APP).expect("new app isolated authorization")
}
fn checkpoint(name:&str,value:&Value){
    let mut f=OpenOptions::new().write(true).create_new(true).open(dir().join(name)).expect("checkpoint already exists or cannot be created; no resending");
    f.write_all(&serde_json::to_vec_pretty(value).unwrap()).expect("write checkpoint");f.sync_all().expect("flush checkpoint");
}
fn read(token:&str)->crate::feishu::SyncInspection{
    let r=crate::feishu::inspect_sync_with_user_token(&config(),token,content_now().unwrap()).expect("read fixed test table");
    assert!(r.complete&&r.read_only);assert_eq!(r.app_id,APP);assert_eq!(r.app_token,BASE);assert_eq!(r.table_id,TABLE);r
}
fn record_ids()->Vec<String>{(0..2).map(|i|{
    let v:Value=serde_json::from_slice(&fs::read(dir().join(format!("direct-sync-seed-{i}-20261020.receipt.json"))).expect("seed receipt required")).unwrap();
    assert_eq!(v["appId"],APP);assert_eq!(v["appToken"],BASE);assert_eq!(v["tableId"],TABLE);
    v["recordId"].as_str().expect("record ID").to_string()
}).collect()}
fn check_owned(r:&crate::feishu::SyncInspection,ids:&[String]){
    assert_eq!(r.records.len(),2,"unexpected records; stop before mutation");
    for(i,id)in ids.iter().enumerate(){let row=r.records.iter().find(|x|&x.record_id==id).expect("owned record missing");assert_eq!(row.shared_fields["title"],fixture(i)["title"]);assert_eq!(row.shared_fields["detail"],fixture(i)["detail"]);assert!(row.modified_at.is_some(),"platform modification time required");}
}

#[test]
#[ignore="read-only final verification of the two approved records; no seed, update, or DB open"]
fn live_direct_completed_snapshot_readonly(){
    let token=opt_in("PROJECT_TRACK_DIRECT_VERIFY_FINAL");let ids=record_ids();
    let completed:Value=serde_json::from_slice(&fs::read(dir().join("direct-sync-20261020.completed.json")).expect("completed receipt required")).unwrap();
    assert_eq!(completed["appId"],APP);assert_eq!(completed["appToken"],BASE);assert_eq!(completed["tableId"],TABLE);assert_eq!(completed["pendingWrites"],0);
    let r=read(&token);check_owned(&r,&ids);
    for(i,id)in ids.iter().enumerate(){let row=r.records.iter().find(|x|&x.record_id==id).unwrap();assert_eq!(row.shared_fields,fixture(i));assert_eq!(row.status,"进行中");}
    println!("DIRECT_FINAL_READONLY {}",json!({"appId":APP,"appToken":BASE,"tableId":TABLE,"recordCount":2,"progress":[25,75],"cloudMutation":false,"databaseOpened":false}));
}

#[test]
#[ignore="explicit creation of exactly two approved synthetic records in fixed new table"]
fn live_direct_seed_two_tasks_once(){
    let token=opt_in("PROJECT_TRACK_DIRECT_SEED");let mut r=read(&token);
    assert!(r.records.iter().all(|row|(0..2).any(|i|row.shared_fields==fixture(i))),"foreign or changed records; do not seed");
    for i in 0..2{
        let receipt=dir().join(format!("direct-sync-seed-{i}-20261020.receipt.json"));
        if receipt.exists(){continue;}
        assert!(!r.records.iter().any(|row|row.shared_fields==fixture(i)),"unreceipted existing fixture; reconcile manually, never duplicate");
        let desired=fixture(i);
        let payload=crate::feishu::prepare_sync_update(&config(),&token,"seed",&desired,"进行中").expect("schema-supported fields");
        checkpoint(&format!("direct-sync-seed-{i}-20261020.intent.json"),&json!({"appId":APP,"appToken":BASE,"tableId":TABLE,"desired":desired,"state":"intent_before_request"}));
        let response=reqwest::blocking::Client::builder().timeout(Duration::from_secs(20)).redirect(reqwest::redirect::Policy::none()).build().unwrap()
            .post(format!("https://open.feishu.cn/open-apis/bitable/v1/apps/{BASE}/tables/{TABLE}/records")).bearer_auth(&token).json(&payload).send()
            .map_err(|_|"create outcome unknown; intent retained; never resend").expect("one creation attempt");
        let v:Value=response.json().map_err(|_|"create response unknown; never resend").unwrap();
        assert_eq!(v["code"],0,"create rejected; intent retained; no automatic retry");
        let id=v["data"]["record"]["record_id"].as_str().expect("record ID absent; reconcile manually");
        assert!(!id.is_empty()&&id.bytes().all(|b|b.is_ascii_alphanumeric()));
        checkpoint(&format!("direct-sync-seed-{i}-20261020.receipt.json"),&json!({"appId":APP,"appToken":BASE,"tableId":TABLE,"recordId":id,"fixture":desired}));
        r=read(&token);
    }
    let ids=record_ids();check_owned(&r,&ids);
    for(i,id)in ids.iter().enumerate(){let row=r.records.iter().find(|x|&x.record_id==id).unwrap();assert_eq!(row.shared_fields,fixture(i));assert_eq!(row.status,"进行中");}
    println!("DIRECT_SEED_VERIFIED {}",json!({"appId":APP,"appToken":BASE,"tableId":TABLE,"recordIds":ids,"recordCount":2,"progress":[25,75],"formalDataChanged":false}));
}

fn task(db:&LocalWorkspace,id:&str)->LocalTask{get_task(&db.connection,id).unwrap().unwrap()}
fn edit(db:&mut LocalWorkspace,id:&str,progress:u8,date:&str){
    let t=task(db,id);db.save(SaveTask{id:Some(t.id),expected_revision:Some(t.revision),title:t.title,detail:t.detail,progress:progress.into(),quadrant:t.quadrant,today:t.today,start_date:t.start_date,planned_date:Some(date.into())}).unwrap();
}
fn input(db:&LocalWorkspace,r:&crate::feishu::SyncInspection,local:&str,cloud:&str)->DownloadInput{
    let t=task(db,local);let c=r.records.iter().find(|x|x.record_id==cloud).unwrap();
    DownloadInput{local_id:local.into(),cloud_id:cloud.into(),subject_id:r.subject_id.clone(),expected_local_revision:t.revision,expected_local_modified_at:t.content_modified_at,expected_cloud_modified_at:c.modified_at,expected_cloud_fields:c.shared_fields.clone()}
}
fn receive(db:&mut LocalWorkspace,r:&crate::feishu::SyncInspection,local:&str,cloud:&str){let i=input(db,r,local,cloud);download(&mut db.connection,r,&i).unwrap();}
fn classification(db:&LocalWorkspace,r:&crate::feishu::SyncInspection,id:&str)->String{
    linked_comparisons(&db.snapshot().unwrap().tasks,r,&existing_links(&db.connection,&scope_from_inspection(r)).unwrap()).unwrap().into_iter().find(|x|x.local_id==id).unwrap().status
}
fn send_operation(db:&mut LocalWorkspace,token:&str,before:&crate::feishu::SyncInspection,op:WorkOperation)->(crate::feishu::SyncInspection,UploadSummary){
    let ids=record_ids();assert!(ids.contains(&op.record_id),"only the two approved IDs are writable");
    let payload=match crate::feishu::prepare_sync_update(&config(),token,&op.record_id,&op.content,&op.status){Ok(p)=>p,Err(e)=>{cancel_unsent_upload(&db.connection,&op.id).unwrap();panic!("not sent; preparation rejected: {e}")}};
    let fresh=read(token);check_owned(&fresh,&ids);
    let old=before.records.iter().find(|r|r.record_id==op.record_id).unwrap();let now=fresh.records.iter().find(|r|r.record_id==op.record_id).unwrap();
    if before.subject_id!=fresh.subject_id||old.modified_at!=now.modified_at||old.shared_fields!=now.shared_fields||old.status!=now.status{cancel_unsent_upload(&db.connection,&op.id).unwrap();panic!("changed during preflight; no mutation sent");}
    let outcome=crate::feishu::send_sync_update(&config(),token,&op.record_id,&payload);
    let after=read(token);check_owned(&after,&ids);
    let result=reconcile_upload(&mut db.connection,&op.id,Some(&after)).unwrap();
    assert_eq!(result.state,"completed","readback not verified; operation remains locked, never resend; transport_ok={}",outcome.is_ok());
    (after,result)
}
fn upload_local(db:&mut LocalWorkspace,token:&str,local:&str,cloud:&str)->(crate::feishu::SyncInspection,UploadSummary){
    let before=read(token);check_owned(&before,&record_ids());let i=input(db,&before,local,cloud);let op=begin_upload(&mut db.connection,&before,&i).unwrap();send_operation(db,token,&before,op)
}
fn external_edit(token:&str,cloud:&str,desired:&Value,label:&str)->crate::feishu::SyncInspection{
    assert!(label.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'_'));
    let ids=record_ids();assert!(ids.iter().any(|x|x==cloud));let before=read(token);check_owned(&before,&ids);
    let old=before.records.iter().find(|r|r.record_id==cloud).unwrap();
    let payload=crate::feishu::prepare_sync_update(&config(),token,cloud,desired,"进行中").unwrap();
    checkpoint(&format!("direct-sync-{label}-20261020.intent.json"),&json!({"appId":APP,"appToken":BASE,"tableId":TABLE,"recordId":cloud,"before":old.shared_fields,"modifiedAt":old.modified_at,"desired":desired}));
    let fresh=read(token);check_owned(&fresh,&ids);let now=fresh.records.iter().find(|r|r.record_id==cloud).unwrap();assert_eq!(old.shared_fields,now.shared_fields);assert_eq!(old.modified_at,now.modified_at);
    let outcome=crate::feishu::send_sync_update(&config(),token,cloud,&payload);let after=read(token);check_owned(&after,&ids);
    let checked=after.records.iter().find(|r|r.record_id==cloud).unwrap();assert_eq!(&checked.shared_fields,desired,"external test edit unverified, intent retained, never resend");assert_eq!(checked.status,"进行中");
    checkpoint(&format!("direct-sync-{label}-20261020.receipt.json"),&json!({"recordId":cloud,"verified":true,"transportOk":outcome.is_ok()}));after
}

#[test]
#[ignore="approved two-record real-cloud sync/core acceptance; independent persistent DB, no native UI claim"]
fn live_direct_sync_conflict_undo_and_batch(){
    let token=opt_in("PROJECT_TRACK_DIRECT_SYNC");let ids=record_ids();let mut r=read(&token);check_owned(&r,&ids);
    for(i,id)in ids.iter().enumerate(){assert_eq!(r.records.iter().find(|x|&x.record_id==id).unwrap().shared_fields,fixture(i));}
    let path=dir().join("feishu-direct-sync-20261020.sqlite3");
    OpenOptions::new().write(true).create_new(true).open(&path).expect("local test DB already exists; inspect prior run, never reset or replay");
    let mut db=LocalWorkspace::open(&path).unwrap();let mut locals=Vec::new();
    for(i,id)in ids.iter().enumerate(){
        let c:Content=serde_json::from_value(fixture(i)).unwrap();
        let saved=db.save(SaveTask{id:None,expected_revision:None,title:c.title,detail:c.detail,progress:c.progress.into(),quadrant:c.quadrant,today:c.today,start_date:c.start_date,planned_date:c.planned_date}).unwrap();
        let t=saved.tasks.iter().find(|t|t.title==fixture(i)["title"].as_str().unwrap()).unwrap().clone();locals.push(t.id.clone());
        let cloud=r.records.iter().find(|x|&x.record_id==id).unwrap();
        save_pairing(&mut db.connection,&t,cloud,&r,&SavePairingInput{local_id:t.id.clone(),cloud_id:id.clone(),expected_local_revision:t.revision,expected_local_modified_at:t.content_modified_at,expected_cloud_modified_at:cloud.modified_at}).unwrap();
        receive(&mut db,&r,&t.id,id);
    }
    println!("DIRECT_SYNC_STAGE baseline-established-two-records");
    // Upload all seven shared fields, then restore the verified remote preimage.
    edit(&mut db,&locals[0],60,"2026-10-26");let(after,uploaded)=upload_local(&mut db,&token,&locals[0],&ids[0]);r=after;
    assert_eq!(r.records.iter().find(|x|x.record_id==ids[0]).unwrap().planned_date.as_deref(),Some("2026-10-26"));
    assert_eq!(classification(&db,&r,&locals[0]),"equal");
    let op=begin_upload_restore(&mut db.connection,&r,&uploaded.id).unwrap();let(after,_)=send_operation(&mut db,&token,&r,op);r=after;
    db.undo().unwrap();assert_eq!(serde_json::to_value(Content::from(&task(&db,&locals[0]))).unwrap(),fixture(0));receive(&mut db,&r,&locals[0],&ids[0]);
    println!("DIRECT_SYNC_STAGE upload-date-and-remote-restore-passed");
    // Real formula regression: completed -> in-progress when progress decreases.
    edit(&mut db,&locals[0],100,"2026-10-25");let(after,_)=upload_local(&mut db,&token,&locals[0],&ids[0]);r=after;assert_eq!(r.records.iter().find(|x|x.record_id==ids[0]).unwrap().status,"已完成");
    edit(&mut db,&locals[0],71,"2026-10-25");let(after,_)=upload_local(&mut db,&token,&locals[0],&ids[0]);r=after;assert_eq!(r.records.iter().find(|x|x.record_id==ids[0]).unwrap().status,"进行中");
    edit(&mut db,&locals[0],25,"2026-10-25");let(after,_)=upload_local(&mut db,&token,&locals[0],&ids[0]);r=after;
    println!("DIRECT_SYNC_STAGE completed-to-in-progress-formula-passed");
    // External cloud change -> download -> local undo must not trigger an upload.
    let mut changed=fixture(1);changed["progress"]=json!(40);r=external_edit(&token,&ids[1],&changed,"download40");
    receive(&mut db,&r,&locals[1],&ids[1]);assert_eq!(task(&db,&locals[1]).progress,40);
    db.undo().unwrap();assert_eq!(task(&db,&locals[1]).progress,75);r=read(&token);assert_eq!(r.records.iter().find(|x|x.record_id==ids[1]).unwrap().progress,40);
    r=external_edit(&token,&ids[1],&fixture(1),"download_restore75");receive(&mut db,&r,&locals[1],&ids[1]);
    println!("DIRECT_SYNC_STAGE download-and-local-undo-no-cloud-write-passed");
    // Both sides changed: stale consent rejected before mutation, choose cloud explicitly.
    let stale=input(&db,&r,&locals[0],&ids[0]);edit(&mut db,&locals[0],35,"2026-10-25");
    let mut changed=fixture(0);changed["progress"]=json!(45);r=external_edit(&token,&ids[0],&changed,"conflict45");
    assert_eq!(classification(&db,&r,&locals[0]),"conflict");
    let current=task(&db,&locals[0]);let stale=DownloadInput{expected_local_revision:current.revision,expected_local_modified_at:current.content_modified_at,..stale};
    assert!(begin_upload(&mut db.connection,&r,&stale).is_err(),"stale cloud confirmation must not create/send a new write");
    receive(&mut db,&r,&locals[0],&ids[0]);assert_eq!(task(&db,&locals[0]).progress,45);
    r=external_edit(&token,&ids[0],&fixture(0),"conflict_restore25");receive(&mut db,&r,&locals[0],&ids[0]);
    println!("DIRECT_SYNC_STAGE conflict-stale-consent-and-explicit-resolution-passed");
    // Production batch core + real cloud reads + physical DB reopen (not a GUI test).
    edit(&mut db,&locals[0],10,"2026-10-25");edit(&mut db,&locals[1],20,"2026-10-25");r=read(&token);
    let entries:Vec<Value>=(0..2).map(|i|json!({"direction":"cloud","input":input(&db,&r,&locals[i],&ids[i])})).collect();
    let plan:BatchInput=serde_json::from_value(json!({"entries":entries})).unwrap();let job=prepare_batch(&mut db.connection,&r,plan).unwrap();
    let before_history:i64=db.connection.query_row("SELECT COUNT(*) FROM local_history",[],|r|r.get(0)).unwrap();
    let first=claim_batch(&mut db.connection,&job.id,&r.subject_id).unwrap().unwrap();download(&mut db.connection,&r,&first.input).unwrap();
    control_batch(&mut db.connection,&job.id,"stop").unwrap();let stopped=finish_batch(&mut db.connection,&job.id,"completed").unwrap();assert_eq!((stopped.state.as_str(),stopped.completed,stopped.queued),("stopped",1,1));
    drop(db);let mut db=LocalWorkspace::open(&path).unwrap();assert!(claim_batch(&mut db.connection,&job.id,&r.subject_id).is_err());
    control_batch(&mut db.connection,&job.id,"resume").unwrap();r=read(&token);let next=claim_batch(&mut db.connection,&job.id,&r.subject_id).unwrap().unwrap();assert_ne!(next.input.local_id,first.input.local_id);
    download(&mut db.connection,&r,&next.input).unwrap();assert_eq!(finish_batch(&mut db.connection,&job.id,"completed").unwrap().completed,2);
    let after_history:i64=db.connection.query_row("SELECT COUNT(*) FROM local_history",[],|r|r.get(0)).unwrap();assert_eq!(after_history-before_history,2);
    drop(db);let mut db=LocalWorkspace::open(&path).unwrap();assert!(claim_batch(&mut db.connection,&job.id,&r.subject_id).unwrap().is_none());assert!(control_batch(&mut db.connection,&job.id,"resume").is_err());
    r=read(&token);check_owned(&r,&ids);for(i,id)in ids.iter().enumerate(){assert_eq!(r.records.iter().find(|x|&x.record_id==id).unwrap().shared_fields,fixture(i));assert_eq!(serde_json::to_value(Content::from(&task(&db,&locals[i]))).unwrap(),fixture(i));}
    let pending:i64=db.connection.query_row("SELECT COUNT(*) FROM local_sync_operations WHERE state IN ('pending','conflict')",[],|r|r.get(0)).unwrap();assert_eq!(pending,0);
    checkpoint("direct-sync-20261020.completed.json",&json!({"appId":APP,"appToken":BASE,"tableId":TABLE,"recordIds":ids,"localIds":locals,"database":path,"batchId":job.id,"finalProgress":[25,75],"pendingWrites":0,"nativeUiVerified":false}));
    println!("DIRECT_SYNC_VERIFIED {}",json!({"recordCount":2,"finalProgress":[25,75],"uploadAndRemoteRestore":true,"downloadAndLocalUndo":true,"formulaRegression":true,"conflictGuard":true,"batchStopReopenResumeNoReplay":true,"pendingWrites":pending,"nativeUiVerified":false,"formalDataChanged":false}));
}
