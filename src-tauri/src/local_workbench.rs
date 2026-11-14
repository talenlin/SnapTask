//! Offline workspace core, with explicit capability-gated cloud acceptance commands.
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Mutex, time::Duration};
use tauri::{AppHandle, Manager, State, WebviewUrl, WebviewWindowBuilder};
#[path = "local_workbench_backup.rs"]
mod backup;
#[path = "local_workbench_ime.rs"]
pub(crate) mod ime;
pub use backup::{ExportResult, RestorePreview, RestoreResult};
#[path = "local_workbench_moves.rs"]
mod moves;
pub use moves::MoveTask;
#[path = "local_workbench_csv.rs"]
mod csv_export;
pub use csv_export::CsvExport;
#[path = "local_workbench_sync.rs"]
mod sync_preview;
pub use sync_preview::{BatchInput, BatchSummary, PairingPreview, SavePairingResult, SyncDemo, UploadSummary};
pub(crate) use sync_preview::WorkOperation;
#[cfg(feature = "local-sync-fault-ui-acceptance")]
#[path = "local_workbench_fault_lab.rs"]
pub(crate) mod fault_lab;
#[cfg(all(test, feature = "feishu-direct-sync-ui-acceptance"))]
#[path = "local_workbench_transport_tests.rs"]
mod transport_tests;
#[cfg(feature = "local-workbench-acceptance")]
#[path = "local_workbench_auth.rs"]
mod isolated_auth;
#[cfg(feature = "local-workbench-acceptance")]
pub use sync_preview::{
    DownloadInput, SavePairingInput,
};

pub struct LocalWorkspaceState {
    pub(crate) cloud_access: crate::local_cloud_access::CloudAccess,
    feishu_context: Mutex<FeishuContext>,
    path: PathBuf,
    database: Mutex<Option<LocalWorkspace>>,
    pending_restore: Mutex<Option<backup::PreparedRestore>>,
    feishu_readonly_enabled: bool,
    feishu_pairing_save_enabled: bool,
}
#[derive(Default)]
struct FeishuContext { version: u64, changing: usize, batch_run: Option<BatchPermit> }
struct BatchPermit { batch_id: String, subject_id: String, run_id: String }
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalBatchInput {
    pub context_version: u64,
    pub confirm_batch: bool,
    pub batch: BatchInput,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalBatchAction {
    pub context_version: u64,
    pub batch_id: String,
    pub run_id: String,
}
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct FormalBatchRun {
    #[serde(flatten)]
    batch: BatchSummary,
    run_id: String,
}
pub(crate) enum FormalBatchStep { Settled(BatchSummary), Upload(WorkOperation) }
#[derive(Serialize)]
#[serde(rename_all="camelCase")]
pub struct FormalPairingPreview {
    #[serde(flatten)]
    preview: PairingPreview,
    context_version: u64,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalPairingInput {
    pub context_version: u64,
    pub subject_id: String,
    pub pairing: sync_preview::SavePairingInput,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalDownloadInput {
    pub context_version: u64,
    pub download: sync_preview::DownloadInput,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalImportInput {
    pub context_version: u64,
    pub import: sync_preview::ImportInput,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalBatchImportInput {
    pub context_version: u64,
    pub confirm_import: bool,
    pub imports: Vec<sync_preview::ImportInput>,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalUploadInput {
    pub context_version: u64,
    pub confirm_overwrite: bool,
    pub upload: sync_preview::DownloadInput,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct FormalRestoreInput {
    pub context_version: u64,
    pub operation_id: String,
    pub confirm_restore: bool,
    pub expected_restore_content: serde_json::Value,
    pub expected_restore_status: String,
}

#[cfg(feature = "feishu-direct-sync-ui-acceptance")]
fn direct_acceptance_database_path(exe: &std::path::Path) -> Result<PathBuf, String> {
    if !exe.is_absolute() { return Err("隔离程序路径必须为绝对路径".into()); }
    let folder = exe.parent().ok_or("无法确定隔离程序目录")?;
    Ok(folder.join("direct-sync-acceptance-20261020").join("local-workspace.sqlite3"))
}

impl LocalWorkspaceState {
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    fn prepared_direct_from_exe(exe: &std::path::Path) -> Result<Self, String> {
        let path = direct_acceptance_database_path(exe)?;
        crate::startup_diagnostics::database_path(&path);
        crate::startup_diagnostics::stage("prepared_database");
        Self::prepared_direct_acceptance(path)
    }
    #[cfg(feature="feishu-direct-sync-ui-acceptance")]
    fn prepared_direct_acceptance(path:PathBuf)->Result<Self,String>{
        if !path.is_file(){return Err("请先准备独立新飞书测试库；不会创建空库或导入其他数据".into());}
        let database=LocalWorkspace::open(&path)?;
        let tasks=database.snapshot()?.tasks;
        if tasks.len()!=2 || !tasks.iter().all(|t|t.title.starts_with("【同步验收")&&t.detail.contains("DIRECT-20261020-")){
            return Err("独立测试库必须只包含两条已批准的同步验收任务".into());
        }
        let connections:i64=database.connection.query_row("SELECT count(*) FROM local_sync_connections WHERE platform='feishu' AND space_id=?1 AND table_id=?2 AND subject_id LIKE ?3",params![crate::direct_account_scope::APP_TOKEN,crate::direct_account_scope::TABLE_ID,format!("{}:%",crate::direct_account_scope::APP_ID)],|r|r.get(0)).map_err(error)?;
        let all:i64=database.connection.query_row("SELECT count(*) FROM local_sync_connections",[],|r|r.get(0)).map_err(error)?;
        let confirmed_restore = if all == 0 {
            let marker_table: i64 = database.connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='local_direct_restore_marker'", [], |r| r.get(0)).map_err(error)?;
            if marker_table == 1 {
                let restored_revision: Option<i64> = database.connection.query_row(
                    "SELECT restored_revision FROM local_direct_restore_marker WHERE identifier=?1",
                    [crate::direct_account_scope::SYNC_IDENTIFIER], |r| r.get(0)).optional().map_err(error)?;
                let current_revision: i64 = database.connection.query_row("SELECT revision FROM local_meta WHERE id=1", [], |r| r.get(0)).map_err(error)?;
                restored_revision.is_some_and(|revision| revision > 0 && revision <= current_revision)
            } else { false }
        } else { false };
        if !((connections==1&&all==1)||confirmed_restore){return Err("测试库连接范围不匹配，拒绝启动".into());}
        Ok(Self{cloud_access:crate::local_cloud_access::CloudAccess::new(path.with_file_name("local-cloud-access.sqlite3")),feishu_context:Mutex::new(FeishuContext::default()),path,database:Mutex::new(Some(LocalWorkspace{demo_mode:true,..database})),pending_restore:Mutex::new(None),feishu_readonly_enabled:true,feishu_pairing_save_enabled:true})
    }
    #[cfg(any(test, feature = "local-workbench-acceptance"))]
    fn acceptance(
        path: PathBuf,
        feishu_readonly_enabled: bool,
        feishu_pairing_save_enabled: bool,
    ) -> Result<Self, String> {
        let mut database = LocalWorkspace::open(&path)?;
        database.seed_acceptance_examples()?;
        database.demo_mode = true;
        Ok(Self {
            feishu_context: Mutex::new(FeishuContext::default()),
            cloud_access: crate::local_cloud_access::CloudAccess::new(path.with_file_name("local-cloud-access.sqlite3")),
            path,
            database: Mutex::new(Some(database)),
            pending_restore: Mutex::new(None),
            feishu_readonly_enabled,
            feishu_pairing_save_enabled,
        })
    }
    pub fn new(path: PathBuf) -> Self {
        Self {
            feishu_context: Mutex::new(FeishuContext::default()),
            cloud_access: crate::local_cloud_access::CloudAccess::new(path.with_file_name("local-cloud-access.sqlite3")),
            path,
            database: Mutex::new(None),
            pending_restore: Mutex::new(None),
            feishu_readonly_enabled: true,
            feishu_pairing_save_enabled: false,
        }
    }
    fn verify_feishu_inspection(
        expected: &crate::feishu::FeishuConfig,
        inspection: &crate::feishu::SyncInspection,
    ) -> Result<(), String> {
        if !inspection.read_only || !inspection.complete {
            return Err("飞书读取不完整，不能生成同步预览".into());
        }
        if expected.app_id.is_empty() || expected.app_token.is_empty() || expected.table_id.is_empty()
            || inspection.app_id != expected.app_id || inspection.app_token != expected.app_token
            || inspection.table_id != expected.table_id
        {
            return Err("飞书目标与当前连接不一致，请重新检查".into());
        }
        // The authenticated adapter constructs this app-scoped subject from user_info.open_id.
        // A display name or browser account is not identity evidence.
        let prefix = format!("{}:", expected.app_id);
        if !inspection.subject_id.strip_prefix(&prefix).is_some_and(|id| !id.trim().is_empty()) {
            return Err("未能核实当前应用下的飞书用户标识".into());
        }
        Ok(())
    }
    pub(crate) fn preview_feishu_connection(&self, expected: &crate::feishu::FeishuConfig, inspection: crate::feishu::SyncInspection) -> Result<PairingPreview,String> {
        Self::verify_feishu_inspection(expected,&inspection)?;
        self.with(|db| sync_preview::pairing_preview(&db.connection,&db.snapshot()?.tasks,inspection))
    }
    pub(crate) fn begin_feishu_change(&self) -> Result<(),String> {
        let mut context=self.feishu_context.lock().map_err(|_|"飞书连接状态繁忙")?;
        context.version=context.version.checked_add(1).ok_or("飞书连接状态已失效，请重启应用")?;
        context.batch_run=None;
        context.changing+=1;
        Ok(())
    }
    pub(crate) fn end_feishu_change(&self) {
        if let Ok(mut context)=self.feishu_context.lock() {context.changing=context.changing.saturating_sub(1);}
    }
    pub(crate) fn feishu_context_version(&self) -> Result<u64,String> {
        let context=self.feishu_context.lock().map_err(|_|"飞书连接状态繁忙")?;
        if context.changing>0 {return Err("飞书连接或授权正在变化，请完成后重新检查".into());}
        Ok(context.version)
    }
    pub(crate) fn with_feishu_context<T>(&self, version:u64, action:impl FnOnce()->Result<T,String>) -> Result<T,String> {
        // Local commits keep this short. Upload dispatch deliberately fences one
        // bounded HTTP send; credential lookup, schema preparation and readback stay outside.
        let context=self.feishu_context.lock().map_err(|_|"飞书连接状态繁忙")?;
        if context.changing>0 || context.version!=version {return Err("飞书连接或授权已变化，旧确认已失效，请重新检查".into());}
        action()
    }
    pub(crate) fn formal_feishu_preview(&self, expected:&crate::feishu::FeishuConfig, inspection:crate::feishu::SyncInspection, version:u64) -> Result<FormalPairingPreview,String> {
        self.with_feishu_context(version,||Ok(FormalPairingPreview{preview:self.preview_feishu_connection(expected,inspection)?,context_version:version}))
    }
    pub(crate) fn save_feishu_pairing(&self, expected:&crate::feishu::FeishuConfig, inspection:crate::feishu::SyncInspection, input:&FormalPairingInput) -> Result<SavePairingResult,String> {
        self.with_feishu_context(input.context_version,|| {
            Self::verify_feishu_inspection(expected,&inspection)?;
            if inspection.subject_id!=input.subject_id {return Err("飞书账号与预览不一致，旧确认已失效".into());}
            let cloud=inspection.records.iter().find(|r|r.record_id==input.pairing.cloud_id).ok_or("所选飞书记录不存在或不可见")?;
            if cloud.modified_at.is_none() {return Err("飞书记录版本未知，请重新检查后关联".into());}
            self.with(|db| {
                let local=db.snapshot()?.tasks.into_iter().find(|t|t.id==input.pairing.local_id).ok_or("所选本地任务不存在")?;
                sync_preview::save_pairing(&mut db.connection,&local,cloud,&inspection,&input.pairing)
            })
        })
    }
    pub(crate) fn download_feishu_task(&self, expected:&crate::feishu::FeishuConfig, inspection:crate::feishu::SyncInspection, input:&FormalDownloadInput) -> Result<Snapshot,String> {
        self.with_feishu_context(input.context_version,|| {
            Self::verify_feishu_inspection(expected,&inspection)?;
            self.with(|db| {
                sync_preview::download(&mut db.connection,&inspection,&input.download)?;
                let mut snapshot=db.snapshot()?;
                snapshot.feishu_readonly_enabled=self.feishu_readonly_enabled;
                snapshot.feishu_pairing_save_enabled=self.feishu_pairing_save_enabled;
                Ok(snapshot)
            })
        })
    }
    pub(crate) fn import_feishu_task(&self, expected:&crate::feishu::FeishuConfig, inspection:crate::feishu::SyncInspection, input:&FormalImportInput) -> Result<Snapshot,String> {
        self.with_feishu_context(input.context_version,|| {
            Self::verify_feishu_inspection(expected,&inspection)?;
            self.with(|db| {
                sync_preview::import_cloud_task(&mut db.connection,&inspection,&input.import)?;
                let mut snapshot=db.snapshot()?;
                snapshot.feishu_readonly_enabled=self.feishu_readonly_enabled;
                snapshot.feishu_pairing_save_enabled=self.feishu_pairing_save_enabled;
                Ok(snapshot)
            })
        })
    }
    pub(crate) fn import_feishu_tasks(&self, expected:&crate::feishu::FeishuConfig, inspection:crate::feishu::SyncInspection, input:&FormalBatchImportInput) -> Result<Snapshot,String> {
        if !input.confirm_import {return Err("请确认将所选飞书任务新增到本地".into());}
        self.with_feishu_context(input.context_version,|| {
            Self::verify_feishu_inspection(expected,&inspection)?;
            self.with(|db| {
                sync_preview::import_cloud_tasks(&mut db.connection,&inspection,&input.imports)?;
                let mut snapshot=db.snapshot()?;
                snapshot.feishu_readonly_enabled=self.feishu_readonly_enabled;
                snapshot.feishu_pairing_save_enabled=self.feishu_pairing_save_enabled;
                Ok(snapshot)
            })
        })
    }
    pub(crate) fn begin_feishu_upload(&self, expected:&crate::feishu::FeishuConfig, inspection:&crate::feishu::SyncInspection, input:&FormalUploadInput) -> Result<WorkOperation,String> {
        if !input.confirm_overwrite {return Err("请明确确认覆盖飞书共有字段".into());}
        self.with_feishu_context(input.context_version,|| {
            Self::verify_feishu_inspection(expected,inspection)?;
            self.with(|db|sync_preview::begin_upload(&mut db.connection,&inspection,&input.upload))
        })
    }
    pub(crate) fn cancel_feishu_unsent(&self,id:&str)->Result<(),String> {
        self.with(|db|sync_preview::cancel_unsent_upload(&db.connection,id))
    }
    pub(crate) fn begin_feishu_restore(&self,expected:&crate::feishu::FeishuConfig,inspection:&crate::feishu::SyncInspection,input:&FormalRestoreInput)->Result<WorkOperation,String> {
        if !input.confirm_restore {return Err("请预览并明确确认恢复上传前内容".into());}
        self.with_feishu_context(input.context_version,|| {
            Self::verify_feishu_inspection(expected,inspection)?;
            self.with(|db| {
                sync_preview::verify_upload_scope(&db.connection,&input.operation_id,inspection)?;
                sync_preview::verify_restore_preview(&db.connection,&input.operation_id,&input.expected_restore_content,&input.expected_restore_status)?;
                sync_preview::begin_upload_restore(&mut db.connection,inspection,&input.operation_id)
            })
        })
    }
    // Only called with the newly created intent, never to replay an operation ID.
    // The bounded single send is fenced against authorization changes; readback is outside.
    pub(crate) fn dispatch_feishu_upload(&self,version:u64,id:&str,send:impl FnOnce()->Result<(),String>)->Result<(),String> {
        let dispatched=self.with_feishu_context(version,|| {
            self.with(|db|sync_preview::verify_unsent_upload(&db.connection,id))?;
            Ok(send())
        });
        match dispatched {
            Ok(_send_result)=>Ok(()), // A response error is not proof of non-delivery.
            Err(reason)=>{self.cancel_feishu_unsent(id)?;Err(format!("尚未发出写入：{reason}"))}
        }
    }
    pub(crate) fn finish_feishu_upload(&self,id:&str,inspection:Option<&crate::feishu::SyncInspection>)->Result<UploadSummary,String> {
        self.with(|db|sync_preview::reconcile_upload(&mut db.connection,id,inspection))
    }
    pub(crate) fn reconcile_feishu_upload(&self,expected:&crate::feishu::FeishuConfig,inspection:crate::feishu::SyncInspection,version:u64,id:&str)->Result<UploadSummary,String> {
        self.with_feishu_context(version,|| {
            Self::verify_feishu_inspection(expected,&inspection)?;
            self.with(|db|{
                sync_preview::verify_upload_scope(&db.connection,id,&inspection)?;
                sync_preview::reconcile_upload(&mut db.connection,id,Some(&inspection))
            })
        })
    }
    fn with_batch_context<T>(&self,version:u64,action:impl FnOnce(&mut FeishuContext)->Result<T,String>)->Result<T,String> {
        let mut context=self.feishu_context.lock().map_err(|_|"飞书连接状态繁忙")?;
        if context.changing>0||context.version!=version {return Err("飞书连接或授权已变化，请重新确认批次".into());}
        action(&mut context)
    }
    fn arm_batch(&self,context:&mut FeishuContext,subject:&str,batch:BatchSummary)->Result<FormalBatchRun,String> {
        let run_id=self.with(|db|db.connection.query_row("SELECT lower(hex(randomblob(16)))",[],|r|r.get::<_,String>(0)).map_err(error))?;
        context.batch_run=Some(BatchPermit{batch_id:batch.id.clone(),subject_id:subject.into(),run_id:run_id.clone()});
        Ok(FormalBatchRun{batch,run_id})
    }
    pub(crate) fn prepare_feishu_batch(&self,config:&crate::feishu::FeishuConfig,read:&crate::feishu::SyncInspection,input:&FormalBatchInput)->Result<FormalBatchRun,String> {
        if !input.confirm_batch {return Err("请先预览方向并明确确认批量覆盖".into());}
        self.with_batch_context(input.context_version,|context| {
            Self::verify_feishu_inspection(config,read)?;
            let batch=self.with(|db|sync_preview::prepare_batch(&mut db.connection,read,input.batch.clone()))?;
            self.arm_batch(context,&read.subject_id,batch)
        })
    }
    pub(crate) fn start_feishu_batch(&self,config:&crate::feishu::FeishuConfig,read:&crate::feishu::SyncInspection,version:u64,id:&str,confirm:bool)->Result<FormalBatchRun,String> {
        if !confirm {return Err("请核对保存的方向和内容，明确确认继续".into());}
        self.with_batch_context(version,|context| {
            Self::verify_feishu_inspection(config,read)?;
            let batch=self.with(|db| {
                sync_preview::verify_batch_scope(&db.connection,id,read)?;
                sync_preview::control_batch(&mut db.connection,id,"start")
            })?;
            self.arm_batch(context,&read.subject_id,batch)
        })
    }
    // Subtracts permission only. No credential lookup, cloud-access gate or network.
    pub(crate) fn stop_feishu_batch(&self,config:&crate::feishu::FeishuConfig,id:&str)->Result<(),String> {
        let mut context=self.feishu_context.lock().map_err(|_|"飞书连接状态繁忙")?;
        if context.batch_run.as_ref().is_some_and(|p|p.batch_id==id) {context.batch_run=None;}
        self.with(|db| {
            sync_preview::verify_batch_target(&db.connection,id,&config.app_token,&config.table_id)?;
            sync_preview::control_batch(&mut db.connection,id,"stop").map(|_|())
        })
    }
    pub(crate) fn step_feishu_batch(&self,config:&crate::feishu::FeishuConfig,read:&crate::feishu::SyncInspection,input:&FormalBatchAction)->Result<FormalBatchStep,String> {
        self.with_batch_context(input.context_version,|context| {
            Self::verify_feishu_inspection(config,read)?;
            if !context.batch_run.as_ref().is_some_and(|p|p.batch_id==input.batch_id&&p.subject_id==read.subject_id&&p.run_id==input.run_id) {
                return Err("批次本次运行许可已失效，请人工确认启动／继续；不会重发".into());
            }
            let result=self.with(|db| {
                sync_preview::verify_batch_scope(&db.connection,&input.batch_id,read)?;
                let choice=sync_preview::claim_batch(&mut db.connection,&input.batch_id,&read.subject_id)?.ok_or("本批已完成，不能重放")?;
                if choice.direction=="cloud" {
                    let outcome=if sync_preview::download(&mut db.connection,read,&choice.input).is_ok(){"completed"}else{"blocked"};
                    return Ok(FormalBatchStep::Settled(sync_preview::finish_batch(&mut db.connection,&input.batch_id,outcome)?));
                }
                let op=match sync_preview::begin_upload(&mut db.connection,read,&choice.input) {
                    Ok(op)=>op,
                    Err(_)=>return Ok(FormalBatchStep::Settled(sync_preview::finish_batch(&mut db.connection,&input.batch_id,"blocked")?)),
                };
                if let Err(reason)=sync_preview::attach_batch_operation(&mut db.connection,&input.batch_id,&op.id) {
                    sync_preview::cancel_unsent_upload(&db.connection,&op.id)?;
                    sync_preview::finish_batch(&mut db.connection,&input.batch_id,"blocked")?;
                    return Err(reason);
                }
                Ok(FormalBatchStep::Upload(op))
            });
            if !matches!(&result,Ok(FormalBatchStep::Settled(batch)) if batch.state=="ready") && !matches!(&result,Ok(FormalBatchStep::Upload(_))) {context.batch_run=None;}
            result
        })
    }
    pub(crate) fn finish_feishu_batch_upload(&self,id:&str,operation:&str,read:Option<&crate::feishu::SyncInspection>)->Result<BatchSummary,String> {
        let result=self.with(|db| {
            let operation=sync_preview::reconcile_upload(&mut db.connection,operation,read)?;
            sync_preview::finish_batch(&mut db.connection,id,if operation.state=="completed"{"completed"}else if operation.state=="notApplied"{"blocked"}else{"uncertain"})
        });
        if !matches!(&result,Ok(batch) if batch.state=="ready") {
            if let Ok(mut context)=self.feishu_context.lock() {
                if context.batch_run.as_ref().is_some_and(|p|p.batch_id==id) {context.batch_run=None;}
            }
        }
        result
    }
    pub(crate) fn reconcile_feishu_batch(&self,config:&crate::feishu::FeishuConfig,read:&crate::feishu::SyncInspection,version:u64,id:&str)->Result<BatchSummary,String> {
        self.with_batch_context(version,|context| {
            Self::verify_feishu_inspection(config,read)?;
            let batch=self.with(|db|sync_preview::reconcile_batch(&mut db.connection,id,read))?;
            if context.batch_run.as_ref().is_some_and(|p|p.batch_id==id) {context.batch_run=None;}
            Ok(batch)
        })
    }
    fn with<T>(
        &self,
        action: impl FnOnce(&mut LocalWorkspace) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut guard = self
            .database
            .lock()
            .map_err(|_| "本地数据库繁忙，请重试".to_string())?;
        if guard.is_none() {
            *guard = Some(LocalWorkspace::open(&self.path)?);
        }
        action(guard.as_mut().expect("initialized local database"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LocalTask {
    id: String,
    title: String,
    detail: String,
    progress: u8,
    quadrant: String,
    today: bool,
    start_date: Option<String>,
    planned_date: Option<String>,
    trashed: bool,
    revision: i64,
    // Missing in old databases/backups: unknown, not the time of migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_modified_at: Option<i64>,
}

fn content_now() -> Result<i64, String> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(error)?
        .as_millis();
    i64::try_from(millis).map_err(error)
}
// Revision and time are evidence, never business content. Undo makes a new edit.
fn same_content(a: &LocalTask, b: &LocalTask) -> bool {
    let mut a = a.clone();
    a.revision = b.revision;
    a.content_modified_at = b.content_modified_at;
    a == *b
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SaveTask {
    id: Option<String>,
    expected_revision: Option<i64>,
    title: String,
    detail: String,
    progress: i64,
    quadrant: String,
    today: bool,
    start_date: Option<String>,
    planned_date: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    tasks: Vec<LocalTask>,
    undo_label: Option<String>,
    database_path: String,
    demo_mode: bool,
    feishu_readonly_enabled: bool,
    feishu_pairing_save_enabled: bool,
}

struct LocalWorkspace {
    connection: Connection,
    path: PathBuf,
    demo_mode: bool,
}

fn error(e: impl std::fmt::Display) -> String {
    format!("本地存储失败：{e}")
}
fn date_valid(s: &str) -> bool {
    if !s.bytes().enumerate().all(|(i, b)| {
        if i == 4 || i == 7 {
            b == b'-'
        } else {
            b.is_ascii_digit()
        }
    }) {
        return false;
    }
    if s.len() != 10 || !s.is_ascii() || &s[4..5] != "-" || &s[7..8] != "-" {
        return false;
    }
    let (Ok(y), Ok(m), Ok(d)) = (
        s[..4].parse::<u32>(),
        s[5..7].parse::<u32>(),
        s[8..].parse::<u32>(),
    ) else {
        return false;
    };
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let max = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if leap {
                29
            } else {
                28
            }
        }
        _ => 0,
    };
    y >= 1 && d >= 1 && d <= max
}
fn normalize_date(d: Option<String>) -> Result<Option<String>, String> {
    let d = d.filter(|s| !s.is_empty());
    if d.as_ref().is_some_and(|s| !date_valid(s)) {
        return Err("日期须为有效的 YYYY-MM-DD".into());
    }
    Ok(d)
}
fn get_task(c: &Connection, id: &str) -> Result<Option<LocalTask>, String> {
    let json: Option<String> = c
        .query_row("SELECT data FROM local_tasks WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()
        .map_err(error)?;
    json.map(|s| serde_json::from_str(&s).map_err(error))
        .transpose()
}
fn next_revision(c: &Connection) -> Result<i64, String> {
    c.execute("UPDATE local_meta SET revision=revision+1 WHERE id=1", [])
        .map_err(error)?;
    c.query_row("SELECT revision FROM local_meta WHERE id=1", [], |r| {
        r.get(0)
    })
    .map_err(error)
}
fn put(c: &Connection, task: &LocalTask) -> Result<(), String> {
    let json = serde_json::to_string(task).map_err(error)?;
    c.execute("INSERT INTO local_tasks(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![task.id, json]).map_err(error)?;
    Ok(())
}
fn log(
    c: &Connection,
    before: &Option<LocalTask>,
    after: &LocalTask,
    label: &str,
) -> Result<(), String> {
    c.execute(
        "INSERT INTO local_history(task_id,before_data,after_data,label) VALUES(?1,?2,?3,?4)",
        params![
            after.id,
            serde_json::to_string(before).map_err(error)?,
            serde_json::to_string(after).map_err(error)?,
            label
        ],
    )
    .map_err(error)?;
    Ok(())
}
fn check_revision(task: &LocalTask, revision: Option<i64>) -> Result<(), String> {
    if revision != Some(task.revision) {
        return Err("任务已变更，请重新打开详情后再保存".into());
    }
    Ok(())
}

impl LocalWorkspace {
    fn open(path: &std::path::Path) -> Result<Self, String> {
        let connection = Connection::open(path).map_err(error)?;
        connection
            .busy_timeout(Duration::from_secs(3))
            .map_err(error)?;
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS local_tasks(id TEXT PRIMARY KEY,data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS local_meta(id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
            INSERT OR IGNORE INTO local_meta VALUES(1,0);
            CREATE TABLE IF NOT EXISTS local_history(id INTEGER PRIMARY KEY AUTOINCREMENT,task_id TEXT NOT NULL,before_data TEXT NOT NULL,after_data TEXT NOT NULL,label TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0);").map_err(error)?;
        sync_preview::initialize(&connection)?;
        ime::initialize(&connection)?;
        Ok(Self {
            connection,
            path: path.to_path_buf(),
            demo_mode: false,
        })
    }
    #[cfg(any(test, feature = "local-workbench-acceptance"))]
    fn seed_acceptance_examples(&mut self) -> Result<(), String> {
        // Only acceptance startup calls this. Never seed the normal workspace.
        self.connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS local_demo_state(id INTEGER PRIMARY KEY CHECK(id=1));",
            )
            .map_err(error)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let initialized: bool = tx
            .query_row("SELECT EXISTS(SELECT 1 FROM local_demo_state)", [], |r| {
                r.get(0)
            })
            .map_err(error)?;
        if initialized {
            return Ok(());
        }
        let existing: i64 = tx
            .query_row("SELECT COUNT(*) FROM local_tasks", [], |r| r.get(0))
            .map_err(error)?;
        if existing == 0 {
            for (index, title, progress, quadrant, today, planned_date) in [
                (
                    1,
                    "【示例】设计任务工作台",
                    71,
                    "in",
                    true,
                    Some("2026-10-20"),
                ),
                (
                    2,
                    "【示例】处理紧急问题",
                    25,
                    "iu",
                    true,
                    Some("2026-10-09"),
                ),
                (3, "【示例】回复常规通知", 0, "un", false, None),
                (4, "【示例】整理已完成资料", 100, "nn", false, None),
            ] {
                let mut task = LocalTask {
                    id: format!("acceptance-example-{index}"),
                    title: title.into(),
                    detail: "隔离验收示例，可编辑、移入回收站和撤销；不是个人真实任务。".into(),
                    progress,
                    quadrant: quadrant.into(),
                    today,
                    start_date: None,
                    planned_date: planned_date.map(str::to_owned),
                    trashed: false,
                    revision: next_revision(&tx)?,
                    content_modified_at: Some(content_now()?),
                };
                put(&tx, &task)?;
                if index == 1 {
                    let before = task.clone();
                    task.progress = 42;
                    task.revision = next_revision(&tx)?;
                    task.content_modified_at = Some(content_now()?);
                    put(&tx, &task)?;
                    log(
                        &tx,
                        &Some(before),
                        &task,
                        "示例进度修改（42% → 撤销恢复 71%）",
                    )?;
                }
            }
        }
        // Keep the marker even after users trash all examples; do not resurrect them.
        tx.execute("INSERT INTO local_demo_state VALUES(1)", [])
            .map_err(error)?;
        tx.commit().map_err(error)?;
        Ok(())
    }
    fn snapshot(&self) -> Result<Snapshot, String> {
        let mut statement = self
            .connection
            .prepare("SELECT data FROM local_tasks ORDER BY id")
            .map_err(error)?;
        let rows = statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(error)?;
        let tasks = rows
            .map(|r| serde_json::from_str::<LocalTask>(&r.map_err(error)?).map_err(error))
            .collect::<Result<Vec<_>, _>>()?;
        let undo_label = self
            .connection
            .query_row(
                "SELECT label FROM local_history WHERE undone=0 ORDER BY id DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(error)?;
        Ok(Snapshot {
            tasks,
            undo_label,
            database_path: self.path.display().to_string(),
            demo_mode: self.demo_mode,
            feishu_readonly_enabled: false,
            feishu_pairing_save_enabled: false,
        })
    }
    fn save(&mut self, input: SaveTask) -> Result<Snapshot, String> {
        let title = input.title.trim().to_string();
        if title.is_empty() || title.chars().count() > 200 {
            return Err("任务名称须为 1–200 个字符".into());
        }
        if input.detail.chars().count() > 10000 {
            return Err("任务详情不能超过 10000 个字符".into());
        }
        if !(0..=100).contains(&input.progress) {
            return Err("进度须为 0–100 的整数".into());
        }
        if !["iu", "in", "un", "nn"].contains(&input.quadrant.as_str()) {
            return Err("请选择有效的四象限分类".into());
        }
        let start_date = normalize_date(input.start_date)?;
        let planned_date = normalize_date(input.planned_date)?;
        if start_date
            .as_ref()
            .zip(planned_date.as_ref())
            .is_some_and(|(s, d)| s > d)
        {
            return Err("计划完成日期不能早于开始日期".into());
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let before = match input.id.as_ref() {
            Some(id) => {
                let t = get_task(&tx, id)?.ok_or("任务不存在")?;
                check_revision(&t, input.expected_revision)?;
                if t.trashed {
                    return Err("请先从回收站恢复任务".into());
                }
                Some(t)
            }
            None => None,
        };
        let id = match input.id {
            Some(id) => id,
            None => tx
                .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
                .map_err(error)?,
        };
        let mut after = LocalTask {
            id,
            title,
            detail: input.detail,
            progress: input.progress as u8,
            quadrant: input.quadrant,
            today: input.today,
            start_date,
            planned_date,
            trashed: false,
            revision: before.as_ref().map_or(0, |t| t.revision),
            content_modified_at: before.as_ref().and_then(|t| t.content_modified_at),
        };
        if before.as_ref().is_some_and(|t| same_content(t, &after)) {
            tx.commit().map_err(error)?;
            return self.snapshot();
        }
        after.revision = next_revision(&tx)?;
        after.content_modified_at = Some(content_now()?);
        put(&tx, &after)?;
        log(
            &tx,
            &before,
            &after,
            if before.is_some() {
                "修改任务"
            } else {
                "新增任务"
            },
        )?;
        tx.commit().map_err(error)?;
        self.snapshot()
    }
    fn trash(&mut self, id: &str, revision: i64, trashed: bool) -> Result<Snapshot, String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let before = get_task(&tx, id)?.ok_or("任务不存在")?;
        check_revision(&before, Some(revision))?;
        if before.trashed == trashed {
            return Err("任务状态已改变，请刷新".into());
        }
        let mut after = before.clone();
        after.trashed = trashed;
        after.revision = next_revision(&tx)?;
        after.content_modified_at = Some(content_now()?);
        put(&tx, &after)?;
        log(
            &tx,
            &Some(before),
            &after,
            if trashed {
                "移入回收站"
            } else {
                "恢复任务"
            },
        )?;
        tx.commit().map_err(error)?;
        self.snapshot()
    }
    fn undo(&mut self) -> Result<Snapshot, String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let row: Option<(i64,String,String,String)> = tx.query_row("SELECT id,task_id,before_data,after_data FROM local_history WHERE undone=0 ORDER BY id DESC LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(error)?;
        let (history_id, id, before, after) = row.ok_or("没有可撤销的操作")?;
        let mut expected: LocalTask = serde_json::from_str(&after).map_err(error)?;
        let current = get_task(&tx, &id)?.ok_or("任务不存在，无法撤销")?;
        expected.revision = current.revision;
        if !same_content(&expected, &current) {
            return Err("任务已发生其他变更，无法安全撤销".into());
        }
        let before: Option<LocalTask> = serde_json::from_str(&before).map_err(error)?;
        if before.as_ref() == Some(&current) {
            // An activity-only IME entry is undoable without changing task content
            // or creating a false cloud-sync difference.
            next_revision(&tx)?;
        } else {
            let mut restored = before.unwrap_or_else(|| {
                let mut t = current;
                t.trashed = true;
                t
            });
            restored.revision = next_revision(&tx)?;
            restored.content_modified_at = Some(content_now()?);
            put(&tx, &restored)?;
        }
        tx.execute(
            "UPDATE local_history SET undone=1 WHERE id=?1",
            [history_id],
        )
        .map_err(error)?;
        ime::undo_activity(&tx, history_id)?;
        tx.commit().map_err(error)?;
        self.snapshot()
    }
}

#[tauri::command]
pub async fn open_local_workbench(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("local-workbench") {
        window.show().map_err(error)?;
        window.unminimize().map_err(error)?;
        window.set_focus().map_err(error)?;
    } else {
        WebviewWindowBuilder::new(
            &app,
            "local-workbench",
            WebviewUrl::App("local-workbench.html".into()),
        )
        .title("本地任务工作台")
        .inner_size(1120.0, 780.0)
        .min_inner_size(720.0, 520.0)
        .resizable(true)
        .decorations(true)
        .transparent(false)
        // Let internal HTML5 card drags reach the frontend on Windows.
        .disable_drag_drop_handler()
        .build()
        .map_err(error)?;
    }
    Ok(())
}
#[tauri::command]
pub fn local_snapshot(state: State<'_, LocalWorkspaceState>) -> Result<Snapshot, String> {
    let mut snapshot = state.with(|db| db.snapshot())?;
    snapshot.feishu_readonly_enabled = state.feishu_readonly_enabled;
    snapshot.feishu_pairing_save_enabled = state.feishu_pairing_save_enabled;
    Ok(snapshot)
}
#[tauri::command]
pub fn local_preview_legacy_workspace(app: AppHandle, state: State<'_, LocalWorkspaceState>) -> Result<Option<RestorePreview>, String> {
    if app.config().identifier != "app.snaptask.desktop" {
        return Err("旧工作台迁移入口仅在完整主程序中使用；隔离程序请使用JSON备份。".into());
    }
    let root = state.path.parent().and_then(|p| p.parent()).ok_or("无法定位旧工作台目录")?;
    backup::preview_legacy(&state, &root.join("app.snaptask.desktop.localworkbench.acceptance.v3").join("local-workspace.sqlite3"))
}
// Fictional evidence only. No network, credentials, task writes or sync approvals.
#[tauri::command]
pub fn local_sync_demo() -> Result<SyncDemo, String> {
    sync_preview::demo(content_now()?)
}

#[tauri::command]
#[cfg(feature = "local-workbench-acceptance")]
pub async fn local_inspect_isolated_feishu_pairing(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
) -> Result<PairingPreview, String> {
    if !cfg!(feature = "feishu-template-v2-acceptance")
        || !isolated_sync_profile(&window.app_handle().config().identifier,false)
        || !state.feishu_readonly_enabled
    {
        return Err("该隔离程序未启用第二账号飞书只读检查".into());
    }
    let tasks = state.with(|db| db.snapshot().map(|snapshot| snapshot.tasks))?;
    let inspection = tauri::async_runtime::spawn_blocking(move || {
        let config = isolated_sync_config();
        if !crate::feishu::user_authorized_for_app(&config.app_id) {
            return Err("第二账号隔离授权不可用，请重新连接；不会改用正式账号".into());
        }
        let token = crate::feishu::read_user_access_token(&config.app_id)?;
        let now = crate::storage::unix_now()
            .checked_mul(1000)
            .ok_or("检查时间无效")?;
        let inspection = crate::feishu::inspect_sync_with_user_token(&config, &token, now)?;
        Ok::<_, String>(inspection)
    })
    .await
    .map_err(|_| "飞书首次关联预览未完成，请重试".to_string())??;
    state.with(|db| sync_preview::pairing_preview(&db.connection, &tasks, inspection))
}

#[tauri::command]
#[cfg(feature = "local-workbench-acceptance")]
pub async fn local_save_isolated_feishu_pairing(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    input: SavePairingInput,
) -> Result<SavePairingResult, String> {
    if !cfg!(feature = "feishu-template-v2-acceptance")
        || !isolated_sync_profile(&window.app_handle().config().identifier,true)
        || !state.feishu_readonly_enabled
        || !state.feishu_pairing_save_enabled
    {
        return Err("该隔离程序未启用首次关联保存".into());
    }
    let config = isolated_sync_config();
    if !crate::feishu::user_authorized_for_app(&config.app_id) {
        return Err("第二账号隔离授权不可用，请重新连接；不会改用正式账号".into());
    }
    let token = crate::feishu::read_user_access_token(&config.app_id)?;
    let inspection = tauri::async_runtime::spawn_blocking(move || {
        let now = crate::storage::unix_now()
            .checked_mul(1000)
            .ok_or("检查时间无效")?;
        crate::feishu::inspect_sync_with_user_token(&config, &token, now)
    })
    .await
    .map_err(|_| "保存前飞书复检未完成，请重试".to_string())??;
    let cloud = inspection
        .records
        .iter()
        .find(|record| record.record_id == input.cloud_id)
        .ok_or("所选飞书记录已不存在或当前不可见，请重新检查")?;
    state.with(|db| {
        let local = db
            .snapshot()?
            .tasks
            .into_iter()
            .find(|task| task.id == input.local_id)
            .ok_or("所选本地任务已不存在，请重新检查")?;
        sync_preview::save_pairing(&mut db.connection, &local, cloud, &inspection, &input)
    })
}
#[tauri::command]
#[cfg(feature = "local-workbench-acceptance")]
pub async fn local_download_isolated_feishu_task(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    input: DownloadInput,
) -> Result<Snapshot, String> {
    if !cfg!(feature = "feishu-template-v2-acceptance")
        || !isolated_sync_profile(&window.app_handle().config().identifier,true)
        || !state.feishu_pairing_save_enabled
    {
        return Err("当前程序未启用隔离表确认同步".into());
    }
    let inspection = tauri::async_runtime::spawn_blocking(|| {
        let config = isolated_sync_config();
        if !crate::feishu::user_authorized_for_app(&config.app_id) {
            return Err("第二账号授权不可用，请重新连接".into());
        }
        let token = crate::feishu::read_user_access_token(&config.app_id)?;
        crate::feishu::inspect_sync_with_user_token(&config, &token, content_now()?)
    })
    .await
    .map_err(|_| "同步前复检未完成")??;
    state.with(|db| {
        sync_preview::download(&mut db.connection, &inspection, &input)?;
        db.snapshot()
    })
}

#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
fn upload_scope(window: &tauri::WebviewWindow) -> Result<crate::feishu::FeishuConfig, String> {
    if !isolated_sync_profile(&window.app_handle().config().identifier,true) {
        return Err("仅允许第二账号隔离验收程序写入".into());
    }
    Ok(isolated_sync_config())
}

fn isolated_sync_config()->crate::feishu::FeishuConfig{
    if cfg!(feature="feishu-direct-sync-ui-acceptance"){return crate::direct_account_scope::config();}
    crate::feishu::FeishuConfig {
        app_id: crate::write_acceptance::TEMPLATE_V2_APP_ID.into(),
        app_token: crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN.into(),
        table_id: crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID.into(),
        secret_configured: false,
    }
}
fn isolated_sync_profile(identifier:&str,write:bool)->bool{
    if cfg!(feature="feishu-direct-sync-ui-acceptance"){return identifier==crate::direct_account_scope::SYNC_IDENTIFIER;}
    (identifier=="app.snaptask.desktop.localworkbench.syncs3")||(!write&&identifier=="app.snaptask.desktop.localworkbench.syncs2")
}
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
async fn upload_inspect(
    config: crate::feishu::FeishuConfig,
) -> Result<(String, crate::feishu::SyncInspection), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !crate::feishu::user_authorized_for_app(&config.app_id) {
            return Err("第二账号授权不可用".into());
        }
        let token = crate::feishu::read_user_access_token(&config.app_id)?;
        let read = crate::feishu::inspect_sync_with_user_token(&config, &token, content_now()?)?;
        Ok((token, read))
    })
    .await
    .map_err(|_| "写入前检查未完成".to_string())?
}
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
async fn upload_send(
    state: &LocalWorkspaceState,
    config: crate::feishu::FeishuConfig,
    token: String,
    op: sync_preview::WorkOperation,
) -> Result<UploadSummary, String> {
    let id = op.id.clone();
    let after = tauri::async_runtime::spawn_blocking(move || {
        crate::feishu::attempt_sync_update(&config,&token,&op.record_id,&op.content,&op.status,||content_now().ok())
    })
    .await;
    let attempt = after.unwrap_or(crate::feishu::SyncUpdateAttempt {unsent:None,inspection:None});
    state.with(|db| settle_upload_attempt(&mut db.connection,&id,attempt))
}
#[cfg(all(feature = "local-workbench-acceptance", feature = "feishu-template-v2-acceptance"))]
fn settle_upload_attempt(
    connection: &mut Connection, id: &str, attempt: crate::feishu::SyncUpdateAttempt,
) -> Result<UploadSummary,String> {
    if let Some(reason) = attempt.unsent {
        sync_preview::cancel_unsent_upload(connection, id)?;
        return Err(format!("尚未发出写入：{reason}"));
    }
    sync_preview::reconcile_upload(connection, id, attempt.inspection.as_ref())
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_upload_isolated_feishu_task(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    input: DownloadInput,
) -> Result<UploadSummary, String> {
    let config = upload_scope(&window)?;
    let (token, read) = upload_inspect(config.clone()).await?;
    let op = state.with(|db| sync_preview::begin_upload(&mut db.connection, &read, &input))?;
    upload_send(&state, config, token, op).await
}

#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_prepare_isolated_feishu_batch(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    input: BatchInput,
) -> Result<BatchSummary, String> {
    let config = upload_scope(&window)?;
    let (_, read) = upload_inspect(config).await?;
    state.with(|db| sync_preview::prepare_batch(&mut db.connection, &read, input))
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_step_isolated_feishu_batch(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    batch_id: String,
) -> Result<BatchSummary, String> {
    let config = upload_scope(&window)?;
    // Re-read before claiming; no task mutation can occur before a durable claim.
    let (token, read) = match upload_inspect(config.clone()).await {
        Ok(result) => result,
        Err(reason) => {
            state.with(|db| {
                sync_preview::verify_batch_target(
                    &db.connection,
                    &batch_id,
                    &config.app_token,
                    &config.table_id,
                )?;
                sync_preview::control_batch(&mut db.connection, &batch_id, "stop")
            })?;
            return Err(reason);
        }
    };
    let choice = state
        .with(|db| {
            sync_preview::verify_batch_scope(&db.connection, &batch_id, &read)?;
            sync_preview::claim_batch(&mut db.connection, &batch_id, &read.subject_id)
        })?
        .ok_or("本批已经完成，不能重复提交")?;
    let outcome = if choice.direction == "cloud" {
        state
            .with(|db| sync_preview::download(&mut db.connection, &read, &choice.input))
            .map(|_| "completed".to_string())
    } else {
        match state.with(|db| {
            let op = sync_preview::begin_upload(&mut db.connection, &read, &choice.input)?;
            sync_preview::attach_batch_operation(&mut db.connection, &batch_id, &op.id)?;
            Ok(op)
        }) {
            Ok(op) => Ok(match upload_send(&state, config, token, op).await {
                Ok(s) if s.state == "completed" => "completed".to_string(),
                _ => "uncertain".to_string(),
            }),
            Err(e) => Err(e),
        }
    };
    // Errors after claim are conservative: block the batch; single upload intents
    // remain separately queryable and are never sent again by this scheduler.
    let status = match &outcome {
        Ok(s) => s.as_str(),
        Err(_) => "blocked",
    };
    let result =
        state.with(|db| sync_preview::finish_batch(&mut db.connection, &batch_id, status))?;
    Ok(result)
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_control_isolated_feishu_batch(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    batch_id: String,
    action: String,
) -> Result<BatchSummary, String> {
    let config = upload_scope(&window)?;
    // Stop must remain available offline. It only prevents the next claim.
    if action == "stop" {
        return state.with(|db| {
            sync_preview::verify_batch_target(
                &db.connection,
                &batch_id,
                &config.app_token,
                &config.table_id,
            )?;
            sync_preview::control_batch(&mut db.connection, &batch_id, &action)
        });
    }
    let (_, read) = upload_inspect(config).await?;
    state.with(|db| {
        sync_preview::verify_batch_scope(&db.connection, &batch_id, &read)?;
        sync_preview::control_batch(&mut db.connection, &batch_id, &action)
    })
}

#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_reconcile_isolated_feishu_batch(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    batch_id: String,
) -> Result<BatchSummary, String> {
    let config = upload_scope(&window)?;
    let (_, read) = upload_inspect(config).await?;
    state.with(|db| sync_preview::reconcile_batch(&mut db.connection, &batch_id, &read))
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_reconcile_isolated_feishu_batch() -> Result<(), String> {
    Err("当前构建没有批次核实能力".into())
}

#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_prepare_isolated_feishu_batch() -> Result<(), String> {
    Err("当前构建没有批量执行能力".into())
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_step_isolated_feishu_batch() -> Result<(), String> {
    Err("当前构建没有批量执行能力".into())
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_control_isolated_feishu_batch() -> Result<(), String> {
    Err("当前构建没有批量执行能力".into())
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_restore_isolated_feishu_upload(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    operation_id: String,
) -> Result<UploadSummary, String> {
    let config = upload_scope(&window)?;
    let (token, read) = upload_inspect(config.clone()).await?;
    let op = state
        .with(|db| sync_preview::begin_upload_restore(&mut db.connection, &read, &operation_id))?;
    upload_send(&state, config, token, op).await
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    feature = "feishu-template-v2-acceptance"
))]
pub async fn local_reconcile_isolated_feishu_upload(
    window: tauri::WebviewWindow,
    state: State<'_, LocalWorkspaceState>,
    operation_id: String,
) -> Result<UploadSummary, String> {
    let config = upload_scope(&window)?;
    let (_, read) = upload_inspect(config).await?;
    state.with(|db| sync_preview::reconcile_upload(&mut db.connection, &operation_id, Some(&read)))
}

#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_upload_isolated_feishu_task() -> Result<(), String> {
    Err("当前构建没有飞书写入能力".into())
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_restore_isolated_feishu_upload() -> Result<(), String> {
    Err("当前构建没有飞书恢复能力".into())
}
#[tauri::command]
#[cfg(all(
    feature = "local-workbench-acceptance",
    not(feature = "feishu-template-v2-acceptance")
))]
pub fn local_reconcile_isolated_feishu_upload() -> Result<(), String> {
    Err("当前构建没有飞书核实能力".into())
}

#[tauri::command]
pub fn local_save_task(
    state: State<'_, LocalWorkspaceState>,
    input: SaveTask,
) -> Result<Snapshot, String> {
    state.with(|db| db.save(input))
}
#[tauri::command]
pub fn local_move_task(
    state: State<'_, LocalWorkspaceState>,
    input: MoveTask,
) -> Result<Snapshot, String> {
    state.with(|db| db.move_task(input))
}
#[tauri::command]
pub fn local_set_trashed(
    state: State<'_, LocalWorkspaceState>,
    id: String,
    revision: i64,
    trashed: bool,
) -> Result<Snapshot, String> {
    state.with(|db| db.trash(&id, revision, trashed))
}
#[tauri::command]
pub fn local_undo(state: State<'_, LocalWorkspaceState>) -> Result<Snapshot, String> {
    state.with(|db| db.undo())
}

pub use ime::ImeApiState;

#[tauri::command]
pub fn local_export_backup(
    state: State<'_, LocalWorkspaceState>,
) -> Result<Option<ExportResult>, String> {
    backup::export(&state)
}
#[tauri::command]
pub fn local_export_csv(
    state: State<'_, LocalWorkspaceState>,
) -> Result<Option<CsvExport>, String> {
    csv_export::export(&state)
}
#[tauri::command]
pub fn local_preview_restore(
    state: State<'_, LocalWorkspaceState>,
    json: String,
    source: String,
) -> Result<Option<RestorePreview>, String> {
    backup::preview(&state, &json, &source)
}
#[tauri::command]
pub fn local_confirm_restore(
    state: State<'_, LocalWorkspaceState>,
    request_id: String,
    confirm_replace: bool,
) -> Result<RestoreResult, String> {
    backup::confirm(&state, &request_id, confirm_replace)
}
#[tauri::command]
pub fn local_cancel_restore(
    state: State<'_, LocalWorkspaceState>,
    request_id: String,
) -> Result<(), String> {
    backup::cancel(&state, &request_id)
}

/// Acceptance profiles have no tray/autostart; cloud/auth commands reject non-sync profiles.
#[cfg(feature = "local-workbench-acceptance")]
pub fn run_acceptance() {
    use isolated_auth::*;
    let context = tauri::generate_context!();
    // Tauri panics on a setup-hook error even when Builder::run returns Result.
    // Validate the prepared direct profile before entering that hook.
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    let direct_state = {
        crate::startup_diagnostics::stage("credential_profile");
        let prepare = || -> Result<LocalWorkspaceState, String> {
            crate::feishu::validate_credential_profile(&context.config().identifier)?;
            if context.config().identifier != crate::direct_account_scope::SYNC_IDENTIFIER {
                return Err("本地验收程序必须使用隔离标识".into());
            }
            let exe = std::env::current_exe().map_err(error)?;
            LocalWorkspaceState::prepared_direct_from_exe(&exe)
        };
        match prepare() {
            Ok(state) => state,
            Err(error) => { crate::startup_diagnostics::failure(&error); return; }
        }
    };
    let result = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(IsolatedAuthState::default())
        .setup(move |app| {
            #[cfg(not(feature = "feishu-direct-sync-ui-acceptance"))]
            {
            crate::feishu::validate_credential_profile(&app.config().identifier).map_err(std::io::Error::other)?;
            let old_profile=[
                "app.snaptask.desktop.localworkbench.acceptance",
                "app.snaptask.desktop.localworkbench.acceptance.v3",
                "app.snaptask.desktop.localworkbench.backupacceptance",
                "app.snaptask.desktop.localworkbench.restoreacceptance.rev22",
                "app.snaptask.desktop.localworkbench.syncs1",
                "app.snaptask.desktop.localworkbench.syncs2",
                "app.snaptask.desktop.localworkbench.syncs3",
            ]
            .contains(&app.config().identifier.as_str());
            if !old_profile {
                return Err(std::io::Error::other("本地验收程序必须使用隔离标识").into());
            }
            let path = app.path().app_data_dir()?;
            std::fs::create_dir_all(&path)?;
            let readonly_enabled = cfg!(feature = "feishu-template-v2-acceptance")
                && isolated_sync_profile(&app.config().identifier,false);
            let pairing_save_enabled = cfg!(feature = "feishu-template-v2-acceptance")
                && isolated_sync_profile(&app.config().identifier,true);
            let state=
                LocalWorkspaceState::acceptance(
                    path.join("local-workspace.sqlite3"),
                    readonly_enabled,
                    pairing_save_enabled,
                )
                .map_err(std::io::Error::other)?;
            app.manage(state);
            }
            #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
            app.manage(direct_state);
            #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
            crate::startup_diagnostics::ready();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            local_snapshot,
            local_start_isolated_feishu_auth,
            local_poll_isolated_feishu_auth,
            local_cancel_isolated_feishu_auth,
            local_open_isolated_feishu_auth,
            local_sync_demo,
            local_inspect_isolated_feishu_pairing,
            local_save_isolated_feishu_pairing,
            local_download_isolated_feishu_task,
            local_upload_isolated_feishu_task,
            local_restore_isolated_feishu_upload,
            local_reconcile_isolated_feishu_upload,
            local_prepare_isolated_feishu_batch,
            local_step_isolated_feishu_batch,
            local_control_isolated_feishu_batch,
            local_reconcile_isolated_feishu_batch,
            local_save_task,
            local_move_task,
            local_set_trashed,
            local_undo,
            local_export_backup,
            local_export_csv,
            local_preview_restore,
            local_confirm_restore,
            local_cancel_restore
        ])
        .run(context);
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    if let Err(error) = result { crate::startup_diagnostics::failure(&error.to_string()); }
    #[cfg(not(feature = "feishu-direct-sync-ui-acceptance"))]
    result.expect("本地工作台隔离验收启动失败");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn formal_batch_upload_keeps_uncertain_journal_and_readonly_reconcile_never_replays() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let task=state.with(|db|db.save(input(None,25))).unwrap().tasks.remove(0);
        let config=crate::feishu::FeishuConfig{app_id:"cli_queue".into(),app_token:"base_queue".into(),table_id:"tbl_queue".into(),secret_configured:false};
        let desired=serde_json::json!({"title":task.title,"detail":task.detail,"progress":25,"quadrant":task.quadrant,"today":task.today,"startDate":task.start_date,"plannedDate":task.planned_date});
        let mut old=desired.clone();old["progress"]=serde_json::json!(60);
        let inspection=|fields:serde_json::Value|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"批次上传用户".into(),subject_id:"cli_queue:user".into(),app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],records:vec![crate::feishu::SyncRecordTime{record_id:"recQueue".into(),title:task.title.clone(),progress:fields["progress"].as_u64().unwrap() as u8,status:"进行中".into(),planned_date:task.planned_date.clone(),modified_at:Some(1792500000000),time_source:"platform".into(),shared_fields:fields}]};
        state.save_feishu_pairing(&config,inspection(old.clone()),&FormalPairingInput{context_version:0,subject_id:"cli_queue:user".into(),pairing:sync_preview::SavePairingInput{local_id:task.id.clone(),cloud_id:"recQueue".into(),expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000)}}).unwrap();
        let run=state.prepare_feishu_batch(&config,&inspection(old.clone()),&FormalBatchInput{context_version:0,confirm_batch:true,batch:BatchInput{entries:vec![sync_preview::BatchChoice{direction:"local".into(),input:sync_preview::DownloadInput{local_id:task.id.clone(),cloud_id:"recQueue".into(),subject_id:"cli_queue:user".into(),expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000),expected_cloud_fields:old.clone()}}]}}).unwrap();
        let action=FormalBatchAction{context_version:0,batch_id:run.batch.id.clone(),run_id:run.run_id};
        let FormalBatchStep::Upload(op)=state.step_feishu_batch(&config,&inspection(old.clone()),&action).unwrap() else {panic!("upload became download")};
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,inspection(old.clone())).unwrap()).unwrap();
        assert_eq!(preview["uploadOperations"][0]["id"],op.id,"durable evidence exists before sender is invoked");
        let sends=std::cell::Cell::new(0);
        state.dispatch_feishu_upload(0,&op.id,||{sends.set(sends.get()+1);Err("lost response".into())}).unwrap();
        assert_eq!(state.finish_feishu_batch_upload(&run.batch.id,&op.id,None).unwrap().state,"uncertain");
        assert!(state.step_feishu_batch(&config,&inspection(old.clone()),&action).is_err());
        assert!(state.start_feishu_batch(&config,&inspection(old.clone()),0,&run.batch.id,true).is_err());
        assert!(state.reconcile_feishu_batch(&config,&inspection(old),0,&run.batch.id).is_err());
        let mut verified=inspection(desired);verified.records[0].modified_at=Some(1792500001000);
        assert_eq!(state.reconcile_feishu_batch(&config,&verified,0,&run.batch.id).unwrap().state,"completed");
        assert_eq!(sends.get(),1);
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks,vec![task]);
    }
    #[test]
    fn formal_batch_download_requires_review_and_fresh_run_after_stop_without_replaying_completed_item() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let config=crate::feishu::FeishuConfig{app_id:"cli_batch".into(),app_token:"base_batch".into(),table_id:"tbl_batch".into(),secret_configured:false};
        let mut read=crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"批次用户".into(),subject_id:"cli_batch:user".into(),app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,records:vec![],conditional_write_verified:false,warnings:vec![]};
        let mut choices=vec![];
        for n in 0..2 {
            let title=format!("批次任务{n}");let mut create=input(None,25);create.title=title.clone();
            let task=state.with(|db|db.save(create)).unwrap().tasks.into_iter().find(|t|t.title==title).unwrap();
            let fields=serde_json::json!({"title":task.title,"detail":task.detail,"progress":75,"quadrant":task.quadrant,"today":task.today,"startDate":task.start_date,"plannedDate":task.planned_date});
            let cloud_id=format!("recBatch{n}");
            read.records.push(crate::feishu::SyncRecordTime{record_id:cloud_id.clone(),title:task.title.clone(),progress:75,status:"进行中".into(),planned_date:task.planned_date.clone(),modified_at:Some(1792500000000),time_source:"platform".into(),shared_fields:fields.clone()});
            // Use the formal pairing boundary, not a pre-seeded batch or database side channel.
            let pairing_read=crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"批次用户".into(),subject_id:read.subject_id.clone(),app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:read.checked_at,conditional_write_verified:false,warnings:vec![],records:vec![crate::feishu::SyncRecordTime{record_id:cloud_id.clone(),title:task.title.clone(),progress:75,status:"进行中".into(),planned_date:task.planned_date.clone(),modified_at:Some(1792500000000),time_source:"platform".into(),shared_fields:fields.clone()}]};
            state.save_feishu_pairing(&config,pairing_read,&FormalPairingInput{context_version:0,subject_id:read.subject_id.clone(),pairing:sync_preview::SavePairingInput{local_id:task.id.clone(),cloud_id:cloud_id.clone(),expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000)}}).unwrap();
            choices.push(sync_preview::BatchChoice{direction:"cloud".into(),input:sync_preview::DownloadInput{local_id:task.id,cloud_id,subject_id:read.subject_id.clone(),expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000),expected_cloud_fields:fields}});
        }
        let before=state.with(|db|db.snapshot()).unwrap().tasks;
        let mut confirmation=FormalBatchInput{context_version:0,confirm_batch:false,batch:BatchInput{entries:choices}};
        assert!(state.prepare_feishu_batch(&config,&read,&confirmation).is_err());
        confirmation.confirm_batch=true;
        let run=state.prepare_feishu_batch(&config,&read,&confirmation).unwrap();
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks,before,"preparation never downloads");
        let action=FormalBatchAction{context_version:0,batch_id:run.batch.id.clone(),run_id:run.run_id.clone()};
        let FormalBatchStep::Settled(first)=state.step_feishu_batch(&config,&read,&action).unwrap() else {panic!("download became upload")};
        assert_eq!((first.completed,first.queued),(1,1));
        state.stop_feishu_batch(&config,&run.batch.id).unwrap();
        assert!(state.step_feishu_batch(&config,&read,&action).is_err());
        assert!(state.start_feishu_batch(&config,&read,0,&run.batch.id,false).is_err());
        let continued=state.start_feishu_batch(&config,&read,0,&run.batch.id,true).unwrap();
        assert_ne!(continued.run_id,run.run_id,"old renderer approval must not survive manual continuation");
        assert!(state.step_feishu_batch(&config,&read,&action).is_err());
        let action=FormalBatchAction{run_id:continued.run_id,..action};
        let FormalBatchStep::Settled(done)=state.step_feishu_batch(&config,&read,&action).unwrap() else {panic!("download became upload")};
        assert_eq!((done.state.as_str(),done.completed),("completed",2));
        assert!(state.step_feishu_batch(&config,&read,&action).is_err());
        assert!(state.with(|db|db.snapshot()).unwrap().tasks.iter().all(|t|t.progress==75));
        state.with(|db|db.undo()).unwrap();
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks.iter().filter(|t|t.progress==25).count(),1,"each downloaded item retains its own undo history");
    }
    #[test]
    fn formal_restore_requires_reviewed_content_and_unchanged_uploaded_version_then_preserves_local_task() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let task=state.with(|db|db.save(input(None,25))).unwrap().tasks.remove(0);
        let config=crate::feishu::FeishuConfig{app_id:"cli_restore".into(),app_token:"base_restore".into(),table_id:"tbl_restore".into(),secret_configured:false};
        let desired=serde_json::json!({"title":task.title,"detail":task.detail,"progress":25,"quadrant":task.quadrant,"today":task.today,"startDate":task.start_date,"plannedDate":task.planned_date});
        let mut before=desired.clone();before["progress"]=serde_json::json!(60);
        let inspect=|fields:serde_json::Value,status:&str,time:i64|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"恢复用户".into(),subject_id:"cli_restore:user_a".into(),
            app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],
            records:vec![crate::feishu::SyncRecordTime{record_id:"recRestore".into(),title:task.title.clone(),progress:fields["progress"].as_u64().unwrap() as u8,status:status.into(),planned_date:task.planned_date.clone(),modified_at:Some(time),time_source:"platform".into(),shared_fields:fields}]};
        let original=inspect(before.clone(),"暂停",1792500000000);
        state.save_feishu_pairing(&config,original,&FormalPairingInput{context_version:0,subject_id:"cli_restore:user_a".into(),pairing:sync_preview::SavePairingInput{local_id:task.id.clone(),cloud_id:"recRestore".into(),
            expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000)}}).unwrap();
        let upload=state.begin_feishu_upload(&config,&inspect(before.clone(),"暂停",1792500000000),&FormalUploadInput{context_version:0,confirm_overwrite:true,upload:sync_preview::DownloadInput{local_id:task.id.clone(),cloud_id:"recRestore".into(),subject_id:"cli_restore:user_a".into(),
            expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000),expected_cloud_fields:before.clone()}}).unwrap();
        state.dispatch_feishu_upload(0,&upload.id,||Ok(())).unwrap();
        assert_eq!(state.finish_feishu_upload(&upload.id,Some(&inspect(desired.clone(),"进行中",1792500001000))).unwrap().state,"completed");
        let baseline=serde_json::to_value(state.preview_feishu_connection(&config,inspect(desired.clone(),"进行中",1792500001000)).unwrap()).unwrap()["existingLinks"][0]["lastSuccessfulSyncAt"].clone();
        let mut confirmation=FormalRestoreInput{context_version:0,operation_id:upload.id.clone(),confirm_restore:false,expected_restore_content:before.clone(),expected_restore_status:"暂停".into()};
        assert!(state.begin_feishu_restore(&config,&inspect(desired.clone(),"进行中",1792500001000),&confirmation).is_err());
        confirmation.confirm_restore=true;
        assert!(state.begin_feishu_restore(&config,&inspect(desired.clone(),"进行中",1792500001001),&confirmation).is_err(),"a later cloud edit must block restoration even if content looks equal");
        confirmation.expected_restore_status="已完成".into();
        assert!(state.begin_feishu_restore(&config,&inspect(desired.clone(),"进行中",1792500001000),&confirmation).is_err());
        confirmation.expected_restore_status="暂停".into();
        let restore=state.begin_feishu_restore(&config,&inspect(desired,"进行中",1792500001000),&confirmation).unwrap();
        assert_eq!(restore.content,before);assert_eq!(restore.status,"暂停");
        state.dispatch_feishu_upload(0,&restore.id,||Ok(())).unwrap();
        assert_eq!(state.finish_feishu_upload(&restore.id,None).unwrap().state,"pending");
        assert_eq!(state.reconcile_feishu_upload(&config,inspect(before.clone(),"暂停",1792500002000),0,&restore.id).unwrap().state,"completed");
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,inspect(before,"暂停",1792500002000)).unwrap()).unwrap();
        assert!(preview["uploadOperations"].as_array().unwrap().iter().any(|op|op["id"]==upload.id&&op["state"]=="restored"));
        assert_eq!(preview["existingLinks"][0]["lastSuccessfulSyncAt"],baseline,"restoring cloud must not claim local and cloud are synced");
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks,vec![task]);
    }
    #[test]
    fn formal_preview_keeps_older_unresolved_operations_visible_beyond_recent_completed_history() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let config=crate::feishu::FeishuConfig{app_id:"cli_log".into(),app_token:"base_log".into(),table_id:"tbl_log".into(),secret_configured:false};
        let mut unresolved=Vec::new();
        for index in 0..14 {
            let title=format!("日志任务{index}");let mut create=input(None,42);create.title=title.clone();
            let task=state.with(|db|db.save(create)).unwrap().tasks.into_iter().find(|t|t.title==title).unwrap();
            let content=serde_json::json!({"title":task.title,"detail":task.detail,"progress":42,"quadrant":task.quadrant,"today":task.today,"startDate":task.start_date,"plannedDate":task.planned_date});
            let mut old=content.clone();old["progress"]=serde_json::json!(60);
            let inspect=|fields:serde_json::Value|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"日志用户".into(),subject_id:"cli_log:user_a".into(),
                app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],
                records:vec![crate::feishu::SyncRecordTime{record_id:format!("recLog{index}"),title:task.title.clone(),progress:fields["progress"].as_u64().unwrap() as u8,status:"进行中".into(),planned_date:task.planned_date.clone(),
                    modified_at:Some(1792500000000),time_source:"platform".into(),shared_fields:fields}]};
            state.save_feishu_pairing(&config,inspect(old.clone()),&FormalPairingInput{context_version:0,subject_id:"cli_log:user_a".into(),pairing:sync_preview::SavePairingInput{local_id:task.id.clone(),cloud_id:format!("recLog{index}"),
                expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000)}}).unwrap();
            let op=state.begin_feishu_upload(&config,&inspect(old.clone()),&FormalUploadInput{context_version:0,confirm_overwrite:true,upload:sync_preview::DownloadInput{local_id:task.id.clone(),cloud_id:format!("recLog{index}"),subject_id:"cli_log:user_a".into(),
                expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000),expected_cloud_fields:old}}).unwrap();
            if index==0 {unresolved.push(op.id);}
            else if index==1 {let mut conflicting=content.clone();conflicting["progress"]=serde_json::json!(80);state.finish_feishu_upload(&op.id,Some(&inspect(conflicting))).unwrap();unresolved.push(op.id);}
            else {state.finish_feishu_upload(&op.id,Some(&inspect(content))).unwrap();}
        }
        let read=crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"日志用户".into(),subject_id:"cli_log:user_a".into(),app_id:config.app_id.clone(),
            app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,records:vec![],conditional_write_verified:false,warnings:vec![]};
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,read).unwrap()).unwrap();
        let operations=preview["uploadOperations"].as_array().unwrap();
        for id in unresolved {assert!(operations.iter().any(|op|op["id"]==id),"old pending/conflict operation lost its reconciliation entry");}
        assert_eq!(operations.iter().filter(|op|op["state"]=="completed").count(),10);
        assert_eq!(operations.len(),12);
    }
    #[test]
    fn formal_upload_is_confirmed_fenced_and_uncertain_results_are_reconciled_without_resend() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let task=state.with(|db|db.save(input(None,25))).unwrap().tasks.remove(0);
        let config=crate::feishu::FeishuConfig{app_id:"cli_upload".into(),app_token:"base_upload".into(),table_id:"tbl_upload".into(),secret_configured:false};
        let desired=serde_json::json!({"title":task.title,"detail":task.detail,"progress":25,"quadrant":task.quadrant,"today":task.today,"startDate":task.start_date,"plannedDate":task.planned_date});
        let mut old=desired.clone();old["progress"]=serde_json::json!(60);
        let inspection=|fields:serde_json::Value|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"上传用户".into(),subject_id:"cli_upload:user_a".into(),
            app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],
            records:vec![crate::feishu::SyncRecordTime{record_id:"rec_upload".into(),title:task.title.clone(),progress:fields["progress"].as_u64().unwrap() as u8,
                status:"进行中".into(),planned_date:task.planned_date.clone(),modified_at:Some(1792500000000),time_source:"platform".into(),shared_fields:fields}]};
        state.save_feishu_pairing(&config,inspection(old.clone()),&FormalPairingInput{context_version:0,subject_id:"cli_upload:user_a".into(),pairing:sync_preview::SavePairingInput{
            local_id:task.id.clone(),cloud_id:"rec_upload".into(),expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000)}}).unwrap();
        let mut confirmation=FormalUploadInput{context_version:0,confirm_overwrite:false,upload:sync_preview::DownloadInput{local_id:task.id.clone(),cloud_id:"rec_upload".into(),subject_id:"cli_upload:user_a".into(),
            expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000),expected_cloud_fields:old.clone()}};
        assert!(state.begin_feishu_upload(&config,&inspection(old.clone()),&confirmation).is_err());
        confirmation.confirm_overwrite=true;
        let cancelled=state.begin_feishu_upload(&config,&inspection(old.clone()),&confirmation).unwrap();
        state.begin_feishu_change().unwrap();state.end_feishu_change();
        assert!(state.dispatch_feishu_upload(0,&cancelled.id,||panic!("stale confirmation sent a mutation")).is_err());
        confirmation.context_version=state.feishu_context_version().unwrap();
        let op=state.begin_feishu_upload(&config,&inspection(old.clone()),&confirmation).unwrap();
        let sends=std::cell::Cell::new(0);
        state.dispatch_feishu_upload(confirmation.context_version,&op.id,||{sends.set(sends.get()+1);Err("response lost after request".into())}).unwrap();
        assert_eq!(state.finish_feishu_upload(&op.id,None).unwrap().state,"pending");
        assert!(state.begin_feishu_upload(&config,&inspection(old.clone()),&confirmation).is_err());
        let before=serde_json::to_value(state.preview_feishu_connection(&config,inspection(old)).unwrap()).unwrap();
        assert_eq!(before["existingLinks"][0]["baselineEstablished"],false);
        let mut foreign=inspection(desired.clone());foreign.subject_id="cli_upload:user_b".into();
        assert!(state.reconcile_feishu_upload(&config,foreign,confirmation.context_version,&op.id).is_err(),"another user must not inspect or settle this operation");
        let verified=state.reconcile_feishu_upload(&config,inspection(desired),confirmation.context_version,&op.id).unwrap();
        assert_eq!(verified.state,"completed");assert_eq!(sends.get(),1);
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks,vec![task]);
    }
    #[test]
    fn formal_download_rechecks_context_then_updates_seven_fields_with_baseline_and_undo() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let task=state.with(|db|db.save(input(None,25))).unwrap().tasks.remove(0);
        let config=crate::feishu::FeishuConfig{app_id:"cli_download".into(),app_token:"base_download".into(),table_id:"tbl_download".into(),secret_configured:false};
        let fields=serde_json::json!({"title":"确认下载任务","detail":"来自飞书的详情","progress":75,"quadrant":"nn","today":false,"startDate":"2026-10-21","plannedDate":"2026-10-28"});
        let inspection=|user:&str|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"同名账号".into(),
            subject_id:format!("cli_download:{user}"),app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),
            checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],
            records:vec![crate::feishu::SyncRecordTime{shared_fields:fields.clone(),record_id:"rec_download".into(),title:"确认下载任务".into(),
                progress:75,status:"进行中".into(),planned_date:Some("2026-10-28".into()),modified_at:Some(1792500000000),time_source:"platform".into()}]};
        let version=state.feishu_context_version().unwrap();
        state.save_feishu_pairing(&config,inspection("user_a"),&FormalPairingInput{context_version:version,subject_id:"cli_download:user_a".into(),
            pairing:sync_preview::SavePairingInput{local_id:task.id.clone(),cloud_id:"rec_download".into(),expected_local_revision:task.revision,
                expected_local_modified_at:task.content_modified_at,expected_cloud_modified_at:Some(1792500000000)}}).unwrap();
        let mut confirmation=FormalDownloadInput{context_version:version,download:sync_preview::DownloadInput{local_id:task.id.clone(),cloud_id:"rec_download".into(),
            subject_id:"cli_download:user_a".into(),expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,
            expected_cloud_modified_at:Some(1792500000000),expected_cloud_fields:fields.clone()}};
        state.begin_feishu_change().unwrap();state.end_feishu_change();
        assert!(state.download_feishu_task(&config,inspection("user_a"),&confirmation).is_err());
        confirmation.context_version=state.feishu_context_version().unwrap();
        assert!(state.download_feishu_task(&config,inspection("user_b"),&confirmation).is_err());
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks,vec![task.clone()],"rejected confirmations must leave local content unchanged");
        let downloaded=state.download_feishu_task(&config,inspection("user_a"),&confirmation).unwrap();
        let current=&downloaded.tasks[0];
        assert_eq!((&current.title,&current.detail,current.progress,current.quadrant.as_str(),current.today,current.start_date.as_deref(),current.planned_date.as_deref()),
            (&"确认下载任务".to_string(),&"来自飞书的详情".to_string(),75,"nn",false,Some("2026-10-21"),Some("2026-10-28")));
        assert!(downloaded.undo_label.is_some());
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,inspection("user_a")).unwrap()).unwrap();
        assert_eq!(preview["comparisons"][0]["baselineEstablished"],true);
        assert_eq!(preview["comparisons"][0]["status"],"equal");
        assert!(preview["comparisons"][0]["lastSuccessfulSyncAt"].as_i64().unwrap()>0);
        let undone=state.with(|db|db.undo()).unwrap().tasks.remove(0);
        assert_eq!((&undone.title,&undone.detail,undone.progress,&undone.quadrant,undone.today,&undone.start_date,&undone.planned_date),
            (&task.title,&task.detail,task.progress,&task.quadrant,task.today,&task.start_date,&task.planned_date));
    }
    #[test]
    fn formal_import_cloud_only_task_requires_fresh_confirmation_and_prevents_duplicates() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let config=crate::feishu::FeishuConfig{app_id:"cli_import".into(),app_token:"base_import".into(),table_id:"tbl_import".into(),secret_configured:false};
        let fields=serde_json::json!({"title":"飞书已有任务","detail":"完整详情","progress":60,"quadrant":"in","today":true,"startDate":"2026-10-21","plannedDate":"2026-10-28"});
        let inspection=|subject:&str,content:serde_json::Value|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"测试账号".into(),
            subject_id:format!("cli_import:{subject}"),app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),
            checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],records:vec![crate::feishu::SyncRecordTime{
                record_id:"rec_import".into(),title:"飞书已有任务".into(),progress:60,status:"进行中".into(),planned_date:Some("2026-10-28".into()),
                modified_at:Some(1792500000000),time_source:"platformSystem".into(),shared_fields:content}]};
        let mut confirmation=FormalImportInput{context_version:state.feishu_context_version().unwrap(),import:sync_preview::ImportInput{
            cloud_id:"rec_import".into(),subject_id:"cli_import:user_a".into(),expected_cloud_modified_at:Some(1792500000000),
            expected_cloud_fields:fields.clone(),confirm_import:false}};
        assert!(state.import_feishu_task(&config,inspection("user_a",fields.clone()),&confirmation).is_err());
        assert!(state.with(|db|db.snapshot()).unwrap().tasks.is_empty());
        confirmation.import.confirm_import=true;
        state.begin_feishu_change().unwrap();state.end_feishu_change();
        assert!(state.import_feishu_task(&config,inspection("user_a",fields.clone()),&confirmation).is_err());
        confirmation.context_version=state.feishu_context_version().unwrap();
        assert!(state.import_feishu_task(&config,inspection("user_b",fields.clone()),&confirmation).is_err());
        let mut changed=fields.clone();changed["progress"]=serde_json::json!(75);
        assert!(state.import_feishu_task(&config,inspection("user_a",changed),&confirmation).is_err());
        let imported=state.import_feishu_task(&config,inspection("user_a",fields.clone()),&confirmation).unwrap();
        assert_eq!(imported.tasks.len(),1);
        let task=&imported.tasks[0];
        assert_eq!((&task.title,&task.detail,task.progress,task.quadrant.as_str(),task.today,task.start_date.as_deref(),task.planned_date.as_deref()),
            (&"飞书已有任务".to_string(),&"完整详情".to_string(),60,"in",true,Some("2026-10-21"),Some("2026-10-28")));
        assert!(imported.undo_label.as_deref().unwrap_or("").contains("飞书导入"));
        assert!(state.import_feishu_task(&config,inspection("user_a",fields.clone()),&confirmation).is_err());
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks.len(),1);
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,inspection("user_a",fields)).unwrap()).unwrap();
        assert_eq!(preview["existingLinks"].as_array().unwrap().len(),1);
        assert_eq!(preview["comparisons"][0]["status"],"equal");
        let undone=state.with(|db|db.undo()).unwrap();
        assert_eq!(undone.tasks.len(),1);
        assert!(undone.tasks[0].trashed,"撤销新增任务应将其移入本地回收站");
    }
    #[test]
    fn formal_batch_import_is_atomic_when_later_record_changes() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let config=crate::feishu::FeishuConfig{app_id:"cli_multi".into(),app_token:"base_multi".into(),table_id:"tbl_multi".into(),secret_configured:false};
        let first=serde_json::json!({"title":"任务甲","detail":"甲的详情","progress":20,"quadrant":"iu","today":true,"startDate":null,"plannedDate":"2026-10-28"});
        let second=serde_json::json!({"title":"任务乙","detail":"乙的详情","progress":80,"quadrant":"nn","today":false,"startDate":null,"plannedDate":"2026-10-29"});
        let inspect=|second_fields:serde_json::Value|crate::feishu::SyncInspection{read_only:true,complete:true,account_name:"测试账号".into(),subject_id:"cli_multi:user_a".into(),
            app_id:config.app_id.clone(),app_token:config.app_token.clone(),table_id:config.table_id.clone(),checked_at:1792600000000,conditional_write_verified:false,warnings:vec![],
            records:vec![crate::feishu::SyncRecordTime{record_id:"rec_a".into(),title:"任务甲".into(),progress:20,status:"进行中".into(),planned_date:Some("2026-10-28".into()),modified_at:Some(1792500000000),time_source:"platformSystem".into(),shared_fields:first.clone()},
                crate::feishu::SyncRecordTime{record_id:"rec_b".into(),title:"任务乙".into(),progress:80,status:"进行中".into(),planned_date:Some("2026-10-29".into()),modified_at:Some(1792500000001),time_source:"platformSystem".into(),shared_fields:second_fields}]};
        let entry=|cloud_id:&str,modified_at:i64,fields:serde_json::Value|sync_preview::ImportInput{cloud_id:cloud_id.into(),subject_id:"cli_multi:user_a".into(),expected_cloud_modified_at:Some(modified_at),expected_cloud_fields:fields,confirm_import:true};
        let mut input=FormalBatchImportInput{context_version:state.feishu_context_version().unwrap(),confirm_import:false,
            imports:vec![entry("rec_a",1792500000000,first.clone()),entry("rec_b",1792500000001,second.clone())]};
        assert!(state.import_feishu_tasks(&config,inspect(second.clone()),&input).is_err());
        input.confirm_import=true;
        let mut changed=second.clone();changed["progress"]=serde_json::json!(75);
        assert!(state.import_feishu_tasks(&config,inspect(changed),&input).is_err());
        assert!(state.with(|db|db.snapshot()).unwrap().tasks.is_empty(),"a stale later record must roll back the whole batch");
        let imported=state.import_feishu_tasks(&config,inspect(second.clone()),&input).unwrap();
        assert_eq!(imported.tasks.len(),2);
        assert!(imported.tasks.iter().any(|t|t.title=="任务甲"));
        assert!(imported.tasks.iter().any(|t|t.title=="任务乙"));
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,inspect(second.clone())).unwrap()).unwrap();
        assert_eq!(preview["existingLinks"].as_array().unwrap().len(),2);
        assert!(state.import_feishu_tasks(&config,inspect(second.clone()),&input).is_err(),"cannot create duplicate local copies");
        assert_eq!(state.with(|db|db.snapshot()).unwrap().tasks.len(),2);
        let rollback=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let existing=FormalImportInput{context_version:rollback.feishu_context_version().unwrap(),import:entry("rec_b",1792500000001,second.clone())};
        rollback.import_feishu_task(&config,inspect(second.clone()),&existing).unwrap();
        let rollback_batch=FormalBatchImportInput{context_version:rollback.feishu_context_version().unwrap(),confirm_import:true,
            imports:vec![entry("rec_a",1792500000000,first.clone()),entry("rec_b",1792500000001,second.clone())]};
        assert!(rollback.import_feishu_tasks(&config,inspect(second),&rollback_batch).is_err());
        let remaining=rollback.with(|db|db.snapshot()).unwrap();
        assert_eq!(remaining.tasks.len(),1,"a duplicate second entry must roll back the first insertion");
        assert_eq!(remaining.tasks[0].title,"任务乙");
    }
    #[test]
    fn formal_pairing_requires_current_account_confirmation_and_never_changes_task_content() {
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let task=state.with(|db| db.save(input(None,25))).unwrap().tasks.remove(0);
        let before=state.with(|db| db.snapshot()).unwrap();
        let config=crate::feishu::FeishuConfig{app_id:"cli_pair".into(),app_token:"base_pair".into(),table_id:"tbl_pair".into(),secret_configured:false};
        let inspection=|user:&str|crate::feishu::SyncInspection{
            read_only:true,complete:true,account_name:"同名账号".into(),subject_id:format!("cli_pair:{user}"),
            app_id:"cli_pair".into(),app_token:"base_pair".into(),table_id:"tbl_pair".into(),checked_at:1792500000000,
            records:vec![crate::feishu::SyncRecordTime{shared_fields:serde_json::json!({"progress":75}),record_id:"rec_pair".into(),
                title:task.title.clone(),progress:75,status:"进行中".into(),planned_date:None,modified_at:Some(1792400000000),time_source:"platform".into()}],
            conditional_write_verified:false,warnings:vec![],
        };
        let mut selection=FormalPairingInput{context_version:state.feishu_context_version().unwrap(),subject_id:"cli_pair:user_a".into(),
            pairing:sync_preview::SavePairingInput{local_id:task.id.clone(),cloud_id:"rec_pair".into(),
                expected_local_revision:task.revision,expected_local_modified_at:task.content_modified_at,
                expected_cloud_modified_at:Some(1792400000000)}};
        assert!(state.save_feishu_pairing(&config,inspection("user_b"),&selection).is_err());
        state.begin_feishu_change().unwrap();
        assert!(state.save_feishu_pairing(&config,inspection("user_a"),&selection).is_err(),"cannot confirm while authorization is changing");
        state.end_feishu_change();
        assert!(state.save_feishu_pairing(&config,inspection("user_a"),&selection).is_err(),"old confirmation cannot revive after change");
        selection.context_version=state.feishu_context_version().unwrap();
        let saved=serde_json::to_value(state.save_feishu_pairing(&config,inspection("user_a"),&selection).unwrap()).unwrap();
        for flag in ["baselineEstablished","successfulSyncRecorded","localTaskChanged","cloudRecordChanged"] {assert_eq!(saved[flag],false);}
        let own=serde_json::to_value(state.preview_feishu_connection(&config,inspection("user_a")).unwrap()).unwrap();
        assert_eq!(own["existingLinks"].as_array().unwrap().len(),1);
        let other=serde_json::to_value(state.preview_feishu_connection(&config,inspection("user_b")).unwrap()).unwrap();
        assert_eq!(other["existingLinks"],serde_json::json!([]));
        let after=state.with(|db| db.snapshot()).unwrap();
        assert_eq!(after.tasks,before.tasks);assert_eq!(after.undo_label,before.undo_label);
    }
    #[test]
    fn formal_read_preview_accepts_only_complete_user_evidence_for_the_selected_target() {
        let config=crate::feishu::FeishuConfig{app_id:"cli_read_context".into(),app_token:"base_read_context".into(),table_id:"tbl_read_context".into(),secret_configured:false};
        let inspection=||crate::feishu::SyncInspection{
            read_only:true,complete:true,account_name:"同名账号".into(),subject_id:"cli_read_context:user_a".into(),
            app_id:"cli_read_context".into(),app_token:"base_read_context".into(),table_id:"tbl_read_context".into(),
            checked_at:1792500000000,records:vec![],conditional_write_verified:false,warnings:vec![],
        };
        let state=LocalWorkspaceState::new(PathBuf::from(":memory:"));
        let preview=serde_json::to_value(state.preview_feishu_connection(&config,inspection()).unwrap()).unwrap();
        assert_eq!(preview["kind"],"pairingPreview");
        assert_eq!(preview["subjectId"],"cli_read_context:user_a");
        assert_eq!(preview["requiresExplicitSelection"],true);
        assert_eq!(preview["existingLinks"],serde_json::json!([]));
        for case in ["incomplete","non-readonly","wrong-app","wrong-base","wrong-table","foreign-subject","empty-subject"] {
            let mut read=inspection();
            match case {
                "incomplete"=>read.complete=false,"non-readonly"=>read.read_only=false,
                "wrong-app"=>read.app_id="cli_other".into(),"wrong-base"=>read.app_token="base_other".into(),
                "wrong-table"=>read.table_id="tbl_other".into(),"foreign-subject"=>read.subject_id="cli_other:user_a".into(),
                _=>read.subject_id="cli_read_context:".into(),
            }
            assert!(state.preview_feishu_connection(&config,read).is_err(),"{case} must not reach a formal preview");
        }
    }
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    #[test]
    fn direct_portable_preflight_does_not_create_missing_database_or_panic() {
        let folder = std::env::temp_dir().join(format!("direct-preflight-{}-{}", std::process::id(), content_now().unwrap()));
        let result = LocalWorkspaceState::prepared_direct_from_exe(&folder.join("acceptance.exe"));
        assert!(matches!(result, Err(ref error) if error.contains("独立新飞书测试库")));
        assert!(!folder.exists(), "missing profile must never be silently initialized");
    }
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    #[test]
    fn direct_portable_database_is_tied_to_exe_not_roaming_or_working_directory() {
        let folder = std::env::temp_dir().join("direct-profile-path-test");
        assert_eq!(direct_acceptance_database_path(&folder.join("acceptance.exe")).unwrap(),
            folder.join("direct-sync-acceptance-20261020").join("local-workspace.sqlite3"));
        assert!(direct_acceptance_database_path(std::path::Path::new("acceptance.exe")).is_err());
    }
    #[cfg(feature = "feishu-direct-sync-ui-acceptance")]
    #[test]
    fn direct_portable_preflight_loads_only_prepared_scope_and_keeps_local_edits() {
        let folder = std::env::temp_dir().join(format!("direct-load-{}-{}", std::process::id(), content_now().unwrap()));
        let exe = folder.join("acceptance.exe");
        let path = direct_acceptance_database_path(&exe).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        {
            let mut db = LocalWorkspace::open(&path).unwrap();
            for (name, progress) in [("A", 25), ("B", 75)] {
                let mut task = input(None, progress);
                task.title = format!("【同步验收{name}】仅临时测试");
                task.detail = format!("DIRECT-20261020-{name}");
                db.save(task).unwrap();
            }
            db.connection.execute("INSERT INTO local_sync_connections VALUES('test','feishu',?1,?2,?3)",
                params![format!("{}:synthetic-test",crate::direct_account_scope::APP_ID),crate::direct_account_scope::APP_TOKEN,crate::direct_account_scope::TABLE_ID]).unwrap();
        }
        let state = LocalWorkspaceState::prepared_direct_from_exe(&exe).unwrap();
        let before = state.with(|db| db.snapshot()).unwrap();
        assert_eq!(before.tasks.len(), 2);
        assert!(before.demo_mode);
        state.with(|db| {
            let mut edit = input(Some(&before.tasks[0]), 45);
            edit.title = before.tasks[0].title.clone();
            edit.detail = before.tasks[0].detail.clone();
            db.save(edit)
        }).unwrap();
        drop(state);
        let state = LocalWorkspaceState::prepared_direct_from_exe(&exe).unwrap();
        assert!(state.with(|db| db.snapshot()).unwrap().tasks.iter().any(|t| t.progress == 45));
        state.with(|db| db.connection.execute("UPDATE local_sync_connections SET subject_id='other-account'", []).map_err(error)).unwrap();
        drop(state);
        assert!(matches!(LocalWorkspaceState::prepared_direct_from_exe(&exe), Err(ref e) if e.contains("连接范围不匹配")));
        {
            let db = LocalWorkspace::open(&path).unwrap();
            db.connection.execute("DELETE FROM local_sync_connections", []).unwrap();
        }
        assert!(matches!(LocalWorkspaceState::prepared_direct_from_exe(&exe), Err(ref e) if e.contains("连接范围不匹配")), "no connection without a restore marker must still be rejected");
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir(path.parent().unwrap()).unwrap();
        std::fs::remove_dir(folder).unwrap();
    }
    #[test]
    fn sync_profile_never_crosses_direct_and_legacy_accounts(){
        assert!(!isolated_sync_profile("app.snaptask.desktop",true));
        let direct=cfg!(feature="feishu-direct-sync-ui-acceptance");
        assert_eq!(isolated_sync_profile(crate::direct_account_scope::SYNC_IDENTIFIER,true),direct);
        assert_eq!(isolated_sync_profile("app.snaptask.desktop.localworkbench.syncs3",true),!direct);
        let c=isolated_sync_config();
        assert_eq!(c.app_id,if direct{crate::direct_account_scope::APP_ID}else{crate::write_acceptance::TEMPLATE_V2_APP_ID});
        assert_eq!(c.app_token,if direct{crate::direct_account_scope::APP_TOKEN}else{crate::write_acceptance::TEMPLATE_V2_FINAL_APP_TOKEN});
        assert_eq!(c.table_id,if direct{crate::direct_account_scope::TABLE_ID}else{crate::write_acceptance::TEMPLATE_V2_FINAL_TABLE_ID});
    }
    fn db() -> LocalWorkspace {
        LocalWorkspace::open(std::path::Path::new(":memory:")).unwrap()
    }
    #[test]
    fn acceptance_cold_start_has_examples_and_undo_fixture() {
        let state =
            LocalWorkspaceState::acceptance(PathBuf::from(":memory:"), false, false).unwrap();
        let snapshot = state.with(|db| db.snapshot()).unwrap();
        assert_eq!(
            snapshot.tasks.len(),
            4,
            "cold-start acceptance must include examples"
        );
        let task = snapshot
            .tasks
            .iter()
            .find(|t| t.title == "【示例】设计任务工作台")
            .unwrap();
        assert_eq!(task.progress, 42);
        let restored = state.with(|db| db.undo()).unwrap();
        assert_eq!(
            restored
                .tasks
                .iter()
                .find(|t| t.id == task.id)
                .unwrap()
                .progress,
            71
        );
    }
    #[test]
    fn acceptance_restart_preserves_changes_and_never_resurrects_examples() {
        let path = std::env::temp_dir().join(format!(
            "local-demo-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let state = LocalWorkspaceState::acceptance(path.clone(), false, false).unwrap();
        let tasks = state.with(|db| db.snapshot()).unwrap().tasks;
        for task in tasks {
            state
                .with(|db| db.trash(&task.id, task.revision, true))
                .unwrap();
        }
        drop(state);
        let reopened = LocalWorkspaceState::acceptance(path.clone(), false, false).unwrap();
        let snapshot = reopened.with(|db| db.snapshot()).unwrap();
        assert_eq!(snapshot.tasks.len(), 4);
        assert!(snapshot.tasks.iter().all(|t| t.trashed));
        assert!(snapshot.demo_mode);
        drop(reopened);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn acceptance_initialization_never_overwrites_existing_tasks() {
        let mut db = db();
        let snapshot = db.save(input(None, 20)).unwrap();
        db.seed_acceptance_examples().unwrap();
        assert_eq!(db.snapshot().unwrap().tasks, snapshot.tasks);
        assert_eq!(db.snapshot().unwrap().undo_label, snapshot.undo_label);
    }
    pub(super) fn input(t: Option<&LocalTask>, progress: i64) -> SaveTask {
        SaveTask {
            id: t.map(|t| t.id.clone()),
            expected_revision: t.map(|t| t.revision),
            title: "本地验收任务".into(),
            detail: "仅测试数据".into(),
            progress,
            quadrant: "iu".into(),
            today: true,
            start_date: Some("2026-10-09".into()),
            planned_date: Some("2026-10-20".into()),
        }
    }
    #[test]
    fn empty_and_progress_rollbacks() {
        let mut d = db();
        assert!(d.snapshot().unwrap().tasks.is_empty());
        let mut t = d.save(input(None, 0)).unwrap().tasks.remove(0);
        for p in [71, 100, 20, 0] {
            t = d.save(input(Some(&t), p)).unwrap().tasks.remove(0);
            assert_eq!(t.progress, p as u8);
        }
        t = d.undo().unwrap().tasks.remove(0);
        assert_eq!(t.progress, 20);
        t = d.undo().unwrap().tasks.remove(0);
        assert_eq!(t.progress, 100);
    }
    #[test]
    fn date_undo_and_stale_edits() {
        let mut d = db();
        let t = d.save(input(None, 20)).unwrap().tasks.remove(0);
        let mut i = input(Some(&t), 20);
        i.planned_date = Some("2026-11-01".into());
        let edited = d.save(i).unwrap().tasks.remove(0);
        assert!(d
            .save(input(Some(&t), 30))
            .unwrap_err()
            .contains("任务已变更"));
        let reverted = d.undo().unwrap().tasks.remove(0);
        assert_eq!(reverted.planned_date, t.planned_date);
        assert!(d.save(input(Some(&edited), 30)).is_err());
    }
    #[test]
    fn trash_restore_and_undo_add_are_recoverable() {
        let mut d = db();
        let t = d.save(input(None, 71)).unwrap().tasks.remove(0);
        let t = d.trash(&t.id, t.revision, true).unwrap().tasks.remove(0);
        assert!(t.trashed);
        assert!(d.save(input(Some(&t), 50)).is_err());
        let t = d.trash(&t.id, t.revision, false).unwrap().tasks.remove(0);
        assert!(!t.trashed);
        assert!(d.undo().unwrap().tasks[0].trashed);
        assert!(!d.undo().unwrap().tasks[0].trashed);
        let t = d.undo().unwrap().tasks.remove(0);
        assert!(t.trashed);
        assert!(d.snapshot().unwrap().undo_label.is_none());
        assert!(!d.trash(&t.id, t.revision, false).unwrap().tasks[0].trashed);
    }
    #[test]
    fn invalid_inputs_do_not_mutate_storage() {
        let mut d = db();
        for p in [-1, 101] {
            assert!(d.save(input(None, p)).is_err());
        }
        let mut i = input(None, 1);
        i.title = " ".into();
        assert!(d.save(i).is_err());
        for date in ["2026-02-29", "2026-10-08", "xxxx-10-12", "2026-13-01"] {
            let mut i = input(None, 1);
            i.planned_date = Some(date.into());
            assert!(d.save(i).is_err());
        }
        assert!(date_valid("2028-02-29"));
        assert!(!date_valid("1900-02-29"));
        assert!(d.snapshot().unwrap().tasks.is_empty());
        assert!(d.undo().is_err());
    }
    #[test]
    fn reads_and_noop_saves_preserve_time_revision_and_history() {
        let mut d = db();
        let t = d.save(input(None, 25)).unwrap().tasks.remove(0);
        assert!(t.content_modified_at.unwrap() > 1_000_000_000_000);
        let history: i64 = d
            .connection
            .query_row("SELECT count(*) FROM local_history", [], |r| r.get(0))
            .unwrap();
        for _ in 0..3 {
            assert_eq!(d.snapshot().unwrap().tasks[0], t);
        }
        assert_eq!(d.save(input(Some(&t), 25)).unwrap().tasks[0], t);
        assert_eq!(
            d.connection
                .query_row("SELECT count(*) FROM local_history", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            history
        );
    }
    #[test]
    fn legacy_json_time_stays_unknown_until_content_changes_and_undo_is_new_edit() {
        let mut d = db();
        let mut t = d.save(input(None, 25)).unwrap().tasks.remove(0);
        let mut json = serde_json::to_value(&t).unwrap();
        json.as_object_mut().unwrap().remove("contentModifiedAt");
        d.connection
            .execute(
                "UPDATE local_tasks SET data=?1 WHERE id=?2",
                params![json.to_string(), t.id],
            )
            .unwrap();
        t = d.snapshot().unwrap().tasks.remove(0);
        assert!(t.content_modified_at.is_none());
        assert!(d.save(input(Some(&t), 25)).unwrap().tasks[0]
            .content_modified_at
            .is_none());
        let edited = d.save(input(Some(&t), 71)).unwrap().tasks.remove(0);
        assert!(edited.content_modified_at.is_some());
        let undone = d.undo().unwrap().tasks.remove(0);
        assert_eq!(undone.progress, 25);
        assert!(undone.content_modified_at.is_some());
        assert!(undone.revision > edited.revision);
    }
    #[test]
    fn failed_history_write_rolls_back_task_and_revision() {
        let mut d = db();
        d.connection.execute_batch("CREATE TRIGGER reject_history BEFORE INSERT ON local_history BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
        assert!(d.save(input(None, 20)).is_err());
        assert!(d.snapshot().unwrap().tasks.is_empty());
        let revision: i64 = d
            .connection
            .query_row("SELECT revision FROM local_meta WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(revision, 0);
    }
    #[test]
    fn restart_and_multi_connection_preserve_history_and_isolation() {
        let path = std::env::temp_dir().join(format!(
            "local-workbench-test-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sentinel = path.with_extension("cloud-sentinel");
        std::fs::write(&sentinel, b"cloud unchanged").unwrap();
        let mut d = LocalWorkspace::open(&path).unwrap();
        let t = d.save(input(None, 100)).unwrap().tasks.remove(0);
        let mut other = LocalWorkspace::open(&path).unwrap();
        d.save(input(Some(&t), 71)).unwrap();
        assert!(other.save(input(Some(&t), 20)).is_err());
        drop(d);
        drop(other);
        let mut reopened = LocalWorkspace::open(&path).unwrap();
        assert_eq!(reopened.snapshot().unwrap().tasks[0].progress, 71);
        assert_eq!(reopened.undo().unwrap().tasks[0].progress, 100);
        assert_eq!(std::fs::read(&sentinel).unwrap(), b"cloud unchanged");
        drop(reopened);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(&sentinel).unwrap();
    }
}
