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
        let mut whole = Scanner::new(b"synthetic archive key");
        whole.push(&text).unwrap();
        let expected = whole.finish().unwrap();
        assert_eq!(expected.len(), 1);
        assert_eq!(expected[0].start, 3);
        assert_eq!(expected[0].end, 3 + secret.len());
        assert_eq!(
            expected[0].fingerprint,
            fingerprint(secret.as_bytes(), b"synthetic archive key")
        );
        assert!(!format!("{expected:?}").contains(&secret));
        let mut pieces = Scanner::new(b"synthetic archive key");
        for c in text.chars() {
            pieces.push(c.encode_utf8(&mut [0; 4])).unwrap();
        }
        assert_eq!(pieces.finish().unwrap(), expected);
    }
}

#[test]
fn clean_text_and_large_values_and_overlaps() {
    let mut clean = Scanner::new(b"synthetic archive key");
    clean.push("an ordinary message").unwrap();
    assert!(clean.finish().unwrap().is_empty());
    let secret = format!("sk-ant-api03-{}", "a".repeat(100_000));
    let mut scan = Scanner::new(b"synthetic archive key");
    scan.push(&format!("Bearer {secret}")).unwrap();
    let findings = scan.finish().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].end, 7 + secret.len());
}

#[test]
fn private_key_continues_across_blocks() {
    let mut scan = Scanner::new(b"synthetic archive key");
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
    let mut scan = Scanner::new(b"synthetic archive key");
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
        let mut scan = Scanner::new(b"synthetic archive key");
        scan.push(&format!("{label}={value}")).unwrap();
        assert_eq!(scan.finish().unwrap().len(), count, "{label}");
    }
}

fn fingerprint(text: &[u8], key: &[u8]) -> String {
    let mut hash = Fingerprint::new_from_slice(key).unwrap();
    hash.update(text);
    format!("{:x}", hash.finalize().into_bytes())
}

#[test]
fn fingerprints_use_the_callers_key_for_every_finding_family() {
    for text in ["AKIAABCDEFGHIJKLMNOP", "token=Ab3dEf7hIj9kLm2nOp4qRs6t"] {
        let scan = |key: &[u8]| {
            let mut scanner = Scanner::new(key);
            scanner.push(text).unwrap();
            scanner.finish().unwrap()
        };
        let one = scan(b"first archive");
        let two = scan(b"second archive");
        assert_eq!(one.len(), 1);
        assert_eq!(two.len(), 1);
        assert_ne!(one[0].fingerprint, two[0].fingerprint);
        let found = &one[0];
        assert_eq!(
            found.fingerprint,
            fingerprint(&text.as_bytes()[found.start..found.end], b"first archive")
        );
    }
    assert_eq!(
        fingerprint(b"Hi There", &[0x0b; 20]),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

#[test]
fn quoted_and_prefixed_assignment_labels_work_at_every_split() {
    let value = "Ab3dEf7hIj9kLm2nOp4qRs6t";
    for label in [
        r#""token": ""#,
        "SECRET_TOKEN=",
        "MY_SERVICE_API_KEY=",
        "service-access-key=",
        "secret_token=",
        "unrelated_password=",
        r#""MY_SERVICE_API_KEY": ""#,
        "'SERVICE_TOKEN': '",
    ] {
        let text = format!("{label}{value}\" tail");
        for cut in 0..=text.len() {
            let mut scanner = Scanner::new(b"synthetic archive key");
            scanner.push(&text[..cut]).unwrap();
            scanner.push(&text[cut..]).unwrap();
            let found = scanner.finish().unwrap();
            assert_eq!(found.len(), 1, "{label}, split {cut}");
            assert_eq!(&text[found[0].start..found[0].end], value);
            assert_eq!(
                found[0].fingerprint,
                fingerprint(value.as_bytes(), b"synthetic archive key")
            );
        }
    }
}

#[test]
fn catalog_findings_are_independent_of_every_byte_split() {
    let catalog: serde_json::Value = serde_json::from_str(include_str!(
        "../../tests/fixtures/credential-output/conformance.json"
    ))
    .unwrap();
    let cases = catalog["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 138);
    for case in cases {
        let text: String = case["text_parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|part| part.as_str().unwrap())
            .collect();
        let scan = |cut: usize| {
            let mut scanner = Scanner::new(b"synthetic archive key");
            // A UTF-8 decoder carries an incomplete character into the next piece.
            let mut complete = cut;
            while !text.is_char_boundary(complete) {
                complete -= 1;
            }
            scanner.push(&text[..complete]).unwrap();
            scanner.push(&text[complete..]).unwrap();
            scanner.finish().unwrap()
        };
        let expected = scan(text.len());
        for region in case["regions_utf8"].as_array().unwrap() {
            let start = region[0].as_u64().unwrap() as usize;
            let end = region[1].as_u64().unwrap() as usize;
            assert!(
                expected
                    .iter()
                    .any(|finding| finding.start == start && finding.end == end),
                "{} missing {start}..{end}",
                case["id"]
            );
        }
        for found in &expected {
            assert_eq!(
                found.fingerprint,
                fingerprint(
                    &text.as_bytes()[found.start..found.end],
                    b"synthetic archive key"
                )
            );
        }
        for cut in 0..=text.len() {
            assert_eq!(scan(cut), expected, "{}, split {cut}", case["id"]);
        }
    }
}
