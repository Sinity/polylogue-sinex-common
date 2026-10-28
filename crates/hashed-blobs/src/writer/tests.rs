//! Real SIGKILL probes. The durable descriptor file stands in for a product
//! transaction: each test reopens exclusively from the committed descriptor.
use crate::{Extent, Preparation, Reader, SegmentWriter};
use std::{
    fs::File,
    io::{Read, Write},
    process::{Command, Stdio},
};
fn commit(root: &std::path::Path, extent: &Extent) {
    let path = root.join("committed.new");
    let mut file = File::create(&path).unwrap();
    file.write_all(&extent.encode()).unwrap();
    file.sync_all().unwrap();
    std::fs::rename(path, root.join("committed")).unwrap();
    File::open(root).unwrap().sync_all().unwrap();
}
fn stop() {
    unsafe { libc::raise(libc::SIGSTOP) };
    unreachable!("parent kills the stopped worker")
}
#[test]
fn crash_worker() {
    let Some(root) = std::env::var_os("HASHED_BLOBS_CRASH_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let boundary = std::env::var("HASHED_BLOBS_CRASH_BOUNDARY").unwrap();
    let committed = Extent::decode(&std::fs::read(root.join("committed")).unwrap()).unwrap();
    let mut writer =
        SegmentWriter::open(&root, "segment.zst", committed.end(), &[], 0o600).unwrap();
    let p = Preparation::new(root.join("scratch")).unwrap();
    let group = writer
        .append_group(
            vec![p.plain(&b"second committed payload"[..], 3).unwrap()],
            |_| Ok(None),
        )
        .unwrap();

    let group = group.wait().unwrap();

    commit(&root, &group.extents[0]);
    if boundary == "commit" {
        stop();
    }
    writer.acknowledge(&group).unwrap();
    let tail = writer
        .append_group(
            vec![p.plain(&b"third uncommitted payload"[..], 3).unwrap()],
            |_| Ok(None),
        )
        .unwrap()
        .wait()
        .unwrap();
    assert!(tail.end > group.end);
    drop(writer);
    let _writer = SegmentWriter::open(&root, "segment.zst", group.end, &[], 0o600).unwrap();

    panic!("crash boundary was not reached: {boundary}");
}
#[test]
fn crash_leaves_only_a_tail_at_append_sync_commit_and_truncate() {
    for boundary in ["append", "sync", "commit", "truncate"] {
        let dir = tempfile::tempdir().unwrap();
        let p = Preparation::new(dir.path().join("scratch")).unwrap();
        let mut writer = SegmentWriter::open(dir.path(), "segment.zst", 0, &[], 0o600).unwrap();
        let first = writer
            .append_group(
                vec![p.plain(&b"first committed payload"[..], 3).unwrap()],
                |_| Ok(None),
            )
            .unwrap()
            .wait()
            .unwrap();
        commit(dir.path(), &first.extents[0]);
        writer.acknowledge(&first).unwrap();
        drop(writer);
        let mut child = Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("writer::tests::crash_worker")
            .arg("--nocapture")
            .env("HASHED_BLOBS_CRASH_ROOT", dir.path())
            .env("HASHED_BLOBS_CRASH_BOUNDARY", boundary)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let mut status = 0;
        assert_eq!(
            unsafe { libc::waitpid(child.id() as i32, &mut status, libc::WUNTRACED) },
            child.id() as i32
        );
        assert!(
            libc::WIFSTOPPED(status),
            "worker failed before boundary {boundary}: {status}"
        );
        child.kill().unwrap();
        assert!(!child.wait().unwrap().success());
        let committed =
            Extent::decode(&std::fs::read(dir.path().join("committed")).unwrap()).unwrap();
        let _writer =
            SegmentWriter::open(dir.path(), "segment.zst", committed.end(), &[], 0o600).unwrap();
        assert_eq!(
            std::fs::metadata(dir.path().join("segment.zst"))
                .unwrap()
                .len(),
            committed.end(),
            "ordinary open did not remove uncommitted tail"
        );
        let reader = Reader::new(dir.path());
        assert_eq!(
            reader.read(&first.extents, None).unwrap(),
            b"first committed payload"
        );
        let expected = if matches!(boundary, "commit" | "truncate") {
            &b"second committed payload"[..]
        } else {
            &b"first committed payload"[..]
        };
        assert_eq!(reader.read(&[committed], None).unwrap(), expected);
    }
}
#[test]
fn missing_and_truncated_committed_segments_are_reported() {
    let dir = tempfile::tempdir().unwrap();
    assert!(SegmentWriter::open(dir.path(), "absent", 10, &[], 0o600).is_err());
    let mut file = File::create(dir.path().join("short")).unwrap();
    file.write_all(b"short").unwrap();
    drop(file);
    assert!(SegmentWriter::open(dir.path(), "short", 10, &[], 0o600).is_err());
    let mut bytes = Vec::new();
    File::open(dir.path().join("short"))
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(bytes, b"short");
}
