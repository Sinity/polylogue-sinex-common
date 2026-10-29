use hashed_blobs::{
    DataKey, EncodedBlob, Hash, Preparation, Prepared, ReadLimits, Reader, SegmentWriter,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::{FileExt, MetadataExt},
};

#[test]
fn a_durable_prepared_inode_is_adopted_and_appended_without_a_plaintext_copy() {
    let dir = tempfile::tempdir().unwrap();
    let input = b"{\"records\":[{\"body\":\"synthetic retained input\"}]}\n".repeat(40_000);
    let path = dir.path().join("custody");
    let temporary = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let inode = temporary.as_file().metadata().unwrap().ino();
    let preparation = Preparation::new(dir.path()).unwrap();
    let prepared = preparation
        .plain_into(temporary.reopen().unwrap(), input.as_slice(), 3)
        .unwrap();
    let descriptor = prepared.descriptor();
    assert_eq!(descriptor.hash, Hash::of(&input));
    assert_eq!(descriptor.decoded_len, input.len() as u64);
    assert!(descriptor.stored_len < input.len() as u64 / 20);
    assert_eq!(prepared.into_file().metadata().unwrap().ino(), inode);
    temporary.as_file().sync_all().unwrap();
    temporary.persist(&path).unwrap();
    assert_eq!(
        EncodedBlob::decode(&descriptor.encode()).unwrap(),
        descriptor
    );
    let owned = File::open(&path).unwrap();
    let adopted =
        Prepared::from_encoded(owned, descriptor.clone(), None, ReadLimits::default()).unwrap();
    assert_eq!(adopted.into_file().metadata().unwrap().ino(), inode);
    let adopted = Prepared::from_encoded(
        File::open(&path).unwrap(),
        descriptor.clone(),
        None,
        ReadLimits::default(),
    )
    .unwrap();
    let mut writer = SegmentWriter::open(dir.path(), "segment", 0, &[], 0o600).unwrap();
    let group = writer
        .append_group(vec![adopted], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(group.metrics.appended_bytes, descriptor.stored_len);
    assert_eq!(
        std::fs::metadata(dir.path().join("segment")).unwrap().len(),
        descriptor.stored_len
    );
    writer.acknowledge(&group).unwrap();
    assert_eq!(
        Reader::new(dir.path()).read(&group.extents, None).unwrap(),
        input
    );
    let retry = Prepared::from_encoded(
        File::open(&path).unwrap(),
        descriptor,
        None,
        ReadLimits::default(),
    )
    .unwrap();
    let duplicate = writer
        .append_group(vec![retry], |_| Ok(Some(group.extents[0].clone())))
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(duplicate.metrics.appended_bytes, 0);
    writer.acknowledge(&duplicate).unwrap();
}

#[test]
fn independent_owned_encoded_transfers_reject_tampering_and_allow_a_seekable_reader() {
    let dir = tempfile::tempdir().unwrap();
    let input = b"synthetic body repeated for framed custody\n".repeat(1000);
    let p = Preparation::new(dir.path())
        .unwrap()
        .with_frame_size(256)
        .unwrap();
    let prepared = p.plain(input.as_slice(), 3).unwrap();
    let descriptor = prepared.descriptor();
    let mut file = prepared.into_file();
    let mut encoded = Vec::new();
    file.read_to_end(&mut encoded).unwrap();
    let copy = || {
        let mut file = tempfile::tempfile_in(dir.path()).unwrap();
        std::io::Write::write_all(&mut file, &encoded).unwrap();
        file
    };
    let transferred =
        Prepared::from_encoded(copy(), descriptor.clone(), None, ReadLimits::default()).unwrap();
    let mut reader = transferred.reader(None, ReadLimits::default()).unwrap();
    reader.seek(SeekFrom::Start(100)).unwrap();
    let mut slice = [0; 99];
    reader.read_exact(&mut slice).unwrap();
    assert_eq!(&slice, &input[100..199]);
    for changed in 0..4 {
        let mut wrong = descriptor.clone();
        match changed {
            0 => wrong.hash = Hash::of(b"forged plain identity"),
            1 => wrong.decoded_len += 1,
            2 => wrong.index_hash = Hash::of(b"other index"),
            _ => wrong.encoded_hash = Hash::of(b"other encoding"),
        };
        assert!(Prepared::from_encoded(copy(), wrong, None, ReadLimits::default()).is_err());
    }
    let corrupted = copy();
    corrupted.write_all_at(&[encoded[0] ^ 1], 0).unwrap();
    assert!(
        Prepared::from_encoded(corrupted, descriptor.clone(), None, ReadLimits::default()).is_err()
    );
    let truncated = copy();
    truncated.set_len(descriptor.stored_len - 1).unwrap();
    assert!(Prepared::from_encoded(truncated, descriptor, None, ReadLimits::default()).is_err());
}

#[test]
fn owner_sealing_reads_encoded_custody_without_writing_decoded_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let input = b"synthetic owner sealed input\n".repeat(20_000);
    let p = Preparation::new(dir.path()).unwrap();
    let staged = p.plain(input.as_slice(), 3).unwrap();
    let key = DataKey {
        key_ref: "synthetic-owner-key".into(),
        key: [7; 32],
    };
    let reader = staged.reader(None, ReadLimits::default()).unwrap();
    let target = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let inode = target.as_file().metadata().unwrap().ino();
    let sealed = p
        .sealed_into(target.reopen().unwrap(), reader, 3, &key)
        .unwrap();
    let descriptor = sealed.descriptor();
    assert_eq!(sealed.into_file().metadata().unwrap().ino(), inode);
    assert_eq!(descriptor.hash, Preparation::sealed_name(&key, &input));
    assert!(
        Prepared::from_encoded(
            target.reopen().unwrap(),
            descriptor.clone(),
            None,
            ReadLimits::default()
        )
        .is_err()
    );
    let adopted = Prepared::from_encoded(
        target.reopen().unwrap(),
        descriptor,
        Some(&key),
        ReadLimits::default(),
    )
    .unwrap();
    let mut decoded = Vec::new();
    adopted
        .reader(Some(&key), ReadLimits::default())
        .unwrap()
        .read_to_end(&mut decoded)
        .unwrap();
    assert_eq!(decoded, input);
    let disk = std::fs::read(target.path()).unwrap();
    assert!(!disk.windows(4096).any(|bytes| bytes == &input[..4096]));
}

#[test]
fn mutation_after_adoption_leaves_only_an_uncommitted_tail_and_retry_uses_the_same_encoding() {
    let dir = tempfile::tempdir().unwrap();
    let p = Preparation::new(dir.path()).unwrap();
    let staged = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let prepared = p
        .plain_into(
            staged.reopen().unwrap(),
            &b"durable synthetic custody"[..],
            3,
        )
        .unwrap();
    let descriptor = prepared.descriptor();
    let mut original = Vec::new();
    prepared.into_file().read_to_end(&mut original).unwrap();
    let adopted = Prepared::from_encoded(
        staged.reopen().unwrap(),
        descriptor.clone(),
        None,
        ReadLimits::default(),
    )
    .unwrap();
    staged
        .as_file()
        .write_all_at(&[original[0] ^ 1], 0)
        .unwrap();
    let mut writer = SegmentWriter::open(dir.path(), "segment", 0, &[], 0o600).unwrap();
    assert!(writer.append_group(vec![adopted], |_| Ok(None)).is_err());
    drop(writer);
    staged.as_file().write_all_at(&original, 0).unwrap();
    let mut writer = SegmentWriter::open(dir.path(), "segment", 0, &[], 0o600).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("segment")).unwrap().len(),
        0
    );
    let adopted = Prepared::from_encoded(
        staged.reopen().unwrap(),
        descriptor.clone(),
        None,
        ReadLimits::default(),
    )
    .unwrap();
    let uncommitted = writer
        .append_group(vec![adopted], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    drop(writer);
    let mut writer = SegmentWriter::open(dir.path(), "segment", 0, &[], 0o600).unwrap();
    assert_eq!(
        std::fs::metadata(dir.path().join("segment")).unwrap().len(),
        0
    );
    assert!(writer.acknowledge(&uncommitted).is_err());
    let adopted = Prepared::from_encoded(
        staged.reopen().unwrap(),
        descriptor,
        None,
        ReadLimits::default(),
    )
    .unwrap();
    let committed = writer
        .append_group(vec![adopted], |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    writer.acknowledge(&committed).unwrap();
    assert_eq!(
        Reader::new(dir.path())
            .read(&committed.extents, None)
            .unwrap(),
        b"durable synthetic custody"
    );
}
