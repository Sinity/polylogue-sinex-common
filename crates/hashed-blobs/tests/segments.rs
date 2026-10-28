use hashed_blobs::{DataKey, Extent, Hash, Preparation, Reader, SegmentWriter};
use std::{
    fs::OpenOptions,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::FileExt,
};
fn open(root: &std::path::Path, end: u64) -> SegmentWriter {
    SegmentWriter::open(root, "segment.zst", end, &[], 0o600).unwrap()
}
fn prep(root: &std::path::Path) -> Preparation {
    Preparation::new(root.join("scratch"))
        .unwrap()
        .with_frame_size(32)
        .unwrap()
}
fn put(writer: &mut SegmentWriter, p: &Preparation, bytes: &[u8]) -> Extent {
    let group = writer
        .append_group(vec![p.plain(bytes, 3).unwrap()], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    let e = group.extents[0].clone();
    writer.acknowledge(&group).unwrap();
    e
}
#[test]
fn open_truncates_uncommitted_tail_and_committed_extents_never_move() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let first = put(&mut writer, &p, b"committed bytes");
    let original = std::fs::read(dir.path().join("segment.zst")).unwrap();
    let tail = writer
        .append_group(vec![p.plain(&b"uncommitted"[..], 3).unwrap()], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    assert!(tail.end > first.end());
    drop(writer);
    let mut writer = open(dir.path(), first.end());
    assert_eq!(
        std::fs::read(dir.path().join("segment.zst")).unwrap(),
        original
    );
    let next = put(&mut writer, &p, b"replacement");
    assert_eq!(next.offset, first.end());
    assert_eq!(
        Reader::new(dir.path()).read(&[first], None).unwrap(),
        b"committed bytes"
    );
}
#[test]
fn one_sync_per_group_and_dedup_across_uncommitted_groups() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let first = writer
        .append_group(
            vec![
                p.plain(&b"same"[..], 3).unwrap(),
                p.plain(&b"same"[..], 3).unwrap(),
            ],
            |_| Ok(None),
        )
        .unwrap();
    let second = writer
        .append_group(vec![p.plain(&b"same"[..], 3).unwrap()], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    let first = first.wait().unwrap();
    assert_eq!(first.extents[0], first.extents[1]);
    assert_eq!(second.extents[0], first.extents[0]);
    assert_eq!(first.metrics.syncs, 1);
    assert_eq!(first.metrics.appended_materials, 1);
    assert_eq!(first.metrics.deduplicated_materials, 1);
    assert_eq!(second.metrics.appended_bytes, 0);
    assert_eq!(second.metrics.syncs, 1);
    assert!(writer.acknowledge(&second).is_err());
    writer.acknowledge(&first).unwrap();
    writer.acknowledge(&second).unwrap();
    let committed = first.extents[0].clone();
    let third = writer
        .append_group(vec![p.plain(&b"same"[..], 3).unwrap()], |hash| {
            assert_eq!(*hash, Hash::of(b"same"));
            Ok(Some(committed.clone()))
        })
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(third.metrics.appended_bytes, 0);
}
#[test]
fn read_seek_range_and_plain_stock_zstd_identity() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let body: Vec<u8> = (0..997).map(|i| (i * 31) as u8).collect();
    let extent = put(&mut writer, &p, &body);
    let tail = put(&mut writer, &p, b"tail");
    let reader = Reader::new(dir.path());
    let mut opened = reader.open(&[extent.clone(), tail], None).unwrap();
    opened.seek(SeekFrom::Start(29)).unwrap();
    let mut bytes = [0; 11];
    opened.read_exact(&mut bytes).unwrap();
    assert_eq!(&bytes, &body[29..40]);
    assert_eq!(opened.stats().frames, 2);
    opened.seek(SeekFrom::End(-4)).unwrap();
    let mut last = Vec::new();
    opened.read_to_end(&mut last).unwrap();
    assert_eq!(last, b"tail");
    assert_eq!(
        reader.read_range(&[extent.clone()], 31, 35, None).unwrap(),
        body[31..66]
    );
    assert!(reader.read_range(&[extent.clone()], 997, 1, None).is_err());
    let file = std::fs::File::open(dir.path().join("segment.zst")).unwrap();
    let mut stored = vec![0; extent.len as usize];
    file.read_exact_at(&mut stored, extent.offset).unwrap();
    assert_eq!(zstd::stream::decode_all(&stored[..]).unwrap(), body);
    assert_eq!(Extent::decode(&extent.encode()).unwrap(), extent);
}
#[test]
fn read_checks_every_frame_and_errors_name_segment_extent() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let extent = put(&mut writer, &p, &[b'a'; 97]);
    let file = OpenOptions::new()
        .write(true)
        .open(dir.path().join("segment.zst"))
        .unwrap();
    file.write_all_at(&[0], extent.offset + 5).unwrap();
    let reader = Reader::new(dir.path());
    // Untouched frames remain seekable; the damaged first frame is never returned.
    assert_eq!(
        reader.read_range(&[extent.clone()], 64, 10, None).unwrap(),
        [b'a'; 10]
    );
    let error = reader
        .read_range(&[extent.clone()], 0, 1, None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("segment.zst"));
    assert!(error.contains(&format!("{}+{}", extent.offset, extent.len)));
    let mut open = reader.open(&[extent], None).unwrap();
    assert!(open.read(&mut [0]).is_err());
    assert!(open.seek(SeekFrom::Start(64)).is_err());
}
#[test]
fn sealed_no_plaintext_on_disk_tamper_fails_and_scan_checks_without_key() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let key = DataKey {
        key_ref: "fixture-key".into(),
        key: [7; 32],
    };
    let body = b"private synthetic content marker private synthetic content marker";
    let prepared = p.sealed(std::io::Cursor::new(body), 3, &key).unwrap();
    assert_eq!(prepared.hash(), Preparation::sealed_name(&key, body));
    assert_eq!(
        std::fs::read_dir(dir.path().join("scratch"))
            .unwrap()
            .count(),
        0
    );
    let mut writer = open(dir.path(), 0);
    let group = writer
        .append_group(vec![prepared], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    writer.acknowledge(&group).unwrap();
    let extent = &group.extents[0];
    let reader = Reader::new(dir.path());
    assert_eq!(reader.read(&group.extents, Some(&key)).unwrap(), body);
    assert!(reader.scan(&group.extents).is_empty());
    assert!(reader.open(&group.extents, None).is_err());
    let disk = std::fs::read(dir.path().join("segment.zst")).unwrap();
    assert!(!disk.windows(20).any(|w| w == &body[..20]));
    let file = OpenOptions::new()
        .write(true)
        .open(dir.path().join("segment.zst"))
        .unwrap();
    file.write_all_at(&[disk[0] ^ 1], extent.offset).unwrap();
    let before = std::fs::read(dir.path().join("segment.zst")).unwrap();
    assert!(reader.read(&group.extents, Some(&key)).is_err());
    let mismatches = reader.scan(&group.extents);
    assert_eq!(mismatches.len(), 1);
    assert_eq!(mismatches[0].extent, *extent);
    assert_eq!(
        std::fs::read(dir.path().join("segment.zst")).unwrap(),
        before
    );
}
#[test]
fn punch_removes_only_named_extents_and_recorded_punch_finishes_at_open() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let first = put(&mut writer, &p, b"keep before");
    let removed = put(&mut writer, &p, &vec![42; 20000]);
    let last = put(&mut writer, &p, b"keep after");
    let end = last.end();
    drop(writer);
    let mut writer =
        SegmentWriter::open(dir.path(), "segment.zst", end, &[removed.clone()], 0o600).unwrap();
    writer.punch(&[removed.clone()]).unwrap();
    let reader = Reader::new(dir.path());
    assert_eq!(reader.read(&[first], None).unwrap(), b"keep before");
    assert_eq!(reader.read(&[last], None).unwrap(), b"keep after");
    assert!(reader.read(&[removed], None).is_err());
}
#[test]
fn frame_serialization_round_trips_independent_inputs_and_empty_extents() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = open(dir.path(), 0);
    let reader = Reader::new(dir.path());
    let key = DataKey {
        key_ref: "test".into(),
        key: [42; 32],
    };
    for size in [1, 7, 64, 257] {
        let p = Preparation::new(dir.path().join("scratch"))
            .unwrap()
            .with_frame_size(size)
            .unwrap();
        for len in [0, 1, size - 1, size, size + 1, size * 3] {
            let bytes: Vec<u8> = (0..len)
                .map(|i| ((i * 113 + len * 7) % 251) as u8)
                .collect();
            let plain = p.plain(&bytes[..], 1).unwrap();
            let sealed = p.sealed(std::io::Cursor::new(&bytes), 1, &key).unwrap();
            let group = writer
                .append_group(vec![plain, sealed], |_| Ok(None))
                .unwrap()
                .wait()
                .unwrap();
            writer.acknowledge(&group).unwrap();
            assert_eq!(reader.read(&group.extents[..1], None).unwrap(), bytes);
            assert_eq!(reader.read(&group.extents[1..], Some(&key)).unwrap(), bytes);
            assert_eq!(
                reader
                    .read_range(&group.extents[..1], 0, len as u64, None)
                    .unwrap(),
                bytes
            );
        }
    }
}
#[test]
fn cancellation_and_single_writer_lock_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    assert!(SegmentWriter::open(dir.path(), "segment.zst", 0, &[], 0o600).is_err());
    let e = put(&mut writer, &p, b"content");
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = calls.clone();
    let mut reader = Reader::new(dir.path())
        .open_checked(&[e], None, move || {
            if c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) >= 2 {
                Err(hashed_blobs::Error::Refused("cancelled".into()))
            } else {
                Ok(())
            }
        })
        .unwrap();
    assert!(reader.read(&mut [0; 4]).is_err());
    assert_eq!(reader.stats().frames, 1);
}
#[test]
fn lookup_failure_poison_requires_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    assert!(writer
        .append_group(vec![p.plain(&b"input"[..], 3).unwrap()], |_| Err(
            hashed_blobs::Error::Refused("index unavailable".into())
        ))
        .is_err());
    assert!(writer.append_group(vec![], |_| Ok(None)).is_err());
    drop(writer);
    assert!(SegmentWriter::open(dir.path(), "segment.zst", 0, &[], 0o600).is_ok());
}
#[test]
fn scan_reports_all_mismatches_and_repairs_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let a = put(&mut writer, &p, b"one");
    let b = put(&mut writer, &p, b"two");
    let path = dir.path().join("segment.zst");
    let file = OpenOptions::new().write(true).open(&path).unwrap();
    file.write_all_at(&[0], a.offset).unwrap();
    file.write_all_at(&[0], b.offset).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(Reader::new(dir.path()).scan(&[a, b]).len(), 2);
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[test]
fn caller_budgets_refuse_index_and_frame_before_content_allocation() {
    let dir = tempfile::tempdir().unwrap();
    let p = prep(dir.path());
    let mut writer = open(dir.path(), 0);
    let extent = put(&mut writer, &p, &[9; 97]);
    let reader = Reader::new(dir.path());
    for limits in [
        hashed_blobs::ReadLimits {
            frame_bytes: 31,
            index_bytes: usize::MAX,
        },
        hashed_blobs::ReadLimits {
            frame_bytes: usize::MAX,
            index_bytes: 1,
        },
    ] {
        let result = reader.open_checked_with_limits(&[extent.clone()], None, limits, || Ok(()));
        assert!(matches!(result, Err(hashed_blobs::Error::Refused(_))));
    }
    let mut descriptor = extent.clone();
    descriptor.hash = Hash::of(b"different name");
    assert!(reader.open(&[descriptor], None).is_err());
    let file = OpenOptions::new()
        .write(true)
        .open(dir.path().join("segment.zst"))
        .unwrap();
    file.write_all_at(&u32::MAX.to_le_bytes(), extent.end() - 9)
        .unwrap();
    assert!(reader
        .open_checked_with_limits(
            &[extent],
            None,
            hashed_blobs::ReadLimits {
                frame_bytes: 64,
                index_bytes: 1000
            },
            || Ok(())
        )
        .is_err());
}
