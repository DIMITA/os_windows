use std::path::PathBuf;
use tempfile::TempDir;
use wilai_audit::AuditWriter;
use wilai_core::Mode;
use wilai_daemon::mode_mgr::ModeManager;

fn audit_dir() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let p = tmp.path().to_path_buf();
    (tmp, p)
}

#[tokio::test]
async fn switches_and_audits() {
    let (_tmp, dir) = audit_dir();
    let writer = AuditWriter::open(&dir).unwrap();
    let (tx, _h) = writer.spawn_task();
    let mode = ModeManager::new(Mode::Normal, tx);

    assert_eq!(mode.current().await, Mode::Normal);
    let change = mode.set(Mode::Pentest, "manual").await.unwrap();
    assert_eq!(change.from, Mode::Normal);
    assert_eq!(change.to, Mode::Pentest);
    assert_eq!(mode.current().await, Mode::Pentest);
}

#[tokio::test]
async fn refuses_exit_with_pentest_in_flight() {
    let (_tmp, dir) = audit_dir();
    let writer = AuditWriter::open(&dir).unwrap();
    let (tx, _h) = writer.spawn_task();
    let mode = ModeManager::new(Mode::Pentest, tx);

    mode.pentest_started();
    let err = mode.set(Mode::Normal, "manual").await.err().unwrap();
    assert!(err.to_string().contains("in flight"));
    assert_eq!(mode.current().await, Mode::Pentest);

    mode.pentest_finished();
    mode.set(Mode::Normal, "manual").await.unwrap();
    assert_eq!(mode.current().await, Mode::Normal);
}

#[tokio::test]
async fn no_op_set_keeps_mode() {
    let (_tmp, dir) = audit_dir();
    let writer = AuditWriter::open(&dir).unwrap();
    let (tx, _h) = writer.spawn_task();
    let mode = ModeManager::new(Mode::Normal, tx);
    mode.set(Mode::Normal, "manual").await.unwrap();
    assert_eq!(mode.current().await, Mode::Normal);
}

#[tokio::test]
async fn subscribers_get_notified() {
    let (_tmp, dir) = audit_dir();
    let writer = AuditWriter::open(&dir).unwrap();
    let (tx, _h) = writer.spawn_task();
    let mode = ModeManager::new(Mode::Normal, tx);

    let mut rx = mode.subscribe().await;
    mode.set(Mode::Pentest, "test").await.unwrap();

    let received = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.from, Mode::Normal);
    assert_eq!(received.to, Mode::Pentest);
}
