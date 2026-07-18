use super::*;
use codex_protocol::models::PermissionProfile;

#[test]
fn fingerprint_is_stable_and_changes_with_content() {
    assert_eq!(fingerprint(b"hello"), fingerprint(b"hello"));
    assert_ne!(fingerprint(b"hello"), fingerprint(b"world"));
}

#[test]
fn file_schema_requires_only_declared_fields() {
    let schema = file_schema(
        [
            ("path", JsonSchema::string(None)),
            ("limit", JsonSchema::integer(None)),
        ],
        vec!["path".to_string()],
    );
    let value = serde_json::to_value(schema).expect("schema should serialize");
    assert_eq!(value["required"], serde_json::json!(["path"]));
    assert_eq!(value["additionalProperties"], serde_json::json!(false));
}

#[test]
fn text_decoder_strips_bom_and_normalizes_crlf() {
    let decoded = decode_text(b"\xEF\xBB\xBFone\r\ntwo".to_vec()).expect("UTF-8 text");
    assert!(decoded.utf8_bom);
    assert!(decoded.had_crlf);
    assert_eq!(decoded.text, "one\ntwo");
    assert_eq!(
        encode_text(
            &decoded.text,
            decoded.encoding,
            decoded.had_crlf,
            decoded.utf8_bom,
        )
        .expect("UTF-8 text should re-encode"),
        b"\xEF\xBB\xBFone\r\ntwo"
    );
}

#[test]
fn text_decoder_round_trips_shift_jis() {
    let bytes = vec![0x93, 0xfa, 0x96, 0x7b, b'\n'];
    let decoded = decode_text(bytes.clone()).expect("Shift_JIS text");
    assert_eq!(decoded.text, "日本\n");
    assert_eq!(
        encode_text(
            &decoded.text,
            decoded.encoding,
            decoded.had_crlf,
            decoded.utf8_bom,
        )
        .expect("Shift_JIS text should re-encode"),
        bytes
    );
}

#[test]
fn text_decoder_rejects_binary_and_utf16() {
    for bytes in [
        b"text\0binary".to_vec(),
        vec![0xff, 0xfe, b'o', 0, b'k', 0],
        vec![0xfe, 0xff, 0, b'o', 0, b'k'],
    ] {
        let Err(error) = decode_text(bytes) else {
            panic!("unsupported text must fail closed");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}

#[test]
fn partial_scan_decodes_only_complete_lines() {
    let decoded = decode_scanned_text(b"one\ntwo".to_vec(), false).expect("complete prefix line");
    assert_eq!(decoded.text, "one\n");
}

#[tokio::test]
async fn bounded_stream_read_stops_at_the_exact_cap() {
    let temp_dir = tempfile::tempdir().expect("temp dir");
    let file_path = temp_dir.path().join("bounded.txt");
    std::fs::write(&file_path, b"one\ntwo\n").expect("fixture write");
    let path = PathUri::from_host_native_path(&file_path).expect("path URI");
    let sandbox = codex_file_system::FileSystemSandboxContext::from_permission_profile(
        PermissionProfile::Disabled,
    );
    let fs = codex_exec_server::LocalFileSystem::unsandboxed();

    let (prefix, reached_eof) = read_file_prefix(&fs, &path, &sandbox, 4)
        .await
        .expect("bounded read");

    assert_eq!(prefix, b"one\n");
    assert!(!reached_eof);
}

#[test]
fn mutation_arguments_obey_the_approximate_token_cap() {
    let payload = ToolPayload::Function {
        arguments: "x".repeat(MAX_MUTATION_TOKEN_BYTES + 1),
    };

    let error = mutation_args::<WriteFileArgs>(&payload, "write_file")
        .expect_err("oversized arguments must fail before parsing");

    assert!(error.to_string().contains("10,000-token limit"));
}

#[test]
fn mutation_arguments_enforce_serialized_byte_boundaries_for_edit_and_write() {
    let write_prefix = r#"{"path":"file.txt","content":""#;
    let suffix = r#""}"#;
    let content_len = MAX_MUTATION_TOKEN_BYTES - write_prefix.len() - suffix.len();
    let at_limit = format!("{write_prefix}{}{suffix}", "x".repeat(content_len));
    let parsed = mutation_args::<WriteFileArgs>(
        &ToolPayload::Function {
            arguments: at_limit,
        },
        "write_file",
    )
    .expect("serialized arguments at the effective cap should parse");
    assert_eq!(parsed.content.len(), content_len);

    for (tool_name, arguments) in [
        (
            "write_file",
            format!(
                "{write_prefix}{}{suffix}",
                "界".repeat(MAX_MUTATION_ARGUMENT_BYTES)
            ),
        ),
        (
            "edit_file",
            serde_json::json!({
                "path": "file.txt",
                "old_string": "x",
                "new_string": "界".repeat(MAX_MUTATION_ARGUMENT_BYTES),
            })
            .to_string(),
        ),
    ] {
        let payload = ToolPayload::Function { arguments };
        let error = if tool_name == "write_file" {
            mutation_args::<WriteFileArgs>(&payload, tool_name).map(|_| ())
        } else {
            mutation_args::<EditFileArgs>(&payload, tool_name).map(|_| ())
        }
        .expect_err("multibyte arguments above both hard limits must be rejected");
        assert!(error.to_string().contains("64 KiB"));
    }
}

#[test]
fn receipt_keys_are_separated_by_environment() {
    let path = PathUri::parse("file:///workspace/file.txt").expect("valid path URI");
    let local = FileReceiptKey {
        environment_id: "local".to_string(),
        path: path.clone(),
    };
    let remote = FileReceiptKey {
        environment_id: "remote".to_string(),
        path,
    };

    assert_ne!(local, remote);
}

#[tokio::test]
async fn observed_ranges_reject_unseen_replacements() {
    let state = Arc::new(Mutex::new(FileToolState::default()));
    let key = FileReceiptKey {
        environment_id: "local".to_string(),
        path: PathUri::parse("file:///workspace/file.txt").expect("valid path URI"),
    };
    let receipt = FileReceipt {
        fingerprint: fingerprint(b"one\ntwo"),
        file_size: 8,
        modified_at_ms: 1,
        full_coverage: false,
        write_eligible: true,
        observed_ranges: vec![(1, 1)],
        step_id: 1,
        last_used: 0,
        accounted_bytes: 0,
    };
    {
        let mut guard = state.lock().await;
        insert_receipt(&mut guard, key.clone(), receipt).expect("receipt should fit");
    }
    let error = validate_observed_ranges(&state, &key, "one\ntwo", "two")
        .await
        .expect_err("line two was not observed");
    assert!(error.to_string().contains("observed read range"));
}

#[tokio::test]
async fn observed_ranges_allow_trailing_newline_after_complete_read() {
    let state = Arc::new(Mutex::new(FileToolState::default()));
    let key = FileReceiptKey {
        environment_id: "local".to_string(),
        path: PathUri::parse("file:///workspace/file.txt").expect("valid path URI"),
    };
    let text = "line1\n";
    let receipt = FileReceipt {
        fingerprint: fingerprint(text.as_bytes()),
        file_size: text.len() as u64,
        modified_at_ms: 1,
        full_coverage: true,
        write_eligible: true,
        observed_ranges: vec![(1, 1)],
        step_id: 1,
        last_used: 0,
        accounted_bytes: 0,
    };
    {
        let mut guard = state.lock().await;
        insert_receipt(&mut guard, key.clone(), receipt).expect("receipt should fit");
    }
    validate_observed_ranges(&state, &key, text, "line1\n")
        .await
        .expect("full-coverage trailing-newline edit must be allowed");

    let partial_state = Arc::new(Mutex::new(FileToolState::default()));
    let partial_receipt = FileReceipt {
        fingerprint: fingerprint(text.as_bytes()),
        file_size: text.len() as u64,
        modified_at_ms: 1,
        full_coverage: false,
        write_eligible: true,
        observed_ranges: vec![(1, 1)],
        step_id: 1,
        last_used: 0,
        accounted_bytes: 0,
    };
    {
        let mut guard = partial_state.lock().await;
        insert_receipt(&mut guard, key.clone(), partial_receipt).expect("receipt should fit");
    }
    validate_observed_ranges(&partial_state, &key, text, "line1\n")
        .await
        .expect("EOF trailing newline must map onto the last lines() row");
}

#[test]
fn receipt_store_evicts_the_least_recently_used_entry() {
    let mut state = FileToolState::default();
    for index in 0..MAX_RECEIPTS {
        let key = FileReceiptKey {
            environment_id: "local".to_string(),
            path: PathUri::parse(&format!("file:///workspace/{index}.txt")).expect("valid path"),
        };
        insert_receipt(
            &mut state,
            key,
            FileReceipt {
                fingerprint: fingerprint(index.to_string().as_bytes()),
                file_size: 1,
                modified_at_ms: 1,
                full_coverage: true,
                write_eligible: true,
                observed_ranges: vec![(1, 1)],
                step_id: index as u64,
                last_used: 0,
                accounted_bytes: 0,
            },
        )
        .expect("receipt should fit");
    }
    let oldest = FileReceiptKey {
        environment_id: "local".to_string(),
        path: PathUri::parse("file:///workspace/0.txt").expect("valid path"),
    };
    let newest = FileReceiptKey {
        environment_id: "local".to_string(),
        path: PathUri::parse("file:///workspace/new.txt").expect("valid path"),
    };
    insert_receipt(
        &mut state,
        newest.clone(),
        FileReceipt {
            fingerprint: fingerprint(b"new"),
            file_size: 1,
            modified_at_ms: 1,
            full_coverage: true,
            write_eligible: true,
            observed_ranges: vec![(1, 1)],
            step_id: 999,
            last_used: 0,
            accounted_bytes: 0,
        },
    )
    .expect("new receipt should fit");
    assert!(!state.receipts.contains_key(&oldest));
    assert!(state.receipts.contains_key(&newest));
    assert_eq!(state.receipts.len(), MAX_RECEIPTS);
}

#[test]
fn receipt_range_limits_invalidate_oversized_entries_and_bound_global_accounting() {
    let key = FileReceiptKey {
        environment_id: "local".to_string(),
        path: PathUri::parse("file:///workspace/ranges.txt").expect("valid path"),
    };
    let receipt = |ranges: Vec<(usize, usize)>| FileReceipt {
        fingerprint: fingerprint(b"content"),
        file_size: 7,
        modified_at_ms: 1,
        full_coverage: false,
        write_eligible: true,
        observed_ranges: ranges,
        step_id: 1,
        last_used: 0,
        accounted_bytes: 0,
    };
    let mut state = FileToolState::default();
    insert_receipt(&mut state, key.clone(), receipt(vec![(1, 1)]))
        .expect("initial receipt should fit");
    let error = insert_receipt(
        &mut state,
        key.clone(),
        receipt(vec![(1, 1); MAX_RECEIPT_RANGES_PER_ENTRY + 1]),
    )
    .expect_err("oversized range set must fail closed");
    assert!(error.to_string().contains("too many observed ranges"));
    assert!(!state.receipts.contains_key(&key));
    assert_eq!((state.total_ranges, state.total_bytes), (0, 0));

    let ranges = (0..MAX_RECEIPT_RANGES_PER_ENTRY)
        .map(|index| (index * 2 + 1, index * 2 + 1))
        .collect::<Vec<_>>();
    for index in 0..=MAX_RECEIPT_RANGES / MAX_RECEIPT_RANGES_PER_ENTRY {
        let key = FileReceiptKey {
            environment_id: "local".to_string(),
            path: PathUri::parse(&format!("file:///workspace/ranges-{index}.txt"))
                .expect("valid path"),
        };
        insert_receipt(&mut state, key, receipt(ranges.clone())).expect("receipt should fit");
    }
    assert_eq!(state.total_ranges, MAX_RECEIPT_RANGES);
    assert_eq!(
        state.receipts.len(),
        MAX_RECEIPT_RANGES / MAX_RECEIPT_RANGES_PER_ENTRY
    );
}

#[test]
fn receipt_byte_accounting_never_exceeds_the_store_cap() {
    let mut state = FileToolState::default();
    for index in 0..MAX_RECEIPTS {
        let key = FileReceiptKey {
            environment_id: "local".to_string(),
            path: PathUri::parse(&format!(
                "file:///workspace/{index}-{}.txt",
                "x".repeat(3_000)
            ))
            .expect("valid long path"),
        };
        insert_receipt(
            &mut state,
            key,
            FileReceipt {
                fingerprint: fingerprint(b"content"),
                file_size: 7,
                modified_at_ms: 1,
                full_coverage: true,
                write_eligible: true,
                observed_ranges: vec![(1, 1)],
                step_id: index as u64,
                last_used: 0,
                accounted_bytes: 0,
            },
        )
        .expect("bounded store should evict before exceeding its byte cap");
        assert!(state.total_bytes <= MAX_RECEIPT_BYTES);
    }
    assert!(state.receipts.len() < MAX_RECEIPTS);
}

#[tokio::test]
async fn receipt_provenance_rejects_same_step_and_stale_content() {
    let (_session, turn) = crate::session::tests::make_session_and_context().await;
    let turn = Arc::new(turn);
    let read_step = StepContext::for_test(Arc::clone(&turn));
    let later_step = StepContext::for_test(turn);
    let state = Arc::new(Mutex::new(FileToolState::default()));
    let key = FileReceiptKey {
        environment_id: "local".to_string(),
        path: PathUri::parse("file:///workspace/stale.txt").expect("valid path"),
    };
    let metadata = codex_file_system::FileMetadata {
        is_directory: false,
        is_file: true,
        is_symlink: false,
        size: 6,
        created_at_ms: 1,
        modified_at_ms: 2,
    };
    let receipt = FileReceipt {
        fingerprint: fingerprint(b"before"),
        file_size: metadata.size,
        modified_at_ms: metadata.modified_at_ms,
        full_coverage: true,
        write_eligible: true,
        observed_ranges: vec![(1, 1)],
        step_id: read_step.id(),
        last_used: 0,
        accounted_bytes: 0,
    };
    {
        let mut guard = state.lock().await;
        insert_receipt(&mut guard, key.clone(), receipt).expect("receipt should fit");
    }

    let same_step = validate_receipt(&state, &key, b"before", &metadata, &read_step, true)
        .await
        .expect_err("same-step mutation must fail");
    assert!(same_step.to_string().contains("separate completions"));

    let same_mtime_changed_content =
        validate_receipt(&state, &key, b"change", &metadata, &later_step, true)
            .await
            .expect_err("raw SHA-256 must reject same-mtime content changes");
    assert!(
        same_mtime_changed_content
            .to_string()
            .contains("file changed")
    );

    let stale_metadata = codex_file_system::FileMetadata {
        size: 7,
        modified_at_ms: 3,
        ..metadata
    };
    let stale = validate_receipt(&state, &key, b"changed", &stale_metadata, &later_step, true)
        .await
        .expect_err("stale mutation must fail");
    assert!(stale.to_string().contains("read it again"));

    let touched_metadata = codex_file_system::FileMetadata {
        modified_at_ms: 99,
        ..metadata
    };
    validate_receipt(
        &state,
        &key,
        b"before",
        &touched_metadata,
        &later_step,
        true,
    )
    .await
    .expect("mtime-only changes must not invalidate matching raw bytes");
}
