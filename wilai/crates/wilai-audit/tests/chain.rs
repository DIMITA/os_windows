use wilai_audit::entry::EntryPayload;
use wilai_audit::writer::{AuditWriter, WriteRequest};
use wilai_core::Mode;

#[test]
fn writes_chain_and_verifies() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    {
        let mut w = AuditWriter::open(dir).unwrap();
        for i in 0..5 {
            w.write(WriteRequest {
                session: format!("s{i}"),
                mode: Mode::Normal,
                payload: EntryPayload::SessionStart {
                    entry: "test".into(),
                    cwd: "/tmp".into(),
                    user: "tester".into(),
                },
            })
            .unwrap();
        }
        let head = w.running_hash().to_string();
        let seq = w.seq();
        assert!(seq >= 5);
        assert_eq!(head.len(), 64);
    }

    // Verify chain by reading the file.
    use sha2::{Digest, Sha256};
    let files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    assert_eq!(files.len(), 1);

    let bytes = std::fs::read(&files[0]).unwrap();
    let mut prev = String::from(
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut start = 0usize;
    let mut count = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            let line = &bytes[start..i];
            start = i + 1;
            if line.is_empty() {
                continue;
            }
            count += 1;
            let v: serde_json::Value = serde_json::from_slice(line).unwrap();
            let entry_prev = v.get("prev_hash").and_then(|s| s.as_str()).unwrap();
            assert_eq!(entry_prev, prev, "chain break at entry {count}");
            let mut h = Sha256::new();
            h.update(line);
            prev = hex::encode(h.finalize());
        }
    }
    // genesis + 5 sessions
    assert_eq!(count, 6);
}
