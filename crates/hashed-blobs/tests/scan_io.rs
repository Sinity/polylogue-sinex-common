//! Count actual kernel read requests in a nextest-isolated case, including cache hits.
#![cfg(target_os = "linux")]
use hashed_blobs::{DataKey, Preparation, Reader, SegmentWriter};

fn read_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/io")
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("rchar: "))
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}

#[test]
fn scanning_plain_and_sealed_frames_reads_the_encoded_payload_once() {
    let mut random = 0x7682_193a_b507_4df1u64;
    let body: Vec<u8> = (0..1024 * 1024)
        .map(|_| {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            random as u8
        })
        .collect();
    for sealed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let preparation = Preparation::new(dir.path().join("scratch"))
            .unwrap()
            .with_frame_size(256 * 1024)
            .unwrap();
        let key = DataKey {
            key_ref: "synthetic".into(),
            key: [5; 32],
        };
        let prepared = if sealed {
            preparation
                .sealed(std::io::Cursor::new(&body), 3, &key)
                .unwrap()
        } else {
            preparation.plain(std::io::Cursor::new(&body), 3).unwrap()
        };
        let mut writer = SegmentWriter::open(dir.path(), "segment.hb", 0, &[], 0o600).unwrap();
        let group = writer
            .append_group([Ok(prepared)], |_| Ok(None))
            .unwrap()
            .wait()
            .unwrap();
        writer.acknowledge(&group).unwrap();
        let extent = &group.extents[0];
        assert!(
            extent.len > 1024 * 1024,
            "synthetic payload must remain incompressible"
        );
        let before = read_bytes();
        assert!(Reader::new(dir.path()).scan(&group.extents).is_empty());
        let read = read_bytes() - before;
        println!(
            "sealed={sealed} encoded_bytes={} kernel_read_bytes={read}",
            extent.len
        );
        // The index is opened separately, and reading /proc contributes a few
        // bytes. A second payload pass exceeds this allowance by over 1 MiB.
        assert!(
            read <= extent.len + 65536,
            "sealed={sealed}: read {read} bytes for {} encoded bytes",
            extent.len
        );
    }
}

#[test]
fn preparing_plain_and_sealed_bytes_does_not_read_back_the_encoding() {
    let mut random = 0x7682_193a_b507_4df1u64;
    let body: Vec<u8> = (0..1024 * 1024)
        .map(|_| {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            random as u8
        })
        .collect();
    for sealed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let preparation = Preparation::new(dir.path())
            .unwrap()
            .with_frame_size(65536)
            .unwrap();
        let key = DataKey {
            key_ref: "synthetic".into(),
            key: [5; 32],
        };
        let before = read_bytes();
        let prepared = if sealed {
            preparation
                .sealed(std::io::Cursor::new(&body), 3, &key)
                .unwrap()
        } else {
            preparation.plain(body.as_slice(), 3).unwrap()
        };
        let read = read_bytes() - before;
        assert!(prepared.stored_len() > body.len() as u64);
        // Input is memory-backed. /proc and random-salt setup may read a few
        // bytes, but a complete encoded payload reread exceeds this by 1 MiB.
        assert!(
            read < 65536,
            "sealed={sealed}: preparation read {read} bytes"
        );
        let descriptor = prepared.descriptor();
        let mut file = prepared.into_file();
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
        assert_eq!(descriptor.stored_len, bytes.len() as u64);
        assert_eq!(descriptor.encoded_hash, hashed_blobs::Hash::of(&bytes));
        // Independent decoding also verifies every frame and the bound index.
        let checked = hashed_blobs::Prepared::from_encoded(
            file,
            descriptor,
            sealed.then_some(&key),
            Default::default(),
        )
        .unwrap();
        let mut decoded = Vec::new();
        std::io::Read::read_to_end(
            &mut checked
                .reader(sealed.then_some(&key), Default::default())
                .unwrap(),
            &mut decoded,
        )
        .unwrap();
        assert_eq!(decoded, body);
    }
}
