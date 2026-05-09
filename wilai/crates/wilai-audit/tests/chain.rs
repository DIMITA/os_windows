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

#[test]
fn cross_file_chain_holds_after_rotate() {
    use sha2::{Digest, Sha256};

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();

    let mut w = AuditWriter::open(dir).unwrap();
    for i in 0..2 {
        w.write(WriteRequest {
            session: format!("a{i}"),
            mode: Mode::Normal,
            payload: EntryPayload::SessionStart {
                entry: "test".into(),
                cwd: "/tmp".into(),
                user: "tester".into(),
            },
        })
        .unwrap();
    }

    // Simulate a day rollover by rotating to a deterministic future name.
    w.rotate_to(
        "2099-01-01.jsonl",
        "rotate-test".into(),
        Mode::Normal,
    )
    .unwrap();

    // Read both files and walk the chain end-to-end.
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .collect();
    files.sort();
    assert!(files.len() >= 2, "expected at least 2 jsonl files, got {}", files.len());

    let mut prev = String::from(
        "0000000000000000000000000000000000000000000000000000000000000000",
    );
    let mut count = 0;
    let mut saw_rotate_pre = false;
    let mut saw_rotate_post = false;

    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b != b'\n' {
                continue;
            }
            let line = &bytes[start..i];
            start = i + 1;
            if line.is_empty() {
                continue;
            }
            count += 1;
            let v: serde_json::Value = serde_json::from_slice(line).unwrap();
            assert_eq!(
                v.get("prev_hash").and_then(|s| s.as_str()).unwrap(),
                prev,
                "chain break in {} at entry {}",
                f.display(),
                count
            );
            match v.get("kind").and_then(|s| s.as_str()) {
                Some("system.rotate_pre") => saw_rotate_pre = true,
                Some("system.rotate_post") => saw_rotate_post = true,
                _ => {}
            }
            let mut h = Sha256::new();
            h.update(line);
            prev = hex::encode(h.finalize());
        }
    }

    assert!(saw_rotate_pre, "rotate_pre missing");
    assert!(saw_rotate_post, "rotate_post missing");
}

#[test]
fn signed_entries_verify_and_detect_tamper() {
    use sha2::{Digest, Sha256};

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let key_path = tmp.path().join("audit.ed25519");
    let signer = wilai_audit::sign::generate_to(&key_path).unwrap();

    {
        let mut w = AuditWriter::open_with_signer(dir, Some(signer)).unwrap();
        for i in 0..3 {
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
    }

    let pub_path = tmp.path().join("audit.ed25519.pub");
    let verifier = wilai_audit::AuditVerifier::load(&pub_path).unwrap();

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
    let mut signed_count = 0;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'\n' { continue; }
        let line = &bytes[start..i];
        start = i + 1;
        if line.is_empty() { continue; }

        // Hash chain holds.
        let v: serde_json::Value = serde_json::from_slice(line).unwrap();
        let entry_prev = v.get("prev_hash").and_then(|s| s.as_str()).unwrap();
        assert_eq!(entry_prev, prev, "chain break");

        // Signature is present and valid.
        let (unsigned, sig_hex) = wilai_audit::sign::split_signed_line(line)
            .unwrap()
            .expect("entry should be signed");
        verifier.verify_hex(&unsigned, &sig_hex).unwrap();
        signed_count += 1;

        let mut h = Sha256::new();
        h.update(line);
        prev = hex::encode(h.finalize());
    }
    assert!(signed_count >= 4, "expected genesis + 3 sessions signed");

    // Tamper with one byte; the chain is unchanged but the sig fails.
    let original = std::fs::read(&files[0]).unwrap();
    let mut tampered = original.clone();
    // Find some "session" string and swap a char.
    for (i, w) in tampered.windows(8).enumerate() {
        if w == b"\"session" {
            // mutate the next quote-delimited value's first byte.
            let mut j = i + 8;
            while j < tampered.len() && tampered[j] != b'"' { j += 1; }
            if j + 1 < tampered.len() {
                tampered[j + 1] = if tampered[j + 1] == b'a' { b'b' } else { b'a' };
                break;
            }
        }
    }
    std::fs::write(&files[0], &tampered).unwrap();

    // Re-walk; expect at least one verify failure.
    let bytes = std::fs::read(&files[0]).unwrap();
    let mut found_bad = false;
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'\n' { continue; }
        let line = &bytes[start..i];
        start = i + 1;
        if line.is_empty() { continue; }
        if let Ok(Some((unsigned, sig_hex))) = wilai_audit::sign::split_signed_line(line) {
            if verifier.verify_hex(&unsigned, &sig_hex).is_err() {
                found_bad = true;
                break;
            }
        }
    }
    assert!(found_bad, "tamper not detected by signature");
}
