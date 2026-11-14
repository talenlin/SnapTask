//! Startup-only diagnostics for the independent direct-account acceptance build.
//! No credentials, environment dump, task contents, or network requests are collected.
use std::{
    fs::{File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{atomic::{AtomicBool, Ordering}, Mutex, OnceLock},
};

struct Report { file: File, path: PathBuf }
static REPORT: OnceLock<Mutex<Report>> = OnceLock::new();
static STARTING: AtomicBool = AtomicBool::new(true);

fn line(value: &str) {
    if !STARTING.load(Ordering::SeqCst) { return; }
    if let Some(report) = REPORT.get() {
        if let Ok(mut report) = report.lock() {
            let _ = writeln!(report.file, "{value}");
            let _ = report.file.sync_all();
        }
    }
}

pub fn install() {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis()).unwrap_or_default();
    let name = format!("direct-startup-{}-{now}.log", std::process::id());
    let folder = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    let path = folder.join(name);
    if let Ok(file) = OpenOptions::new().write(true).create_new(true).open(&path) {
        let _ = REPORT.set(Mutex::new(Report { file, path }));
    }
    line("profile=directfeishusync20261020; collection=startup-only");
    stage("entry");
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Startup has no credential or cloud reads. Stop collecting once setup is ready.
        if STARTING.load(Ordering::SeqCst) { line(&format!("startup_panic={info}")); }
        previous(info);
    }));
}

pub fn stage(value: &str) { line(&format!("stage={value}")); }
pub fn database_path(path: &std::path::Path) {
    line(&format!("database_path={}; exists={}", path.display(), path.is_file()));
}
pub fn ready() {
    stage("ready");
    STARTING.store(false, Ordering::SeqCst);
}
pub fn failure(error: &str) {
    line(&format!("startup_error={error}"));
    let path = REPORT.get().and_then(|r| r.lock().ok().map(|r| r.path.clone()));
    let message = match path {
        Some(path) => format!("隔离验收程序启动未完成，未执行云端同步。\n请保留诊断文件：\n{}\n不要删除或重新初始化测试库。", path.display()),
        None => "隔离验收程序启动未完成，无法保存诊断文件。请勿删除或重新初始化测试库。".into(),
    };
    rfd::MessageDialog::new().set_title("隔离验收启动失败")
        .set_description(message).set_level(rfd::MessageLevel::Error).show();
}
