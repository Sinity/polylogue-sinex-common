use super::*;
use std::io::{Cursor, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicBool, Ordering};

fn limits(frame: usize) -> SealedReadLimits {
    SealedReadLimits {
        frame_bytes: frame,
        index_bytes: 1 << 20,
    }
}

fn fixture(frame: usize, n: usize) -> (tempfile::TempDir, Store, DataKey, Vec<u8>, Hash) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(frame);
    let key = DataKey {
        key_ref: "synthetic-key".into(),
        key: [17; 32],
    };
    let bytes: Vec<_> = (0..n).map(|i| (i % 251) as u8).collect();
    let hash = store.put_sealed(Cursor::new(&bytes), 1, &key).unwrap();
    (dir, store, key, bytes, hash)
}

#[test]
fn tiny_reads_decode_each_authenticated_frame_once_per_pass() {
    for size in [0, 1, 65537, 3 * 65536 + 7] {
        let (_dir, store, key, bytes, hash) = fixture(65536, size);
        let mut reader = store
            .read_sealed(&hash, Some(size as u64), &key, limits(65536), || Ok(()))
            .unwrap();
        let proof = reader.stats();
        assert_eq!(proof.decoded_bytes, size as u64);
        let mut got = Vec::new();
        let mut chunk = [0u8; 37];
        loop {
            let n = reader.read(&mut chunk).unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&chunk[..n]);
        }
        assert_eq!(got, bytes);
        assert_eq!(reader.stats().decoded_bytes, size as u64 * 2);
        assert_eq!(reader.stats().frames, proof.frames * 2);
    }
}

#[test]
fn unavailable_wrong_keys_lengths_and_frame_limits_refuse_before_output() {
    let (_dir, store, key, bytes, hash) = fixture(65536, 100000);
    let mut wrong = key.clone();
    wrong.key_ref = "different-key".into();
    assert!(matches!(
        store.read_sealed(&hash, None, &wrong, limits(65536), || Ok(())),
        Err(Error::Unwrap(_))
    ));
    wrong = key.clone();
    wrong.key[0] ^= 1;
    assert!(matches!(
        store.read_sealed(&hash, None, &wrong, limits(65536), || Ok(())),
        Err(Error::Corrupt { .. })
    ));
    assert!(matches!(
        store.read_sealed(
            &hash,
            Some(bytes.len() as u64 + 1),
            &key,
            limits(65536),
            || Ok(())
        ),
        Err(Error::Corrupt { .. })
    ));
    assert!(matches!(
        store.read_sealed(&hash, None, &key, limits(32768), || Ok(())),
        Err(Error::Refused(_))
    ));
    assert!(matches!(
        store.read_sealed(&hash, None, &key, limits(0), || Ok(())),
        Err(Error::Refused(_))
    ));
    let plain = store.put(bytes.as_slice(), 1).unwrap();
    assert!(matches!(
        store.read_sealed(&plain, None, &key, limits(65536), || Ok(())),
        Err(Error::Refused(_))
    ));
}

#[test]
fn cancellation_and_in_place_changes_fuse_open_reader() {
    let (_dir, store, key, _bytes, hash) = fixture(65536, 100000);
    assert!(matches!(
        store.read_sealed(&hash, None, &key, limits(65536), || Err(Error::Refused(
            "cancelled".into()
        ))),
        Err(Error::Refused(_))
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let check = Arc::clone(&stop);
    let mut reader = store
        .read_sealed(&hash, None, &key, limits(65536), move || {
            if check.load(Ordering::Relaxed) {
                Err(Error::Refused("expired".into()))
            } else {
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(reader.read(&mut [0; 3]).unwrap(), 3);
    stop.store(true, Ordering::Relaxed);
    let error = reader.read(&mut [0; 3]).unwrap_err();
    assert!(matches!(
        error.get_ref().unwrap().downcast_ref::<Error>(),
        Some(Error::Refused(_))
    ));
    assert!(reader.read(&mut [0; 3]).is_err());
    let mut reader = store
        .read_sealed(&hash, None, &key, limits(65536), || Ok(()))
        .unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(Store::path_in(_dir.path(), &hash))
        .unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&[0]).unwrap();
    file.sync_all().unwrap();
    assert!(reader.read(&mut [0; 3]).is_err());
}

#[test]
fn ciphertext_corruption_and_forged_frame_extents_are_not_exposed() {
    let (_dir, store, key, _bytes, hash) = fixture(65536, 100000);
    let view = store.view(&hash).unwrap();
    let (mut footer, start) = Store::sealed_footer(&view, hash).unwrap();
    footer.frames[0].1 += 1;
    footer.plain_len += 1;
    let mut proof = blake3::Hasher::new();
    proof.update(&view.at(0, start).unwrap());
    proof.update(&footer.body());
    footer.cipher_hash = Hash(*proof.finalize().as_bytes());
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(Store::path_in(_dir.path(), &hash))
        .unwrap();
    file.seek(SeekFrom::Start(start)).unwrap();
    file.write_all(&footer.encode()).unwrap();
    file.sync_all().unwrap();
    assert!(matches!(
        store.read_sealed(&hash, None, &key, limits(65537), || Ok(())),
        Err(Error::Corrupt { .. })
    ));
}

#[test]
fn larger_legal_frames_work_when_the_caller_grants_a_sufficient_budget() {
    let (_dir, store, key, bytes, hash) = fixture(1 << 20, (1 << 20) + 17);
    assert!(matches!(
        store.read_sealed(&hash, None, &key, limits(65536), || Ok(())),
        Err(Error::Refused(_))
    ));
    let mut reader = store
        .read_sealed(&hash, None, &key, limits(1 << 20), || Ok(()))
        .unwrap();
    let mut got = Vec::new();
    reader.read_to_end(&mut got).unwrap();
    assert_eq!(got, bytes);
    assert_eq!(reader.stats().decoded_bytes, bytes.len() as u64 * 2);
    let mut budget = limits(1 << 20);
    budget.index_bytes = 1;
    assert!(matches!(
        store.read_sealed(&hash, None, &key, budget, || Ok(())),
        Err(Error::Refused(_))
    ));
}

#[test]
fn seeks_match_cursor_and_cache_one_frame() {
    let (_dir, store, key, bytes, hash) = fixture(65536, 3 * 65536 + 17);
    let mut reader = store
        .read_sealed(&hash, None, &key, limits(65536), || Ok(()))
        .unwrap();
    let mut oracle = Cursor::new(&bytes);
    for position in [
        0,
        99,
        65535,
        65536,
        131000,
        196608,
        bytes.len() as u64,
        bytes.len() as u64 + 7,
        50,
    ] {
        assert_eq!(
            reader.seek(SeekFrom::Start(position)).unwrap(),
            oracle.seek(SeekFrom::Start(position)).unwrap()
        );
        let mut actual = [0; 101];
        let mut expected = [0; 101];
        for _ in 0..3 {
            let a = reader.read(&mut actual).unwrap();
            oracle.read_exact(&mut expected[..a]).unwrap();
            assert_eq!(&actual[..a], &expected[..a]);
            if a == 0 {
                assert!(oracle.position() >= bytes.len() as u64);
            }
        }
    }
    reader.seek(SeekFrom::Start(100)).unwrap();
    reader.read_exact(&mut [0; 1]).unwrap();
    let before = reader.stats();
    for position in 100..1000 {
        reader.seek(SeekFrom::Start(position)).unwrap();
        reader.read_exact(&mut [0; 1]).unwrap();
    }
    assert_eq!(reader.stats().decoded_bytes, before.decoded_bytes);
    assert_eq!(
        reader.seek(SeekFrom::End(-17)).unwrap(),
        bytes.len() as u64 - 17
    );
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, &bytes[bytes.len() - 17..]);
    assert_eq!(
        reader.seek(SeekFrom::Current(-1)).unwrap(),
        bytes.len() as u64 - 1
    );
    assert!(reader.seek(SeekFrom::Start(0)).is_ok());
    assert!(reader.seek(SeekFrom::Current(-1)).is_err());
    assert!(reader.read(&mut [0; 1]).is_err());
}

#[test]
fn seek_checks_authority_and_generation_even_on_cached_bytes() {
    let (dir, store, key, _bytes, hash) = fixture(65536, 100000);
    let stop = Arc::new(AtomicBool::new(false));
    let check = Arc::clone(&stop);
    let mut reader = store
        .read_sealed(&hash, None, &key, limits(65536), move || {
            if check.load(Ordering::Relaxed) {
                Err(Error::Refused("revoked".into()))
            } else {
                Ok(())
            }
        })
        .unwrap();
    reader.read_exact(&mut [0; 1]).unwrap();
    stop.store(true, Ordering::Relaxed);
    assert!(reader.seek(SeekFrom::Start(0)).is_err());
    let mut reader = store
        .read_sealed(&hash, None, &key, limits(65536), || Ok(()))
        .unwrap();
    reader.read_exact(&mut [0; 1]).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(Store::path_in(dir.path(), &hash))
        .unwrap();
    file.write_all(&[3]).unwrap();
    file.sync_all().unwrap();
    assert!(reader.seek(SeekFrom::Start(0)).is_err());
}
