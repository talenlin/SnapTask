//! Full core + real HTTP adapters; only the external Feishu server is a loopback fixture.
use super::*;
use serde_json::{json, Value};

const TOKEN: &str = "synthetic-loopback-only";
const NOW: i64 = 1_800_000_000_000;
fn config() -> crate::feishu::FeishuConfig {
    crate::feishu::FeishuConfig {
        app_id: crate::direct_account_scope::APP_ID.into(),
        app_token: crate::direct_account_scope::APP_TOKEN.into(),
        table_id: crate::direct_account_scope::TABLE_ID.into(),
        secret_configured: false,
    }
}
fn schema() -> Value {
    json!({"code":0,"msg":"ok","data":{"has_more":false,"items":[
        {"field_name":"任务名称","type":1},{"field_name":"任务详情","type":1},
        {"field_name":"进度","type":2},{"field_name":"任务类别","type":3},
        {"field_name":"今日必完成","type":7},{"field_name":"开始日期","type":5},
        {"field_name":"计划完成日期","type":5},{"field_name":"状态","type":20}]}})
}
fn snapshot(progress: u8) -> Vec<(u16, Option<String>)> {
    vec![
        (
            200,
            Some(
                json!({"code":0,"msg":"ok","data":{"open_id":"synthetic-user","name":"loopback"}})
                    .to_string(),
            ),
        ),
        (200, Some(schema().to_string())),
        (
            200,
            Some(
                json!({"code":0,"msg":"ok","data":{"has_more":false,"items":[{
            "record_id":crate::direct_account_scope::RECORD_IDS[0],
            "last_modified_time":if progress==60 {NOW-1000} else {NOW},
            "fields":{"任务名称":"loopback-task","任务详情":"synthetic-only","进度":progress,
                "任务类别":"重要且紧急","今日必完成":true,"状态":"进行中"}}]}})
                .to_string(),
            ),
        ),
    ]
}
fn fixture(
    root: &str,
    path: &std::path::Path,
) -> (LocalWorkspace, crate::feishu::SyncInspection, DownloadInput) {
    let read = crate::feishu::sync_test_inspect_at(root, &config(), TOKEN, NOW).unwrap();
    let mut db = LocalWorkspace::open(path).unwrap();
    let mut input = super::tests::input(None, 42);
    input.title = "loopback-task".into();
    input.detail = "synthetic-only".into();
    input.start_date = None;
    input.planned_date = None;
    let local = db.save(input).unwrap().tasks.remove(0);
    sync_preview::save_pairing(
        &mut db.connection,
        &local,
        &read.records[0],
        &read,
        &SavePairingInput {
            local_id: local.id.clone(),
            cloud_id: read.records[0].record_id.clone(),
            expected_local_revision: local.revision,
            expected_local_modified_at: local.content_modified_at,
            expected_cloud_modified_at: read.records[0].modified_at,
        },
    )
    .unwrap();
    let input = DownloadInput {
        local_id: local.id,
        cloud_id: read.records[0].record_id.clone(),
        subject_id: read.subject_id.clone(),
        expected_local_revision: local.revision,
        expected_local_modified_at: local.content_modified_at,
        expected_cloud_modified_at: read.records[0].modified_at,
        expected_cloud_fields: read.records[0].shared_fields.clone(),
    };
    (db, read, input)
}
fn preview(db: &LocalWorkspace, read: crate::feishu::SyncInspection) -> Value {
    serde_json::to_value(
        sync_preview::pairing_preview(&db.connection, &db.snapshot().unwrap().tasks, read).unwrap(),
    )
    .unwrap()
}

#[test]
fn write_transport_denied_after_read_keeps_pending_and_prevents_second_send() {
    for (status, code) in [(401, 99991663), (403, 91403), (200, 91403)] {
        let mut replies = snapshot(60);
        replies.push((200, Some(schema().to_string())));
        replies.push((
            status,
            Some(json!({"code":code,"msg":"synthetic denied"}).to_string()),
        ));
        replies.extend(snapshot(60));
        let (root, server) = crate::feishu::sync_test_server(replies);
        let (mut db, read, input) = fixture(&root, std::path::Path::new(":memory:"));
        let before = db.snapshot().unwrap().tasks;
        let op = sync_preview::begin_upload(&mut db.connection, &read, &input).unwrap();
        // This is the same attempt and settlement path called by native upload_send.
        let attempt = crate::feishu::sync_test_attempt_at(
            &root,
            &config(),
            TOKEN,
            &op.record_id,
            &op.content,
            &op.status,
            NOW,
        );
        let result = settle_upload_attempt(&mut db.connection, &op.id, attempt).unwrap();
        assert_eq!(result.state, "pending");
        assert!(
            sync_preview::begin_upload(&mut db.connection, &read, &input)
                .unwrap_err()
                .contains("待核实")
        );
        assert_eq!(db.snapshot().unwrap().tasks, before);
        let p = preview(&db, read);
        assert_eq!(p["uploadOperations"].as_array().unwrap().len(), 1);
        assert_eq!(p["existingLinks"][0]["baselineEstablished"], false);
        assert_eq!(p["existingLinks"][0]["lastSuccessfulSyncAt"], Value::Null);
        let requests = server.join().unwrap();
        assert_eq!(requests.iter().filter(|r| r.starts_with("PUT ")).count(), 1);
        assert_eq!(requests.len(), 8);
    }
}

#[test]
fn write_transport_preparation_denied_is_the_only_failure_marked_definitely_unsent() {
    let mut replies = snapshot(60);
    replies.push((
        403,
        Some(json!({"code":91403,"msg":"synthetic schema denied before PUT"}).to_string()),
    ));
    let (root, server) = crate::feishu::sync_test_server(replies);
    let (mut db, read, input) = fixture(&root, std::path::Path::new(":memory:"));
    let before = db.snapshot().unwrap().tasks;
    let op = sync_preview::begin_upload(&mut db.connection, &read, &input).unwrap();
    let attempt = crate::feishu::sync_test_attempt_at(
        &root,
        &config(),
        TOKEN,
        &op.record_id,
        &op.content,
        &op.status,
        NOW,
    );
    assert!(attempt.unsent.is_some());
    assert!(settle_upload_attempt(&mut db.connection, &op.id, attempt)
        .unwrap_err()
        .contains("尚未发出写入"));
    assert_eq!(db.snapshot().unwrap().tasks, before);
    let p = preview(&db, read);
    assert_eq!(p["uploadOperations"][0]["state"], "notApplied");
    assert_eq!(p["existingLinks"][0]["baselineEstablished"], false);
    assert_eq!(p["existingLinks"][0]["lastSuccessfulSyncAt"], Value::Null);
    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 4);
    assert!(!requests.iter().any(|r| r.starts_with("PUT ")));
}

#[test]
fn write_transport_lost_response_is_settled_by_readback_never_by_resending() {
    for applied in [false, true] {
        let mut replies = snapshot(60);
        replies.push((200, Some(schema().to_string())));
        replies.push((200, None)); // Server has consumed the full PUT; response is lost.
        replies.extend(snapshot(if applied { 42 } else { 60 }));
        let (root, server) = crate::feishu::sync_test_server(replies);
        let (mut db, read, input) = fixture(&root, std::path::Path::new(":memory:"));
        let before = db.snapshot().unwrap().tasks;
        let op = sync_preview::begin_upload(&mut db.connection, &read, &input).unwrap();
        let attempt = crate::feishu::sync_test_attempt_at(
            &root,
            &config(),
            TOKEN,
            &op.record_id,
            &op.content,
            &op.status,
            NOW,
        );
        let result = settle_upload_attempt(&mut db.connection, &op.id, attempt).unwrap();
        assert_eq!(result.state, if applied { "completed" } else { "pending" });
        if !applied {
            assert!(
                sync_preview::begin_upload(&mut db.connection, &read, &input).is_err(),
                "old read does not authorize replay"
            );
        }
        assert_eq!(
            db.snapshot().unwrap().tasks,
            before,
            "readback never replaces a local task"
        );
        let p = preview(&db, read);
        assert_eq!(p["uploadOperations"].as_array().unwrap().len(), 1);
        assert_eq!(p["existingLinks"][0]["baselineEstablished"], applied);
        assert_eq!(
            p["existingLinks"][0]["lastSuccessfulSyncAt"].is_null(),
            !applied
        );
        let requests = server.join().unwrap();
        let writes = requests
            .iter()
            .filter(|r| r.starts_with("PUT "))
            .collect::<Vec<_>>();
        assert_eq!(writes.len(), 1);
        let body: Value =
            serde_json::from_str(writes[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["fields"]["进度"], 42);
        assert!(
            body["fields"].get("状态").is_none(),
            "formula status is not overwritten"
        );
        assert_eq!(requests.len(), 8);
    }
}

#[test]
fn write_transport_failed_readback_survives_reopen_then_read_only_check_can_complete() {
    let path = std::env::temp_dir().join(format!(
        "project-track-write-transport-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    assert!(!path.exists());
    let mut replies = snapshot(60);
    replies.push((200, Some(schema().to_string())));
    replies.push((200, None));
    replies.push((
        401,
        Some(json!({"code":99991663,"msg":"synthetic expired during readback"}).to_string()),
    ));
    replies.extend(snapshot(42)); // Only consumed by the explicit read-only check after reopen.
    let (root, server) = crate::feishu::sync_test_server(replies);
    let (mut db, read, input) = fixture(&root, &path);
    let before = db.snapshot().unwrap().tasks;
    let op = sync_preview::begin_upload(&mut db.connection, &read, &input).unwrap();
    let attempt = crate::feishu::sync_test_attempt_at(
        &root,
        &config(),
        TOKEN,
        &op.record_id,
        &op.content,
        &op.status,
        NOW,
    );
    assert!(attempt.inspection.is_none());
    assert_eq!(
        settle_upload_attempt(&mut db.connection, &op.id, attempt)
            .unwrap()
            .state,
        "pending"
    );
    drop(db);
    let mut db = LocalWorkspace::open(&path).unwrap();
    assert_eq!(db.snapshot().unwrap().tasks, before);
    // Reopen and failed readback alone do not clear the durable pending guard.
    assert_eq!(
        sync_preview::reconcile_upload(&mut db.connection, &op.id, None)
            .unwrap()
            .state,
        "pending"
    );
    assert!(
        sync_preview::begin_upload(&mut db.connection, &read, &input)
            .unwrap_err()
            .contains("待核实")
    );
    let fresh = crate::feishu::sync_test_inspect_at(&root, &config(), TOKEN, NOW).unwrap();
    assert_eq!(
        sync_preview::reconcile_upload(&mut db.connection, &op.id, Some(&fresh))
            .unwrap()
            .state,
        "completed"
    );
    assert_eq!(db.snapshot().unwrap().tasks, before);
    let p = preview(&db, fresh);
    assert_eq!(p["uploadOperations"].as_array().unwrap().len(), 1);
    assert_eq!(p["existingLinks"][0]["baselineEstablished"], true);
    assert!(!p["existingLinks"][0]["lastSuccessfulSyncAt"].is_null());
    let requests = server.join().unwrap();
    assert_eq!(requests.iter().filter(|r| r.starts_with("PUT ")).count(), 1);
    assert_eq!(requests.len(), 9);
    drop(db);
    std::fs::remove_file(path).unwrap();
}
