//! Synthetic shell for the production workbench commands; no cloud or credentials.
use crate::{local_workbench::LocalWorkspaceState, AppState, Storage};
use rusqlite::OptionalExtension;
use std::{path::Path, sync::Mutex};
use tauri::{State, WebviewWindow};

pub const IDENTIFIER: &str = "app.snaptask.desktop.mainflow.rev36";
fn prepare(exe: &Path, identifier: &str) -> Result<(AppState, LocalWorkspaceState), String> {
    if identifier != IDENTIFIER || !exe.is_absolute() {
        return Err("主流程验收必须使用独立标识及绝对路径".into());
    }
    let folder = exe.parent().ok_or("程序目录无效")?.join("main-flow-rev36");
    std::fs::create_dir_all(&folder).map_err(|_| "无法创建独立主流程目录")?;
    let path = folder.join("project-track.sqlite3");
    let existed = path.exists();
    if existed {
        let db = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| "独立设置库只读预检失败")?;
        let marker: Option<String> = db
            .query_row(
                "SELECT value FROM app_settings WHERE key='local_main_flow_lab'",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "拒绝初始化没有合成标记的设置库")?;
        if marker.as_deref() != Some("rev36-synthetic-only") {
            return Err("拒绝打开没有合成标记的设置库".into());
        }
        for (key, values) in [
            (
                "feishu.app_id",
                ["", "cli_main_flow_lab_a", "cli_main_flow_lab_b"],
            ),
            (
                "feishu.app_token",
                ["", "base_main_flow_lab_a", "base_main_flow_lab_b"],
            ),
            (
                "feishu.table_id",
                ["", "tbl_main_flow_lab_a", "tbl_main_flow_lab_b"],
            ),
        ] {
            let value: Option<String> = db
                .query_row("SELECT value FROM app_settings WHERE key=?1", [key], |r| {
                    r.get(0)
                })
                .optional()
                .map_err(|_| "合成目标预检失败")?;
            if !values.contains(&value.as_deref().unwrap_or("")) {
                return Err("验收库包含非合成目标，拒绝初始化".into());
            }
        }
    }
    let storage = Storage::open(path).map_err(|_| "无法打开独立主流程设置库")?;
    if existed
        && storage.get_setting("local_main_flow_lab")?.as_deref() != Some("rev36-synthetic-only")
    {
        return Err("拒绝打开没有合成标记的设置库".into());
    }
    storage.set_setting("local_main_flow_lab", "rev36-synthetic-only")?;
    let config = crate::local_feishu_target(&storage)?;
    if !["", "cli_main_flow_lab_a", "cli_main_flow_lab_b"].contains(&config.app_id.as_str()) {
        return Err("验收库包含非合成目标，拒绝启动".into());
    }
    Ok((
        AppState {
            storage: Mutex::new(storage),
            write_lock: Mutex::new(()),
            feishu_auth_flow: Mutex::new(None),
        },
        LocalWorkspaceState::new(folder.join("local-workspace.sqlite3")),
    ))
}
fn select_target(state: &AppState, target: &str) -> Result<(), String> {
    if !["a", "b"].contains(&target) {
        return Err("仅支持本机合成目标A或B".into());
    }
    let storage = state.storage.lock().map_err(|_| "设置繁忙")?;
    storage.set_setting("feishu.app_id", &format!("cli_main_flow_lab_{target}"))?;
    storage.set_setting("feishu.app_token", &format!("base_main_flow_lab_{target}"))?;
    storage.set_setting("feishu.table_id", &format!("tbl_main_flow_lab_{target}"))?;
    Ok(())
}
pub fn synthetic_read(state: &AppState) -> Result<crate::feishu::SyncInspection, String> {
    let storage = state.storage.lock().map_err(|_| "设置繁忙")?;
    let count = storage
        .get_setting("main_flow_mock_reads")?
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    storage.set_setting("main_flow_mock_reads", &(count + 1).to_string())?;
    Err("[本机主流程验收] 已到达只读接口替身；没有联网、没有读取凭据、没有飞书账号结果".into())
}
#[tauri::command]
fn local_main_flow_lab_status(
    window: WebviewWindow,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    if window.label() != "local-workbench" {
        return Err("仅本机验收窗口可用".into());
    }
    let storage = state.storage.lock().map_err(|_| "设置繁忙")?;
    let reads = storage
        .get_setting("main_flow_mock_reads")?
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    Ok(
        serde_json::json!({"profile":IDENTIFIER,"syntheticOnly":true,"networkEnabled":false,"credentialAccess":false,"mockReads":reads}),
    )
}
#[tauri::command]
fn local_main_flow_lab_target(
    window: WebviewWindow,
    state: State<'_, AppState>,
    target: String,
) -> Result<(), String> {
    if window.label() != "local-workbench" {
        return Err("仅本机验收窗口可用".into());
    }
    select_target(&state, &target)
}
pub fn run() {
    let context = tauri::generate_context!();
    let prepared = std::env::current_exe()
        .map_err(|e| e.to_string())
        .and_then(|exe| prepare(&exe, &context.config().identifier));
    let (state, local) = match prepared {
        Ok(value) => value,
        Err(error) => {
            rfd::MessageDialog::new()
                .set_title("主流程验收未启动")
                .set_description(error)
                .show();
            return;
        }
    };
    let result = tauri::Builder::default()
        .manage(state)
        .manage(local)
        .invoke_handler(tauri::generate_handler![
            crate::get_local_feishu_access,
            crate::set_local_feishu_access,
            crate::inspect_local_feishu_sync,
            local_main_flow_lab_status,
            local_main_flow_lab_target,
            crate::local_workbench::local_snapshot,
            crate::local_workbench::local_sync_demo,
            crate::local_workbench::local_save_task,
            crate::local_workbench::local_move_task,
            crate::local_workbench::local_undo
        ])
        .run(context);
    if let Err(error) = result {
        rfd::MessageDialog::new()
            .set_title("主流程验收未启动")
            .set_description(error.to_string())
            .show();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn main_flow_preflight_refuses_an_unmarked_file_without_initializing_it() {
        let folder = std::env::temp_dir().join(format!(
            "main-flow-unmarked-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let data = folder.join("main-flow-rev36");
        std::fs::create_dir_all(&data).unwrap();
        let path = data.join("project-track.sqlite3");
        {
            let db = rusqlite::Connection::open(&path).unwrap();
            db.execute_batch("CREATE TABLE preserve_me(value TEXT); INSERT INTO preserve_me VALUES('do not initialize')").unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(prepare(&folder.join("lab.exe"), IDENTIFIER).is_err());
        assert!(
            std::fs::read(&path).unwrap() == before,
            "refused file was modified"
        );
        std::fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    fn main_flow_shell_has_empty_local_start_and_only_synthetic_targets_persist() {
        let folder = std::env::temp_dir().join(format!(
            "main-flow-shell-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&folder).unwrap();
        let exe = folder.join("lab.exe");
        assert!(prepare(&exe, "app.snaptask.desktop").is_err());
        let (state, local) = prepare(&exe, IDENTIFIER).unwrap();
        let config = crate::local_feishu_target(&state.storage.lock().unwrap()).unwrap();
        let status = crate::local_feishu_access_status(config, &local).unwrap();
        assert!(!status.configured);
        assert!(!status.enabled);
        assert!(!folder
            .join("main-flow-rev36/local-workspace.sqlite3")
            .exists());
        select_target(&state, "a").unwrap();
        let config = crate::local_feishu_target(&state.storage.lock().unwrap()).unwrap();
        let status = crate::local_feishu_access_status(config, &local).unwrap();
        assert_eq!(status.app_id, "cli_main_flow_lab_a");
        assert!(!status.enabled);
        drop((state, local));
        let (state, local) = prepare(&exe, IDENTIFIER).unwrap();
        let config = crate::local_feishu_target(&state.storage.lock().unwrap()).unwrap();
        assert_eq!(config.app_id, "cli_main_flow_lab_a");
        assert!(
            !crate::local_feishu_access_status(config, &local)
                .unwrap()
                .enabled
        );
        assert!(select_target(&state, "real-user").is_err());
        drop((state, local));
        std::fs::remove_dir_all(folder).unwrap();
    }
}
