use super::*;

#[test]
fn findings_are_independent_of_piece_boundaries_and_hold_no_literal() {
    for secret in [
        "AKIAABCDEFGHIJKLMNOP".to_owned(),
        format!("sk-ant-api03-{}", "a".repeat(60)),
        format!("ghp_{}", "b".repeat(36)),
        "-----BEGIN PRIVATE KEY-----\nbody\n-----END PRIVATE KEY-----".into(),
    ] {
        let text = format!("é {secret} ordinary prose");
        let mut whole = Scanner::new();
        whole.push(&text).unwrap();
        let expected = whole.finish().unwrap();
        assert_eq!(expected.len(), 1);
        assert_eq!(expected[0].start, 3);
        assert_eq!(expected[0].end, 3 + secret.len());
        assert_eq!(
            expected[0].fingerprint,
            format!("{:x}", Sha256::digest(secret.as_bytes()))
        );
        assert!(!format!("{expected:?}").contains(&secret));
        let mut pieces = Scanner::new();
        for c in text.chars() {
            pieces.push(c.encode_utf8(&mut [0; 4])).unwrap();
        }
        assert_eq!(pieces.finish().unwrap(), expected);
    }
}

#[test]
fn clean_text_and_large_values_and_overlaps() {
    let mut clean = Scanner::new();
    clean.push("an ordinary message").unwrap();
    assert!(clean.finish().unwrap().is_empty());
    let secret = format!("sk-ant-api03-{}", "a".repeat(100_000));
    let mut scan = Scanner::new();
    scan.push(&format!("Bearer {secret}")).unwrap();
    let findings = scan.finish().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].end, 7 + secret.len());
}

#[test]
fn private_key_continues_across_blocks() {
    let mut scan = Scanner::new();
    scan.push("-----BEGIN PRIVATE KEY-----").unwrap();
    assert_eq!(scan.block_boundary().unwrap().len(), 1);
    scan.push("body\n-----END PRIVATE KEY----- clean").unwrap();
    let findings = scan.finish().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].end, 27 + 30);
}

#[test]
fn generic_assignments_require_entropy_and_deduplicate_named_patterns() {
    let text = "password=aaaaaaaaaaaaaaaaaaaaaaaa token=Ab3dEf7hIj9kLm2nOp4qRs6t api_key=sk-ant-api03-abcdefghijklmnopqrstuvwxyz012345";
    let mut scan = Scanner::new();
    for c in text.chars() {
        scan.push(c.encode_utf8(&mut [0; 4])).unwrap();
    }
    let findings = scan.finish().unwrap();
    assert_eq!(findings.len(), 2);
    assert_eq!(findings[0].pattern, "Generic credential assignment");
    assert_eq!(findings[1].pattern, "Anthropic key");
}

#[test]
fn assignment_labels_follow_word_boundaries_and_restart_after_incomplete_labels() {
    let value = "Ab3dEf7hIj9kLm2nOp4qRs6t";
    for (label, count) in [
        ("épassword", 0),
        ("-password", 1),
        ("token token", 1),
        ("api-key", 1),
    ] {
        let mut scan = Scanner::new();
        scan.push(&format!("{label}={value}")).unwrap();
        assert_eq!(scan.finish().unwrap().len(), count, "{label}");
    }
}
