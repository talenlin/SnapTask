//! Explicit, user-identity-only floating writes. No acceptance gate is relaxed.
//! Durable intents are separate from cache/restore data. Uncertain attempts never auto-retry.
use super::*;
use rusqlite::{params, Connection};

pub(crate) struct State {
    db: Mutex<Connection>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UndoReceipt {
    intent_id: i64,
    context_version: u64,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Confirmation {
    #[serde(flatten)]
    result: ConfirmResult,
    undo_receipt: UndoReceipt,
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Reconciliation {
    message: String,
    undo_receipt: Option<UndoReceipt>,
}
fn check_receipt(receipt: &UndoReceipt, version: u64, intent_id: i64) -> Result<(), String> {
    if receipt.context_version != version || receipt.intent_id != intent_id {
        return Err("连接或最近写入已变化，旧撤销入口已失效；本次没有写入".into());
    }
    Ok(())
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Intent {
    id: i64,
    scope: String,
    subject: String,
    input: ConfirmInput,
    before: Option<Task>,
    after: Option<Task>,
    before_ids: Vec<String>,
    undo: bool,
    #[serde(default)]
    created_id: Option<String>,
    #[serde(default)]
    deleting: bool,
}
impl State {
    pub(crate) fn open(path: impl AsRef<std::path::Path>) -> Result<Self, String> {
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS intents(id INTEGER PRIMARY KEY AUTOINCREMENT,scope TEXT NOT NULL,subject TEXT NOT NULL,record_id TEXT,state TEXT NOT NULL,body TEXT NOT NULL);").map_err(|e|e.to_string())?;
        Ok(Self { db: Mutex::new(db) })
    }
    fn pending(&self, scope: &str, subject: &str) -> Result<Vec<Intent>, String> {
        self.rows(scope, subject, "uncertain")
    }
    fn rows(&self, scope: &str, subject: &str, status: &str) -> Result<Vec<Intent>, String> {
        let db = self.db.lock().map_err(|_| "写入日志繁忙")?;
        let mut q=db.prepare("SELECT body FROM intents WHERE scope=?1 AND subject=?2 AND state=?3 ORDER BY id DESC").map_err(|e|e.to_string())?;
        let rows = q
            .query_map(params![scope, subject, status], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?;
        rows.map(|r| {
            serde_json::from_str(&r.map_err(|e| e.to_string())?).map_err(|e| e.to_string())
        })
        .collect()
    }
    fn begin(&self, mut intent: Intent) -> Result<Intent, String> {
        let mut db = self.db.lock().map_err(|_| "写入日志繁忙")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM intents WHERE scope=?1 AND subject=?2 AND state='uncertain'",
                params![intent.scope, intent.subject],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if count > 0 {
            return Err("此账号和目标表存在待核实写入，请先只读核实；本次未发送请求".into());
        }
        tx.execute(
            "INSERT INTO intents(scope,subject,state,body) VALUES(?1,?2,'uncertain','')",
            params![intent.scope, intent.subject],
        )
        .map_err(|e| e.to_string())?;
        intent.id = tx.last_insert_rowid();
        tx.execute(
            "UPDATE intents SET body=?1 WHERE id=?2",
            params![
                serde_json::to_string(&intent).map_err(|e| e.to_string())?,
                intent.id
            ],
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(intent)
    }
    fn save(&self, intent: &Intent, status: &str) -> Result<(), String> {
        self.db.lock().map_err(|_|"写入日志繁忙")?.execute("UPDATE intents SET state=?1,body=?2,record_id=?3 WHERE id=?4 AND scope=?5 AND subject=?6",params![status,serde_json::to_string(intent).map_err(|e|e.to_string())?,intent.after.as_ref().map(|t|t.id.as_str()),intent.id,intent.scope,intent.subject]).map_err(|e|e.to_string())?;
        Ok(())
    }
}
pub(crate) fn enabled(identifier: &str) -> bool {
    identifier == "app.snaptask.desktop"
        && !cfg!(any(
            feature = "feishu-write-acceptance",
            feature = "feishu-template-v2-acceptance",
            feature = "feishu-release-ui-acceptance",
            feature = "local-workbench-acceptance",
            feature = "feishu-direct-account-acceptance",
            feature = "local-main-flow-ui-acceptance",
            feature = "local-sync-fault-ui-acceptance"
        ))
}
fn scope(c: &FeishuConfig) -> String {
    serde_json::json!([c.app_id, c.app_token, c.table_id]).to_string()
}
fn same(a: &Task, b: &Task) -> bool {
    a.id == b.id
        && a.title == b.title
        && a.detail == b.detail
        && a.progress == b.progress
        && a.quadrant == b.quadrant
        && a.status == b.status
        && a.today == b.today
        && a.planned_date == b.planned_date
}
fn desired(input: &ConfirmInput, t: &Task) -> bool {
    t.progress == input.progress
        && t.status == desired_status(input.progress)
        && (!input.change_planned_date || t.planned_date == input.planned_date)
        && (input.task_id.as_ref().map_or_else(
            || {
                t.title == input.temporary_title.as_deref().unwrap_or("")
                    && t.detail == input.description
                    && Some(&t.quadrant) == input.quadrant.as_ref()
                    && t.today == input.today
            },
            |id| id == &t.id,
        ))
}
fn verified(intent: &Intent, tasks: &[Task]) -> Option<Task> {
    if intent.input.task_id.is_none() && intent.created_id.is_none() {
        return None;
    } // never guess a created record by name
    let matches: Vec<_> = tasks
        .iter()
        .filter(|t| {
            desired(&intent.input, t)
                && intent.created_id.as_ref().map_or(true, |id| &t.id == id)
                && if intent.input.task_id.is_none() {
                    !intent.before_ids.contains(&t.id)
                        && t.planned_date == intent.input.planned_date
                } else {
                    intent.before.as_ref().is_some_and(|old| {
                        old.title == t.title
                            && old.detail == t.detail
                            && old.quadrant == t.quadrant
                            && old.today == t.today
                            && (intent.input.change_planned_date
                                || old.planned_date == t.planned_date)
                    })
                }
        })
        .collect();
    (matches.len() == 1
        && intent
            .created_id
            .as_ref()
            .map_or(true, |id| &matches[0].id == id))
    .then(|| matches[0].clone())
}
struct Read {
    config: FeishuConfig,
    token: String,
    inspection: feishu::SyncInspection,
    tasks: Vec<Task>,
    version: u64,
}
fn read(app: &AppHandle) -> Result<Read, String> {
    if !enabled(&app.config().identifier) {
        return Err("当前不是正式浮窗写入入口".into());
    }
    let local = app.state::<local_workbench::LocalWorkspaceState>();
    let version = local.feishu_context_version()?;
    let state = app.state::<AppState>();
    let config = {
        let s = state.storage.lock().map_err(|_| "本地设置繁忙")?;
        if active_task_source(&s)? != "feishu" {
            return Err("当前数据源不是飞书，请重新检查".into());
        }
        local_feishu_target(&s)?
    };
    let token = feishu::read_user_access_token(&config.app_id)?; // never tenant/app fallback
    let inspection = feishu::inspect_sync_with_user_token(
        &config,
        &token,
        storage::unix_now().saturating_mul(1000),
    )?;
    let (tasks, _) = feishu::fetch_tasks_with_access_token(&config, &token, storage::unix_now())?;
    local.with_feishu_context(version, || {
        let s = state.storage.lock().map_err(|_| "本地设置繁忙")?;
        if scope(&local_feishu_target(&s)?) != scope(&config) {
            return Err("飞书目标已变化，请重新确认".into());
        }
        Ok(())
    })?;
    Ok(Read {
        config,
        token,
        inspection,
        tasks,
        version,
    })
}
fn recheck(before: &Read, after: &Read) -> Result<(), String> {
    if before.version != after.version
        || scope(&before.config) != scope(&after.config)
        || before.inspection.subject_id != after.inspection.subject_id
    {
        return Err("飞书账号、授权或目标已变化，旧确认失效；未发送请求".into());
    }
    Ok(())
}
fn ask(window: &WebviewWindow, read: &Read, input: &ConfirmInput, undo: bool) -> bool {
    let action = if undo {
        if input.task_id.is_none() {
            "撤销新增：删除本程序创建的记录"
        } else {
            "撤销修改：恢复原进度和日期"
        }
    } else if input.task_id.is_some() {
        "更新已有任务"
    } else {
        "新增任务"
    };
    let description=format!("操作：{action}\n账号：{}\n用户标识：{}\n应用：{}\n目标空间：{}\n目标表：{}\n任务：{}\n进度：{}%\n计划日期：{}\n\n请核对上述账号和目标。仅本次确认有效；写后将回读核实。是否继续？",read.inspection.account_name,read.inspection.subject_id,read.config.app_id,read.config.app_token,read.config.table_id,input.expected_title.as_deref().or(input.temporary_title.as_deref()).unwrap_or("未命名"),input.progress,if input.change_planned_date{input.planned_date.as_deref().unwrap_or("清空")}else{"不修改"});
    rfd::MessageDialog::new()
        .set_parent(window)
        .set_title("核对飞书写入目标")
        .set_description(description)
        .set_buttons(rfd::MessageButtons::YesNo)
        .set_level(rfd::MessageLevel::Warning)
        .show()
        == rfd::MessageDialogResult::Yes
}
fn prepare(app: &AppHandle, input: &ConfirmInput) -> Result<Read, String> {
    validate_confirm_input(input)?;
    let before = read(app)?;
    let state = app.state::<AppState>();
    if !state
        .floating
        .pending(&scope(&before.config), &before.inspection.subject_id)?
        .is_empty()
    {
        return Err("此账号和目标表存在待核实写入，请先只读核实；本次未发送请求".into());
    }
    if let Some(id) = &input.task_id {
        let t = before
            .tasks
            .iter()
            .find(|t| &t.id == id)
            .ok_or("目标任务不存在，请重新分析")?;
        if input.expected_title.as_deref() != Some(t.title.as_str())
            || remote_task_changed(input, t)
        {
            return Err("检测到飞书冲突：任务内容已变化，请刷新后重新分析".into());
        }
    }
    Ok(before)
}
pub(crate) fn confirm(
    window: &WebviewWindow,
    app: &AppHandle,
    input: ConfirmInput,
    habit: Option<HabitLearningInput>,
) -> Result<Confirmation, String> {
    if let Some(h) = &habit {
        if h.phrase.trim().chars().count() < 2
            || !["alias", "correction", "task_type"].contains(&h.kind.as_str())
            || (h.kind == "task_type" && input.task_id.is_some())
        {
            return Err("习惯类型或内容无效".into());
        }
    }
    let state = app.state::<AppState>();
    let _guard = state
        .write_lock
        .try_lock()
        .map_err(|_| "上一笔写入仍在处理中，请勿重复提交")?;
    let before = prepare(app, &input)?;
    if !ask(window, &before, &input, false) {
        return Err("已取消飞书写入；未发送请求".into());
    }
    let current = read(app)?;
    recheck(&before, &current)?;
    let previous = input
        .task_id
        .as_ref()
        .and_then(|id| before.tasks.iter().find(|t| &t.id == id))
        .cloned();
    if let Some(t) = &previous {
        if !current.tasks.iter().any(|now| same(t, now)) {
            return Err("检测到飞书冲突：确认期间任务已变化，本次没有写入".into());
        }
    }
    let prepared = feishu::prepare_floating_mutation(
        &current.config,
        &current.token,
        &current.inspection,
        build_feishu_write_request(&input)?,
    )?;
    let local = app.state::<local_workbench::LocalWorkspaceState>();
    let mut intent = Intent {
        id: 0,
        scope: scope(&current.config),
        subject: current.inspection.subject_id.clone(),
        input: input.clone(),
        before: previous,
        after: None,
        before_ids: current.tasks.iter().map(|t| t.id.clone()).collect(),
        undo: false,
        created_id: None,
        deleting: false,
    };
    local.with_feishu_context(current.version, || {
        let s = state.storage.lock().map_err(|_| "本地设置繁忙")?;
        if active_task_source(&s)? != "feishu" || scope(&local_feishu_target(&s)?) != intent.scope {
            return Err("飞书目标已变化，未写入".into());
        }
        drop(s); // The context fence blocks target changes; do not block cache reads during HTTP.
        intent = state.floating.begin(intent.clone())?;
        match feishu::send_floating_mutation(prepared) {
            Ok(id) => {
                intent.created_id = if input.task_id.is_none() { id } else { None };
                state.floating.save(&intent, "uncertain")?
            }
            Err(e) if e.definite => {
                state.floating.save(&intent, "rejected")?;
                return Err(e.message);
            }
            Err(_) => {}
        }
        Ok(())
    })?;
    let tasks =
        feishu::fetch_tasks_with_access_token(&current.config, &current.token, storage::unix_now())
            .map_err(|_| "飞书请求可能已送达，回读失败；已保留待核实记录，请勿重复提交")?
            .0;
    let task = verified(&intent, &tasks)
        .ok_or("飞书写入结果待核实；已保留记录，请勿重复提交，可先只读核实")?;
    intent.after = Some(task.clone());
    state.floating.save(&intent, "completed")?;
    let cache = local.with_feishu_context(current.version, || {
        let mut s = state.storage.lock().map_err(|_| "本地缓存繁忙")?;
        if scope(&local_feishu_target(&s)?) != intent.scope {
            return Err("当前连接已变化，未覆盖新连接缓存或习惯".into());
        }
        s.replace_remote_tasks(&tasks, "feishu")?;
        if let Some(h) = habit {
            if h.kind == "task_type" {
                s.save_task_type_habit(&h.phrase, &task.title, &task.quadrant)?;
            } else {
                s.save_habit(&h.phrase, &task, &h.kind)?;
            }
            Ok(true)
        } else {
            Ok(false)
        }
    });
    let (habit_saved, habit_warning) = match cache {
        Ok(saved) => (saved, None),
        Err(e) => (false, Some(e)),
    };
    Ok(Confirmation {
        result: ConfirmResult {
            task: Some(task),
            synced: true,
            queued: false,
            habit_saved,
            habit_warning,
        },
        undo_receipt: UndoReceipt {
            intent_id: intent.id,
            context_version: current.version,
        },
    })
}
pub(crate) fn reconcile(app: &AppHandle) -> Result<Reconciliation, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .write_lock
        .try_lock()
        .map_err(|_| "有写入正在处理中")?;
    let r = read(app)?;
    let pending = state
        .floating
        .pending(&scope(&r.config), &r.inspection.subject_id)?;
    let mut resolved = 0;
    for mut intent in pending.iter().cloned() {
        if intent.deleting {
            if deletion_verified(&intent, &r.inspection) {
                state.floating.save(&intent, "deleted")?;
                resolved += 1;
            }
        } else if intent.undo {
            let ok = if let Some(before) = &intent.before {
                r.tasks.iter().any(|t| same(t, before))
            } else {
                intent
                    .after
                    .as_ref()
                    .is_some_and(|a| r.tasks.iter().all(|t| t.id != a.id))
            };
            if ok {
                state.floating.save(&intent, "undone")?;
                resolved += 1;
            }
        } else if let Some(t) = verified(&intent, &r.tasks) {
            intent.after = Some(t);
            state.floating.save(&intent, "completed")?;
            resolved += 1;
        }
    }
    let undo_receipt = if pending.len() == resolved && !pending.iter().any(|i| i.deleting) {
        state
            .floating
            .rows(&scope(&r.config), &r.inspection.subject_id, "completed")?
            .first()
            .map(|i| UndoReceipt {
                intent_id: i.id,
                context_version: r.version,
            })
    } else {
        None
    };
    let cache_warning = if resolved > 0 {
        let local = app.state::<local_workbench::LocalWorkspaceState>();
        local.with_feishu_context(r.version, || {
            let mut s = state.storage.lock().map_err(|_| "本地缓存繁忙")?;
            if active_task_source(&s)? != "feishu"
                || scope(&local_feishu_target(&s)?) != scope(&r.config)
            {
                return Err("连接已变化".into());
            }
            s.replace_remote_tasks(&r.tasks, "feishu")
        })
        .err()
    } else {
        None
    };
    Ok(Reconciliation {
        message: format!(
            "只读核实完成：已确认 {resolved} 项，仍有 {} 项待核实；没有重新发送写入请求{}",
            pending.len() - resolved,
            if cache_warning.is_some() { "；本地列表未刷新，请在核对当前连接后手动同步" } else { "" }
        ),
        undo_receipt,
    })
}

fn deletion_verified(intent: &Intent, read: &feishu::SyncInspection) -> bool {
    intent.deleting && read.complete && read.read_only && read.subject_id == intent.subject
        && serde_json::json!([read.app_id,read.app_token,read.table_id]).to_string()==intent.scope
        && intent.before.as_ref().is_some_and(|old| read.records.iter().all(|t| t.record_id != old.id))
}
fn delete_snapshot<'a>(r: &'a Read, expected: &Task) -> Result<i64, String> {
    if !r.tasks.iter().any(|t| same(t, expected)) { return Err("删除冲突：任务已变化或不存在，请刷新后重新选择；未删除".into()); }
    r.inspection.records.iter().find(|t| t.record_id==expected.id).and_then(|t|t.modified_at)
        .ok_or_else(||"无法核实飞书记录修改时间，已停止云端删除；可选择仅从浮窗隐藏".into())
}
pub(crate) fn delete_task(window:&WebviewWindow,app:&AppHandle,expected:Task,expected_target_key:String,context_version:u64)->Result<(),String>{
    let state=app.state::<AppState>();
    let _guard=state.write_lock.try_lock().map_err(|_|"有写入正在处理中，请勿重复操作")?;
    let before=read(app)?;
    if before.version!=context_version||scope(&before.config)!=expected_target_key { return Err("连接已变化，旧删除入口已失效".into()); }
    if !state.floating.pending(&expected_target_key,&before.inspection.subject_id)?.is_empty() { return Err("存在待核实写入，请先只读核实后再删除".into()); }
    let modified=delete_snapshot(&before,&expected)?;
    let message=format!("将删除飞书中的这条记录：\n\n任务：{}\n记录 ID：{}\n账号：{}\n用户标识：{}\n应用：{}\n空间：{}\n表：{}\n\n这会删除云端整条记录及其其他字段内容，本按钮不提供撤销。本地工作台任务不会一起删除。若只想清理浮窗，请选“否”，再选择“仅从浮窗隐藏”。\n\n是否删除？",expected.title,expected.id,before.inspection.account_name,before.inspection.subject_id,before.config.app_id,before.config.app_token,before.config.table_id);
    if rfd::MessageDialog::new().set_parent(window).set_title("确认删除飞书记录").set_description(message).set_level(rfd::MessageLevel::Warning).set_buttons(rfd::MessageButtons::YesNo).show()!=rfd::MessageDialogResult::Yes {return Err("已取消删除，未发送请求".into());}
    let current=read(app)?;
    recheck(&before,&current)?;
    if delete_snapshot(&current,&expected)?!=modified {return Err("删除冲突：确认期间记录被修改，本次未删除".into());}
    let prepared=feishu::prepare_floating_mutation(&current.config,&current.token,&current.inspection,FeishuWriteRequest::Delete{record_id:expected.id.clone()})?;
    let mut intent=Intent{id:0,scope:expected_target_key,subject:current.inspection.subject_id.clone(),input:ConfirmInput{task_id:Some(expected.id.clone()),temporary_title:None,description:String::new(),progress:expected.progress,quadrant:None,today:expected.today,change_planned_date:false,planned_date:None,expected_title:Some(expected.title.clone()),expected_progress:Some(expected.progress),expected_status:Some(expected.status.clone()),expected_planned_date_known:true,expected_planned_date:expected.planned_date.clone()},before:Some(expected),after:None,before_ids:vec![],undo:false,created_id:None,deleting:true};
    let local=app.state::<local_workbench::LocalWorkspaceState>();
    local.with_feishu_context(current.version,||{
        let s=state.storage.lock().map_err(|_|"本地设置繁忙")?;
        if active_task_source(&s)?!="feishu"||scope(&local_feishu_target(&s)?)!=intent.scope {return Err("连接已变化，未删除".into());}drop(s);
        intent=state.floating.begin(intent.clone())?;
        if let Err(e)=feishu::send_floating_mutation(prepared){if e.definite{state.floating.save(&intent,"rejected")?;return Err(e.message);}}
        Ok(())
    })?;
    let tasks=feishu::fetch_tasks_with_access_token(&current.config,&current.token,storage::unix_now()).map_err(|_|"删除结果待核实，请只读核实，勿重复删除")?.0;
    let inspection=feishu::inspect_sync_with_user_token(&current.config,&current.token,storage::unix_now().saturating_mul(1000)).map_err(|_|"删除结果待核实，未核实完整记录列表，请只读核实")?;
    if !deletion_verified(&intent,&inspection){return Err("删除结果待核实，请只读核实，勿重复删除".into());}
    state.floating.save(&intent,"deleted")?;
    // Commit no cache from an obsolete connection; confirmed cloud deletion remains in its journal.
    let _=local.with_feishu_context(current.version,||state.storage.lock().map_err(|_|"本地缓存繁忙")?.replace_remote_tasks(&tasks,"feishu"));
    Ok(())
}
pub(crate) fn undo(
    window: &WebviewWindow,
    app: &AppHandle,
    receipt: UndoReceipt,
) -> Result<UndoResult, String> {
    let state = app.state::<AppState>();
    let _guard = state
        .write_lock
        .try_lock()
        .map_err(|_| "有写入正在处理中")?;
    let before = read(app)?;
    let key = scope(&before.config);
    let subject = &before.inspection.subject_id;
    if !state.floating.pending(&key, subject)?.is_empty() {
        return Err("存在待核实写入，请先只读核实后撤销".into());
    }
    let Some(mut intent) = state
        .floating
        .rows(&key, subject, "completed")?
        .into_iter()
        .next()
    else {
        return Ok(UndoResult {
            undone: false,
            task_title: None,
        });
    };
    check_receipt(&receipt, before.version, intent.id)?;
    let written = intent.after.clone().ok_or("缺少已核实的写入快照")?;
    if !before.tasks.iter().any(|t| same(t, &written)) {
        return Err("撤销冲突：任务写入后又被修改，本次没有覆盖或删除".into());
    }
    let mut undo_input = intent.input.clone();
    undo_input.progress = intent
        .before
        .as_ref()
        .map_or(intent.input.progress, |t| t.progress);
    undo_input.planned_date = intent.before.as_ref().and_then(|t| t.planned_date.clone());
    if !ask(window, &before, &undo_input, true) {
        return Err("已取消撤销，未发送请求".into());
    }
    let current = read(app)?;
    recheck(&before, &current)?;
    if !current.tasks.iter().any(|t| same(t, &written)) {
        return Err("撤销冲突：确认期间记录已变化".into());
    }
    let request = if let Some(old) = &intent.before {
        FeishuWriteRequest::Update {
            record_id: written.id.clone(),
            progress: old.progress,
            status: Some(old.status.clone()),
            planned_date: intent
                .input
                .change_planned_date
                .then(|| old.planned_date.clone()),
        }
    } else {
        FeishuWriteRequest::Delete {
            record_id: written.id.clone(),
        }
    };
    let prepared = feishu::prepare_floating_mutation(
        &current.config,
        &current.token,
        &current.inspection,
        request,
    )?;
    app.state::<local_workbench::LocalWorkspaceState>()
        .with_feishu_context(current.version, || {
            intent.undo = true;
            state.floating.save(&intent, "uncertain")?;
            if let Err(e) = feishu::send_floating_mutation(prepared) {
                if e.definite {
                    intent.undo = false;
                    state.floating.save(&intent, "completed")?;
                    return Err(e.message);
                }
            }
            Ok(())
        })?;
    let tasks =
        feishu::fetch_tasks_with_access_token(&current.config, &current.token, storage::unix_now())
            .map_err(|_| "撤销结果待核实，请勿重复提交")?
            .0;
    let ok = if let Some(old) = &intent.before {
        tasks.iter().any(|t| same(t, old))
    } else {
        tasks.iter().all(|t| t.id != written.id)
    };
    if !ok {
        return Err("撤销结果待核实，请先只读核实".into());
    }
    state.floating.save(&intent, "undone")?;
    Ok(UndoResult {
        undone: true,
        task_title: Some(written.title.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floating_delete_requires_exact_record_absence_and_complete_same_identity_read() {
        let mut i=intent();i.deleting=true;
        let mut r=read_fixture();i.scope=scope(&r.config);i.subject=r.inspection.subject_id.clone();
        assert!(deletion_verified(&i,&r.inspection));
        r.inspection.records.push(feishu::SyncRecordTime{record_id:"recTest".into(),title:String::new(),progress:0,status:String::new(),planned_date:None,shared_fields:serde_json::json!({}),modified_at:Some(1),time_source:"platformSystem".into()});
        assert!(!deletion_verified(&i,&r.inspection),"空标题记录不能被当作已删除");
        r.inspection.records.clear();r.inspection.complete=false;assert!(!deletion_verified(&i,&r.inspection));
        r.inspection.complete=true;r.inspection.subject_id.push('x');assert!(!deletion_verified(&i,&r.inspection));
        let s=State::open(":memory:").unwrap();s.begin(i).unwrap();assert!(s.begin(intent()).is_ok(),"其他身份/目标的日志不应混用");
    }
    #[test]
    fn floating_delete_refuses_unknown_modification_time_and_changed_snapshot() {
        let mut r=read_fixture();assert!(delete_snapshot(&r,&task()).is_err());
        r.inspection.records.push(feishu::SyncRecordTime{record_id:"recTest".into(),title:"fixture".into(),progress:20,status:"进行中".into(),planned_date:None,shared_fields:serde_json::json!({}),modified_at:Some(10),time_source:"platformSystem".into()});
        assert_eq!(delete_snapshot(&r,&task()).unwrap(),10);
        let mut edited=task();edited.title.push('x');assert!(delete_snapshot(&r,&edited).is_err());
        let mut i=intent();i.deleting=true;let s=State::open(":memory:").unwrap();s.begin(i.clone()).unwrap();assert!(s.begin(i).is_err());
        let mut old=serde_json::to_value(intent()).unwrap();old.as_object_mut().unwrap().remove("deleting");assert!(!serde_json::from_value::<Intent>(old).unwrap().deleting);
    }
    #[test]
    fn floating_undo_receipt_rejects_another_operation_or_changed_connection() {
        let receipt = UndoReceipt {
            intent_id: 7,
            context_version: 3,
        };
        assert!(check_receipt(&receipt, 3, 7).is_ok());
        assert!(check_receipt(&receipt, 4, 7).is_err());
        assert!(check_receipt(&receipt, 3, 8).is_err());
    }
    fn task() -> Task {
        Task {
            id: "recTest".into(),
            title: "fixture".into(),
            detail: "detail".into(),
            progress: 70,
            quadrant: "iu".into(),
            status: "进行中".into(),
            today: false,
            planned_date: None,
            updated_at: 0,
        }
    }
    fn intent() -> Intent {
        Intent {
            id: 0,
            scope: "app:base:table".into(),
            subject: "app:user".into(),
            input: ConfirmInput {
                task_id: Some("recTest".into()),
                temporary_title: None,
                description: "fixture".into(),
                progress: 70,
                quadrant: None,
                today: false,
                change_planned_date: false,
                planned_date: None,
                expected_title: Some("fixture".into()),
                expected_progress: Some(20),
                expected_status: Some("进行中".into()),
                expected_planned_date_known: true,
                expected_planned_date: None,
            },
            before: Some(task()),
            after: None,
            before_ids: vec!["recTest".into()],
            undo: false,
            created_id: None,
            deleting: false,
        }
    }
    #[test]
    fn formal_identity_only() {
        assert_eq!(
            enabled("app.snaptask.desktop"),
            !cfg!(any(
                feature = "feishu-write-acceptance",
                feature = "feishu-template-v2-acceptance",
                feature = "feishu-release-ui-acceptance",
                feature = "local-workbench-acceptance",
                feature = "feishu-direct-account-acceptance",
                feature = "local-main-flow-ui-acceptance",
                feature = "local-sync-fault-ui-acceptance"
            ))
        );
        assert!(!enabled("app.snaptask.desktop.acceptance"));
        assert!(!enabled(""));
    }
    #[test]
    fn journal_blocks_repeat_and_separates_target_and_account() {
        let s = State::open(":memory:").unwrap();
        let i = s.begin(intent()).unwrap();
        assert!(s.begin(intent()).is_err());
        assert!(s.pending("another", "app:user").unwrap().is_empty());
        assert!(s.pending(&i.scope, "app:other").unwrap().is_empty());
        let mut i = i;
        i.after = Some(task());
        s.save(&i, "completed").unwrap();
        assert_eq!(s.rows(&i.scope, &i.subject, "completed").unwrap().len(), 1);
        assert!(s.begin(intent()).is_ok());
    }
    #[test]
    fn verification_never_accepts_old_or_guessed_created_records() {
        let mut i = intent();
        i.input.task_id = None;
        i.input.temporary_title = Some("fixture".into());
        i.input.description = "detail".into();
        i.input.quadrant = Some("iu".into());
        assert!(verified(&i, &[task()]).is_none());
        let mut t = task();
        t.id = "recNew".into();
        assert!(verified(&i, &[t.clone()]).is_none());
        i.created_id = Some("recNew".into());
        assert!(verified(&i, &[t.clone()]).is_some());
        let mut other = t.clone();
        other.id = "recOther".into();
        assert!(verified(&i, &[other.clone()]).is_none());
        assert_eq!(verified(&i, &[t, other]).unwrap().id, "recNew");
    }
    #[test]
    fn complete_snapshot_detects_changes_but_ignores_cache_time() {
        let a = task();
        let mut b = a.clone();
        b.updated_at = 999;
        assert!(same(&a, &b));
        b.detail = "changed".into();
        assert!(!same(&a, &b));
        b = a.clone();
        b.progress = 100;
        b.status = "已完成".into();
        assert!(!same(&a, &b));
    }
    fn read_fixture() -> Read {
        Read {
            config: FeishuConfig {
                app_id: "cli_fixture".into(),
                app_token: "base_fixture".into(),
                table_id: "tbl_fixture".into(),
                secret_configured: false,
            },
            token: "synthetic-user-token".into(),
            inspection: feishu::SyncInspection {
                read_only: true,
                complete: true,
                account_name: "fictional".into(),
                subject_id: "cli_fixture:user".into(),
                app_id: "cli_fixture".into(),
                app_token: "base_fixture".into(),
                table_id: "tbl_fixture".into(),
                checked_at: 0,
                records: vec![],
                conditional_write_verified: false,
                warnings: vec![],
            },
            tasks: vec![task()],
            version: 3,
        }
    }
    #[test]
    fn changed_target_subject_or_context_cannot_reuse_confirmation() {
        let a = read_fixture();
        for n in 0..5 {
            let mut b = read_fixture();
            match n {
                0 => b.version += 1,
                1 => b.config.app_id.push('x'),
                2 => b.config.app_token.push('x'),
                3 => b.config.table_id.push('x'),
                _ => b.inspection.subject_id.push('x'),
            };
            assert!(recheck(&a, &b).is_err());
        }
        assert!(recheck(&a, &read_fixture()).is_ok());
    }
    #[test]
    fn unresolved_intents_survive_reopen_and_never_store_credentials() {
        let path = std::env::temp_dir().join(format!(
            "floating-journal-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        {
            let s = State::open(&path).unwrap();
            s.begin(intent()).unwrap();
        }
        let s = State::open(&path).unwrap();
        assert_eq!(s.pending("app:base:table", "app:user").unwrap().len(), 1);
        assert!(s.begin(intent()).is_err());
        drop(s);
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes
            .windows(b"synthetic-user-token".len())
            .any(|b| b == b"synthetic-user-token"));
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn formal_flow_does_not_unlock_legacy_acceptance_writer() {
        let r = read_fixture();
        assert!(crate::write_acceptance::ensure_task_write(&r.config).is_err());
    }
}
