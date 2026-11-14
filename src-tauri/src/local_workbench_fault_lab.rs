//! Synthetic native acceptance driver. No network, credentials, authorization, or cloud writes.
use super::*;
use serde_json::{json, Value};

pub const IDENTIFIER: &str = "app.snaptask.desktop.faultlab.rev34";
const TITLE_PREFIX: &str = "【本机验收";
const DETAIL: &str = "SYNTHETIC-FAULT-LAB：仅本机原生验收，不是真实任务，不访问飞书或凭据。";
struct Scenario {
    mode: String,
    progress: [u8; 2],
    modified: [i64; 2],
}
pub struct FaultLab {
    scenario: Mutex<Scenario>,
}
impl FaultLab {
    fn prepare(path: &std::path::Path) -> Result<(Self, LocalWorkspaceState), String> {
        let fresh = path == std::path::Path::new(":memory:") || !path.exists();
        let lab = Self {
            scenario: Mutex::new(Scenario {
                mode: "ok".into(),
                progress: [60, 80],
                modified: [content_now()? - 1000; 2],
            }),
        };
        let mut db = LocalWorkspace::open(path)?;
        db.demo_mode = true;
        if fresh {
            for (name, progress) in [("A", 20), ("B", 30)] {
                db.save(SaveTask {
                    id: None,
                    expected_revision: None,
                    title: format!("{TITLE_PREFIX}{name}】暂停版本复查"),
                    detail: DETAIL.into(),
                    progress,
                    quadrant: "iu".into(),
                    today: true,
                    start_date: None,
                    planned_date: None,
                })?;
            }
            let read = lab.inspection()?;
            for task in db.snapshot()?.tasks {
                let record = read
                    .records
                    .iter()
                    .find(|r| r.title == task.title)
                    .ok_or("合成目标不完整")?;
                sync_preview::save_pairing(
                    &mut db.connection,
                    &task,
                    record,
                    &read,
                    &SavePairingInput {
                        local_id: task.id.clone(),
                        cloud_id: record.record_id.clone(),
                        expected_local_revision: task.revision,
                        expected_local_modified_at: task.content_modified_at,
                        expected_cloud_modified_at: record.modified_at,
                    },
                )?;
            }
        }
        let tasks = db.snapshot()?.tasks;
        if tasks.len() != 2
            || !tasks
                .iter()
                .all(|t| t.title.starts_with(TITLE_PREFIX) && t.detail == DETAIL)
        {
            return Err("本机验收库必须只含两条明确合成任务；拒绝覆盖现有数据".into());
        }
        let state = LocalWorkspaceState {
            feishu_context: Mutex::new(super::FeishuContext::default()),
            cloud_access: crate::local_cloud_access::CloudAccess::new(path.with_file_name("local-cloud-access.sqlite3")),
            path: path.to_path_buf(),
            database: Mutex::new(Some(db)),
            pending_restore: Mutex::new(None),
            feishu_readonly_enabled: true,
            feishu_pairing_save_enabled: true,
        };
        Ok((lab, state))
    }
    fn set_mode(&self, mode: &str) -> Result<(), String> {
        if !["ok", "offline", "expired", "forbidden"].contains(&mode) {
            return Err("不支持的本机验收场景".into());
        }
        self.scenario.lock().map_err(error)?.mode = mode.into();
        Ok(())
    }
    fn inspection(&self) -> Result<crate::feishu::SyncInspection, String> {
        let scenario = self.scenario.lock().map_err(error)?;
        match scenario.mode.as_str() {
            "offline" => {
                return Err(
                    "[本机合成故障] 无法核实飞书授权账号，请检查网络或重新授权；未发送网络请求"
                        .into(),
                )
            }
            "expired" => {
                return Err("[本机合成故障] 授权已过期，停止本次检查；不会改用应用身份".into())
            }
            "forbidden" => {
                return Err("[本机合成故障] Forbidden（代码 91403）；未改动真实权限".into())
            }
            _ => {}
        }
        let records=["A","B"].iter().enumerate().map(|(n,name)| {
            let title=format!("{TITLE_PREFIX}{name}】暂停版本复查");
            crate::feishu::SyncRecordTime{record_id:format!("recFault{name}"),title:title.clone(),progress:scenario.progress[n],
                shared_fields:json!({"title":title,"detail":DETAIL,"progress":scenario.progress[n],"quadrant":"iu","today":true,"startDate":null,"plannedDate":null}),
                status:"进行中".into(),planned_date:None,modified_at:Some(scenario.modified[n]),time_source:"platformSystem".into()}
        }).collect();
        Ok(crate::feishu::SyncInspection {
            read_only: true,
            complete: true,
            account_name: "本机合成身份（无飞书登录）".into(),
            subject_id: "cli_fault_native:synthetic".into(),
            app_id: "cli_fault_native".into(),
            app_token: "base_fault_native".into(),
            table_id: "tbl_fault_native".into(),
            checked_at: content_now()?,
            records,
            conditional_write_verified: false,
            warnings: vec!["本机合成验收：不连接飞书，不使用账号授权或凭据，不允许上传。".into()],
        })
    }
    fn preview(&self, state: &LocalWorkspaceState) -> Result<PairingPreview, String> {
        let read = self.inspection()?;
        state.with(|db| sync_preview::pairing_preview(&db.connection, &db.snapshot()?.tasks, read))
    }
    fn prepare_batch(
        &self,
        state: &LocalWorkspaceState,
        input: BatchInput,
    ) -> Result<BatchSummary, String> {
        if input.entries.iter().any(|e| e.direction != "cloud") {
            return Err("本机验收只允许合成结果下载，禁止上传".into());
        }
        let read = self.inspection()?;
        state.with(|db| sync_preview::prepare_batch(&mut db.connection, &read, input))
    }
    fn step_batch(&self, state: &LocalWorkspaceState, id: &str) -> Result<BatchSummary, String> {
        let read = match self.inspection() {
            Ok(read) => read,
            Err(reason) => {
                state.with(|db| {
                    sync_preview::verify_batch_target(
                        &db.connection,
                        id,
                        "base_fault_native",
                        "tbl_fault_native",
                    )?;
                    sync_preview::control_batch(&mut db.connection, id, "stop")
                })?;
                return Err(reason);
            }
        };
        state.with(|db| {
            sync_preview::verify_batch_scope(&db.connection, id, &read)?;
            let choice = sync_preview::claim_batch(&mut db.connection, id, &read.subject_id)?
                .ok_or("合成批次已完成")?;
            let outcome = if choice.direction == "cloud" {
                sync_preview::download(&mut db.connection, &read, &choice.input)
                    .map(|_| "completed")
            } else {
                Err("本机验收禁止上传".into())
            };
            sync_preview::finish_batch(&mut db.connection, id, outcome.unwrap_or("blocked"))
        })
    }
    fn change_remaining(
        &self,
        state: &LocalWorkspaceState,
        target: &str,
    ) -> Result<Snapshot, String> {
        if !["local", "cloud"].contains(&target) {
            return Err("仅支持改变本地或合成云端版本".into());
        }
        let p = serde_json::to_value(self.preview(state)?).map_err(error)?;
        let job = p["batchJobs"]
            .as_array()
            .and_then(|jobs| {
                jobs.iter().find(|j| {
                    j["completed"] == 1
                        && j["queued"] == 1
                        && [Some("ready"), Some("stopped")].contains(&j["state"].as_str())
                })
            })
            .ok_or("请先建立两条下载批次，并停在已完成1/2的受控边界")?;
        let entry = job["entries"]
            .as_array()
            .and_then(|entries| entries.iter().find(|e| e["state"] == "queued"))
            .ok_or("没有尚未开始的合成条目")?;
        if target == "cloud" {
            let n = match entry["input"]["cloudId"].as_str() {
                Some("recFaultA") => 0,
                Some("recFaultB") => 1,
                _ => return Err("非合成云端目标".into()),
            };
            let mut scenario = self.scenario.lock().map_err(error)?;
            scenario.progress[n] = 65;
            scenario.modified[n] += 1000;
        }
        state.with(|db| {
            if target == "local" {
                let id = entry["input"]["localId"].as_str().ok_or("本地目标缺失")?;
                let t = db
                    .snapshot()?
                    .tasks
                    .into_iter()
                    .find(|t| t.id == id)
                    .ok_or("合成任务不存在")?;
                db.save(SaveTask {
                    id: Some(t.id),
                    expected_revision: Some(t.revision),
                    title: t.title,
                    detail: t.detail,
                    progress: 71,
                    quadrant: t.quadrant,
                    today: t.today,
                    start_date: t.start_date,
                    planned_date: t.planned_date,
                })?;
            }
            db.snapshot()
        })
    }
}

#[tauri::command]
fn local_fault_lab_status(lab: State<'_, FaultLab>) -> Result<Value, String> {
    Ok(
        json!({"profile":IDENTIFIER,"synthetic":true,"networkEnabled":false,"cloudWritingEnabled":false,
        "mode":lab.scenario.lock().map_err(error)?.mode}),
    )
}
#[tauri::command]
fn local_fault_lab_mode(lab: State<'_, FaultLab>, mode: String) -> Result<(), String> {
    lab.set_mode(&mode)
}
#[tauri::command]
fn local_fault_lab_change_remaining(
    lab: State<'_, FaultLab>,
    state: State<'_, LocalWorkspaceState>,
    target: String,
) -> Result<Snapshot, String> {
    lab.change_remaining(&state, &target)
}
#[tauri::command]
fn local_inspect_isolated_feishu_pairing(
    lab: State<'_, FaultLab>,
    state: State<'_, LocalWorkspaceState>,
) -> Result<PairingPreview, String> {
    lab.preview(&state)
}
#[tauri::command]
fn local_prepare_isolated_feishu_batch(
    lab: State<'_, FaultLab>,
    state: State<'_, LocalWorkspaceState>,
    input: BatchInput,
) -> Result<BatchSummary, String> {
    lab.prepare_batch(&state, input)
}
#[tauri::command]
fn local_step_isolated_feishu_batch(
    lab: State<'_, FaultLab>,
    state: State<'_, LocalWorkspaceState>,
    batch_id: String,
) -> Result<BatchSummary, String> {
    lab.step_batch(&state, &batch_id)
}
#[tauri::command]
fn local_control_isolated_feishu_batch(
    lab: State<'_, FaultLab>,
    state: State<'_, LocalWorkspaceState>,
    batch_id: String,
    action: String,
) -> Result<BatchSummary, String> {
    if action == "stop" {
        return state.with(|db| {
            sync_preview::verify_batch_target(
                &db.connection,
                &batch_id,
                "base_fault_native",
                "tbl_fault_native",
            )?;
            sync_preview::control_batch(&mut db.connection, &batch_id, &action)
        });
    }
    let read = lab.inspection()?;
    state.with(|db| {
        sync_preview::verify_batch_scope(&db.connection, &batch_id, &read)?;
        sync_preview::control_batch(&mut db.connection, &batch_id, &action)
    })
}
#[tauri::command]
fn local_download_isolated_feishu_task(
    lab: State<'_, FaultLab>,
    state: State<'_, LocalWorkspaceState>,
    input: DownloadInput,
) -> Result<Snapshot, String> {
    let read = lab.inspection()?;
    state.with(|db| {
        sync_preview::download(&mut db.connection, &read, &input)?;
        db.snapshot()
    })
}

pub(crate) fn run() {
    let context = tauri::generate_context!();
    // Validate before creating any data or entering Tauri's panic-on-setup-error hook.
    let prepare = || -> Result<(FaultLab, LocalWorkspaceState), String> {
        if context.config().identifier != IDENTIFIER {
            return Err("本机故障验收只允许独立rev34标识".into());
        }
        let exe = std::env::current_exe().map_err(error)?;
        let folder = exe.parent().ok_or("程序路径无效")?.join("fault-lab-rev34");
        std::fs::create_dir_all(&folder).map_err(error)?;
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(error)?
            .as_nanos();
        let run = folder.join(format!("run-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&run).map_err(error)?; // Never overwrite or restore another run.
        FaultLab::prepare(&run.join("local-workspace.sqlite3"))
    };
    let (lab, state) = match prepare() {
        Ok(value) => value,
        Err(reason) => {
            show_startup_error(&reason);
            return;
        }
    };
    let result = tauri::Builder::default()
        .manage(lab)
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            local_fault_lab_status,
            local_fault_lab_mode,
            local_fault_lab_change_remaining,
            local_inspect_isolated_feishu_pairing,
            local_prepare_isolated_feishu_batch,
            local_step_isolated_feishu_batch,
            local_control_isolated_feishu_batch,
            local_download_isolated_feishu_task,
            super::local_snapshot,
            super::local_sync_demo,
            super::local_save_task,
            super::local_move_task,
            super::local_set_trashed,
            super::local_undo
        ])
        .run(context);
    if let Err(reason) = result {
        show_startup_error(&reason.to_string());
    }
}
fn show_startup_error(reason: &str) {
    rfd::MessageDialog::new()
        .set_title("本机合成验收启动失败")
        .set_description(format!(
            "{reason}\n没有连接飞书或改动正式任务；请不要删除旧测试库。"
        ))
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_fault_lab_errors_recover_without_replacing_local_tasks() {
        let (lab, state) = FaultLab::prepare(std::path::Path::new(":memory:")).unwrap();
        let before = state.with(|db| db.snapshot().map(|s| s.tasks)).unwrap();
        for mode in ["offline", "expired", "forbidden"] {
            lab.set_mode(mode).unwrap();
            assert!(lab.preview(&state).is_err());
            assert_eq!(
                state.with(|db| db.snapshot().map(|s| s.tasks)).unwrap(),
                before
            );
            lab.set_mode("ok").unwrap();
            let preview = serde_json::to_value(lab.preview(&state).unwrap()).unwrap();
            assert_eq!(preview["comparisons"].as_array().unwrap().len(), 2);
        }
        assert!(lab.set_mode("real-cloud").is_err());
    }
    fn plan(lab: &FaultLab, state: &LocalWorkspaceState) -> BatchInput {
        let p = serde_json::to_value(lab.preview(state).unwrap()).unwrap();
        serde_json::from_value(json!({"entries":p["comparisons"].as_array().unwrap().iter().map(|r|json!({
            "direction":"cloud","input":{"localId":r["localId"],"cloudId":r["cloudId"],"subjectId":r["subjectId"],
                "expectedLocalRevision":r["localRevision"],"expectedLocalModifiedAt":r["localModifiedAt"],
                "expectedCloudModifiedAt":r["cloudModifiedAt"],"expectedCloudFields":r["expectedCloudFields"]}})).collect::<Vec<_>>() })).unwrap()
    }
    #[test]
    fn native_fault_lab_paused_remaining_version_change_blocks_only_the_next_entry() {
        for target in ["local", "cloud"] {
            let (lab, state) = FaultLab::prepare(std::path::Path::new(":memory:")).unwrap();
            let batch = lab.prepare_batch(&state, plan(&lab, &state)).unwrap();
            assert_eq!(lab.step_batch(&state, &batch.id).unwrap().completed, 1);
            lab.change_remaining(&state, target).unwrap();
            let before = state.with(|db| db.snapshot().map(|s| s.tasks)).unwrap();
            let blocked = lab.step_batch(&state, &batch.id).unwrap();
            assert_eq!((blocked.state.as_str(), blocked.completed), ("blocked", 1));
            assert_eq!(
                state.with(|db| db.snapshot().map(|s| s.tasks)).unwrap(),
                before
            );
            assert!(lab.step_batch(&state, &batch.id).is_err());
        }
    }
    #[test]
    fn native_fault_lab_controls_cannot_mutate_without_the_one_of_two_boundary_or_prepare_uploads()
    {
        let (lab, state) = FaultLab::prepare(std::path::Path::new(":memory:")).unwrap();
        let before = state.with(|db| db.snapshot().map(|s| s.tasks)).unwrap();
        for target in ["local", "cloud", "other"] {
            assert!(lab.change_remaining(&state, target).is_err());
        }
        let mut input = plan(&lab, &state);
        input.entries[0].direction = "local".into();
        assert!(lab
            .prepare_batch(&state, input)
            .unwrap_err()
            .contains("禁止上传"));
        assert_eq!(
            state.with(|db| db.snapshot().map(|s| s.tasks)).unwrap(),
            before
        );
        let preview = serde_json::to_value(lab.preview(&state).unwrap()).unwrap();
        assert!(preview["batchJobs"].as_array().unwrap().is_empty());
    }
}
