//! Explicit, per-target cloud-read permission; independent of offline task storage.
use rusqlite::{Connection, OptionalExtension};
use std::{path::PathBuf, sync::Mutex};

pub(crate) struct CloudAccess {
    path: PathBuf,
    gate: Mutex<()>,
}
impl CloudAccess {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            gate: Mutex::new(()),
        }
    }
    fn enabled_unlocked(&self, target: &str) -> Result<bool, String> {
        if !self.path.exists() {
            return Ok(false);
        }
        let db =
            Connection::open_with_flags(&self.path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .map_err(|_| "无法读取本地云端访问开关")?;
        let value: Option<i64> = db
            .query_row(
                "SELECT enabled FROM cloud_access WHERE target=?1",
                [target],
                |r| r.get(0),
            )
            .optional()
            .map_err(|_| "本地云端访问设置无效，已停止检查")?;
        Ok(value == Some(1))
    }
    pub fn enabled(&self, target: &str) -> Result<bool, String> {
        let _guard = self.gate.lock().map_err(|_| "云端访问开关繁忙")?;
        self.enabled_unlocked(target)
    }
    pub fn set_enabled(&self, target: &str, enabled: bool) -> Result<(), String> {
        // Disabling waits for an already-started read; no later read can pass this gate.
        let _guard = self.gate.lock().map_err(|_| "云端访问开关繁忙")?;
        let db = Connection::open(&self.path).map_err(|_| "无法保存本地云端访问开关")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS cloud_access(target TEXT PRIMARY KEY,enabled INTEGER NOT NULL CHECK(enabled IN (0,1)))").map_err(|_| "无法保存本地云端访问开关")?;
        db.execute("INSERT INTO cloud_access(target,enabled) VALUES(?1,?2) ON CONFLICT(target) DO UPDATE SET enabled=excluded.enabled", rusqlite::params![target, i64::from(enabled)]).map_err(|_| "无法保存本地云端访问开关")?;
        Ok(())
    }
    pub fn read<T>(
        &self,
        target: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        let _guard = self.gate.lock().map_err(|_| "云端访问开关繁忙")?;
        if !self.enabled_unlocked(target)? {
            return Err("工作台云端访问已关闭；本地任务仍可独立使用".into());
        }
        action()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn cloud_reads_require_explicit_target_permission_and_disabled_reads_never_dispatch() {
        let path = std::env::temp_dir().join(format!(
            "local-cloud-gate-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gate = CloudAccess::new(path.clone());
        let calls = AtomicUsize::new(0);
        let target = "app-a/base-a/table-a";
        assert!(
            !path.exists(),
            "constructing an offline workspace must not initialize cloud storage"
        );
        assert!(!gate.enabled(target).unwrap());
        assert!(gate
            .read(target, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .is_err());
        gate.set_enabled(target, true).unwrap();
        gate.read(target, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert!(!gate.enabled("app-b/base-a/table-a").unwrap());
        gate.set_enabled(target, false).unwrap();
        assert!(gate
            .read(target, || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        drop(gate);
        let reopened = CloudAccess::new(path.clone());
        assert!(!reopened.enabled(target).unwrap());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn disabling_waits_for_the_active_read_then_blocks_future_dispatch() {
        use std::sync::{mpsc, Arc};
        let path = std::env::temp_dir().join(format!(
            "local-cloud-serial-{}-{}.sqlite3",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let gate = Arc::new(CloudAccess::new(path.clone()));
        gate.set_enabled("target", true).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let reader_gate = gate.clone();
        let reader = std::thread::spawn(move || {
            reader_gate.read("target", || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(42)
            })
        });
        started_rx.recv().unwrap();
        let writer_gate = gate.clone();
        let writer = std::thread::spawn(move || writer_gate.set_enabled("target", false));
        release_tx.send(()).unwrap();
        assert_eq!(reader.join().unwrap().unwrap(), 42);
        writer.join().unwrap().unwrap();
        assert!(gate
            .read("target", || -> Result<(), String> {
                panic!("closed permission dispatched a request")
            })
            .is_err());
        std::fs::remove_file(path).unwrap();
    }
}
