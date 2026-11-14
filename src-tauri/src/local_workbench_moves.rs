//! Targeted, revision-checked moves. Status remains derived from progress.
use super::*;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum MoveKind {
    Status,
    Quadrant,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveTask {
    id: String,
    expected_revision: i64,
    kind: MoveKind,
    target: String,
    progress: Option<i64>,
    confirmed: bool,
}
impl LocalWorkspace {
    pub(super) fn move_task(&mut self, input: MoveTask) -> Result<Snapshot, String> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(error)?;
        let before = get_task(&tx, &input.id)?.ok_or("任务不存在，请刷新")?;
        check_revision(&before, Some(input.expected_revision))?;
        if before.trashed {
            return Err("回收站任务不能拖动，请先恢复".into());
        }
        let mut after = before.clone();
        let label = match input.kind {
            MoveKind::Quadrant => {
                if input.progress.is_some()
                    || !["iu", "in", "un", "nn"].contains(&input.target.as_str())
                {
                    return Err("请选择有效的四象限；分类移动不能修改进度".into());
                }
                after.quadrant = input.target;
                "移动四象限".to_string()
            }
            MoveKind::Status => {
                if !input.confirmed {
                    return Err("请先确认状态移动及进度变化".into());
                }
                let p = input.progress.ok_or("请填写移动后的进度")?;
                let status = match (input.target.as_str(), p) {
                    ("todo", 0) => "未开始",
                    ("done", 100) => "已完成",
                    ("doing", 1..=99) => "进行中",
                    _ => {
                        return Err("未开始须为 0%，已完成须为 100%，进行中须为 1–99 的整数".into())
                    }
                };
                after.progress = p as u8;
                format!("移动状态：{status}")
            }
        };
        if before != after {
            after.revision = next_revision(&tx)?;
            after.content_modified_at = Some(content_now()?);
            put(&tx, &after)?;
            log(&tx, &Some(before), &after, &label)?;
        }
        tx.commit().map_err(error)?;
        self.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    fn db() -> LocalWorkspace {
        LocalWorkspace::open(Path::new(":memory:")).unwrap()
    }
    fn example(db: &mut LocalWorkspace) -> LocalTask {
        db.save(SaveTask {
            id: None,
            expected_revision: None,
            title: "移动测试".into(),
            detail: "保持详情".into(),
            progress: 71,
            quadrant: "in".into(),
            today: true,
            start_date: Some("2026-10-10".into()),
            planned_date: Some("2026-10-20".into()),
        })
        .unwrap()
        .tasks
        .remove(0)
    }
    fn request(t: &LocalTask, kind: MoveKind, target: &str, progress: Option<i64>) -> MoveTask {
        MoveTask {
            id: t.id.clone(),
            expected_revision: t.revision,
            kind,
            target: target.into(),
            progress,
            confirmed: true,
        }
    }
    #[test]
    fn quadrant_move_only_changes_category_and_undo_preserves_all_fields() {
        let mut db = db();
        let t = example(&mut db);
        let moved = db
            .move_task(request(&t, MoveKind::Quadrant, "iu", None))
            .unwrap()
            .tasks
            .remove(0);
        let mut expected = t.clone();
        expected.quadrant = "iu".into();
        expected.revision = moved.revision;
        expected.content_modified_at = moved.content_modified_at;
        assert_eq!(moved, expected);
        let undone = db.undo().unwrap().tasks.remove(0);
        let mut expected = t;
        expected.revision = undone.revision;
        expected.content_modified_at = undone.content_modified_at;
        assert_eq!(undone, expected);
    }
    #[test]
    fn status_moves_complete_reopen_and_reset_without_changing_other_fields() {
        let mut db = db();
        let mut t = example(&mut db);
        for (target, p) in [("done", 100), ("doing", 37), ("todo", 0)] {
            let moved = db
                .move_task(request(&t, MoveKind::Status, target, Some(p)))
                .unwrap()
                .tasks
                .remove(0);
            let mut expected = t.clone();
            expected.progress = p as u8;
            expected.revision = moved.revision;
            expected.content_modified_at = moved.content_modified_at;
            assert_eq!(moved, expected);
            t = moved;
        }
        assert_eq!(db.undo().unwrap().tasks[0].progress, 37);
        assert_eq!(db.undo().unwrap().tasks[0].progress, 100);
        assert_eq!(db.undo().unwrap().tasks[0].progress, 71);
    }
    #[test]
    fn same_group_move_does_not_add_history_or_advance_revision() {
        let mut db = db();
        let t = example(&mut db);
        assert_eq!(
            db.move_task(request(&t, MoveKind::Quadrant, "in", None))
                .unwrap()
                .tasks[0],
            t
        );
        assert_eq!(
            db.move_task(request(&t, MoveKind::Status, "doing", Some(71)))
                .unwrap()
                .tasks[0],
            t
        );
        let count: i64 = db
            .connection
            .query_row("SELECT count(*) FROM local_history", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    #[test]
    fn invalid_targets_progress_and_unconfirmed_moves_never_write() {
        let mut db = db();
        let t = example(&mut db);
        for (target, p) in [
            ("todo", Some(71)),
            ("done", Some(99)),
            ("doing", Some(0)),
            ("doing", Some(100)),
            ("doing", Some(-1)),
            ("doing", None),
            ("evil", Some(20)),
        ] {
            assert!(db
                .move_task(request(&t, MoveKind::Status, target, p))
                .is_err());
        }
        let mut unconfirmed = request(&t, MoveKind::Status, "done", Some(100));
        unconfirmed.confirmed = false;
        assert!(db.move_task(unconfirmed).is_err());
        assert!(db
            .move_task(request(&t, MoveKind::Quadrant, "iu", Some(100)))
            .is_err());
        assert!(db
            .move_task(request(&t, MoveKind::Quadrant, "evil", None))
            .is_err());
        assert_eq!(db.snapshot().unwrap().tasks[0], t);
    }
    #[test]
    fn stale_and_trashed_moves_are_rejected() {
        let mut db = db();
        let t = example(&mut db);
        let moved = db
            .move_task(request(&t, MoveKind::Quadrant, "iu", None))
            .unwrap()
            .tasks
            .remove(0);
        assert!(db
            .move_task(request(&t, MoveKind::Status, "done", Some(100)))
            .is_err());
        let trashed = db
            .trash(&moved.id, moved.revision, true)
            .unwrap()
            .tasks
            .remove(0);
        assert!(db
            .move_task(request(&trashed, MoveKind::Quadrant, "in", None))
            .is_err());
        assert_eq!(db.snapshot().unwrap().tasks[0], trashed);
    }
    #[test]
    fn failed_move_history_rolls_back_task_and_revision() {
        let mut db = db();
        let t = example(&mut db);
        let revision: i64 = db
            .connection
            .query_row("SELECT revision FROM local_meta WHERE id=1", [], |r| {
                r.get(0)
            })
            .unwrap();
        db.connection.execute_batch("CREATE TRIGGER reject_move BEFORE INSERT ON local_history BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
        assert!(db
            .move_task(request(&t, MoveKind::Quadrant, "iu", None))
            .is_err());
        assert_eq!(db.snapshot().unwrap().tasks[0], t);
        assert_eq!(
            db.connection
                .query_row("SELECT revision FROM local_meta WHERE id=1", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            revision
        );
    }
}
