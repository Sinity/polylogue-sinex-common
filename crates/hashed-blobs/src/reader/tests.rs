use super::*;
use crate::{Preparation, SegmentWriter};
use std::{fs::OpenOptions, os::unix::fs::FileExt};

#[test]
fn sealed_tamper_fails_even_when_unkeyed_frame_and_index_digests_are_replaced() {
    for mutation in ["ciphertext", "move", "table"] {
        let dir = tempfile::tempdir().unwrap();
        let preparation = Preparation::new(dir.path().join("scratch"))
            .unwrap()
            .with_frame_size(32)
            .unwrap();
        let key = DataKey {
            key_ref: "fixture".into(),
            key: [5; 32],
        };
        let body: Vec<u8> = (0..53).map(|n| n as u8).collect();
        let mut writer = SegmentWriter::open(dir.path(), "segment.zst", 0, &[], 0o600).unwrap();
        let group = writer
            .append_group(
                vec![preparation
                    .sealed(std::io::Cursor::new(&body), 3, &key)
                    .unwrap()],
                |_| Ok(None),
            )
            .unwrap()
            .wait()
            .unwrap();
        writer.acknowledge(&group).unwrap();
        let mut extent = group.extents[0].clone();
        let path = dir.path().join("segment.zst");
        let mut disk = std::fs::read(&path).unwrap();
        let index_len =
            u64::from_le_bytes(disk[disk.len() - 16..disk.len() - 8].try_into().unwrap()) as usize;
        let index_at = disk.len() - index_len - 16;
        let mut index = Index::decode(&disk[index_at..index_at + index_len]).unwrap();
        match mutation {
            "ciphertext" => {
                disk[0] ^= 1;
                index.frames[0].digest = Hash::of(&disk[..index.frames[0].stored as usize]);
            }
            "move" => {
                let first = index.frames[0].stored as usize;
                let second = index.frames[1].stored as usize;
                let reordered = [&disk[first..first + second], &disk[..first]].concat();
                disk[..first + second].copy_from_slice(&reordered);
                index.frames.swap(0, 1);
            }
            "table" => {
                index.frames[0].decoded -= 1;
                index.frames[1].decoded += 1;
            }
            _ => unreachable!(),
        }
        let encoded = index.encode();
        disk[index_at..index_at + index_len].copy_from_slice(&encoded);
        extent.index_hash = Hash::of(&encoded);
        extent.encoded_hash = Hash::of(&disk);
        OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .write_all_at(&disk, 0)
            .unwrap();
        let error = Reader::new(dir.path())
            .read(&[extent], Some(&key))
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("authentication failed"),
            "{mutation}: {error}"
        );
    }
}
#[test]
fn malformed_extent_descriptors_never_panic() {
    let extent = Extent {
        segment: "segment.zst".into(),
        offset: 0,
        len: 100,
        decoded_len: 32,
        hash: Hash([1; 32]),
        kind: Kind::Plain,
        index_hash: Hash([2; 32]),
        encoded_hash: Hash([3; 32]),
    };
    let encoding = extent.encode();
    for end in 0..encoding.len() {
        assert!(Extent::decode(&encoding[..end]).is_err());
    }
    let mut trailing = encoding.clone();
    trailing.push(0);
    assert!(Extent::decode(&trailing).is_err());
    let mut invalid = extent;
    invalid.segment = "../outside".into();
    assert!(Extent::decode(&invalid.encode()).is_err());
}
