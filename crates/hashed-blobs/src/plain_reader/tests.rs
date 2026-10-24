use super::*;
use crate::{Hash, Reader, Store, DEFAULT_FRAME};
use std::sync::atomic::{AtomicBool, Ordering};

fn store(frame: usize) -> (tempfile::TempDir, Store, Vec<u8>, Hash) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(frame);
    let bytes: Vec<u8> = (0..(8 << 20)).map(|n| (n % 251) as u8).collect();
    let hash = store.put(bytes.as_slice(), 1).unwrap();
    (dir, store, bytes, hash)
}

#[test]
fn oversized_frames_are_decoded_once_per_pass_even_with_tiny_reads() {
    for frame in [DEFAULT_FRAME, 8 << 20] {
        let (_dir, store, bytes, hash) = store(frame);
        let mut reader = store
            .read_plain(&hash, Some(bytes.len() as u64), || Ok(()))
            .unwrap();
        let verified = reader.stats();
        assert_eq!(verified.decoded_bytes, bytes.len() as u64);
        let mut offset = 0;
        let mut buffer = [0; 1023];
        loop {
            let n = reader.read(&mut buffer).unwrap();
            if n == 0 {
                break;
            }
            assert_eq!(buffer[..n], bytes[offset..offset + n]);
            offset += n;
        }
        assert_eq!(offset, bytes.len());
        let after = reader.stats();
        assert_eq!(
            after.decoded_bytes - verified.decoded_bytes,
            bytes.len() as u64
        );
        assert_eq!(after.frames - verified.frames, verified.frames);
        assert_eq!(
            after.compressed_bytes - verified.compressed_bytes,
            verified.compressed_bytes
        );
        assert_eq!(after.metadata_bytes, verified.metadata_bytes);
        assert!(reader.plain.current.is_none());
    }
}

#[test]
fn cancellation_is_checked_during_verification_and_stops_an_open_reader() {
    let (_dir, store, bytes, hash) = store(DEFAULT_FRAME);
    assert!(matches!(
        store.read_plain(&hash, Some(bytes.len() as u64), || Err(Error::Refused(
            "cancelled".into()
        ))),
        Err(Error::Refused(_))
    ));
    let stop = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&stop);
    let mut reader = store
        .read_plain(&hash, Some(bytes.len() as u64), move || {
            if observed.load(Ordering::Relaxed) {
                Err(Error::Refused("cancelled".into()))
            } else {
                Ok(())
            }
        })
        .unwrap();
    assert_eq!(reader.read(&mut [0; 17]).unwrap(), 17);
    stop.store(true, Ordering::Relaxed);
    let error = reader.read(&mut [0; 17]).unwrap_err();
    assert!(matches!(
        error.get_ref().unwrap().downcast_ref::<Error>(),
        Some(Error::Refused(_))
    ));
    stop.store(false, Ordering::Relaxed);
    assert!(reader.read(&mut [0; 17]).is_err());
}

#[test]
fn corruption_and_wrong_lengths_are_refused_before_exposing_bytes() {
    let (_dir, store, bytes, hash) = store(DEFAULT_FRAME);
    assert!(matches!(
        store.read_plain(&hash, Some(bytes.len() as u64 + 1), || Ok(())),
        Err(Error::Corrupt { .. })
    ));
    let path = store.locate(&hash).unwrap();
    let mut encoded = std::fs::read(&path).unwrap();
    encoded[20] ^= 1;
    std::fs::write(path, encoded).unwrap();
    assert!(store
        .read_plain(&hash, Some(bytes.len() as u64), || Ok(()))
        .is_err());
}

#[test]
fn opened_generation_changes_are_refused_without_switching_to_a_replacement() {
    let (dir, store, bytes, hash) = store(DEFAULT_FRAME);
    let readonly = Reader::new(dir.path(), vec![]);
    let mut reader = readonly
        .read_plain(&hash, Some(bytes.len() as u64), || Ok(()))
        .unwrap();
    let path = store.locate(&hash).unwrap();
    let held = path.with_extension("held");
    std::fs::rename(&path, &held).unwrap();
    std::fs::write(&path, b"unrelated replacement").unwrap();
    assert!(reader.read(&mut [0; 19]).is_err());
    // A fresh read of the original path detects the in-place change on its own.
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&held, &path).unwrap();
    let mut reader = readonly.read_plain(&hash, None, || Ok(())).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(1).unwrap();
    assert!(reader.read(&mut [0; 19]).is_err());
}

#[test]
fn delta_and_sealed_encodings_keep_their_existing_key_contracts() {
    let (_dir, store, bytes, hash) = store(DEFAULT_FRAME);
    let delta = store.put_delta(&hash, &bytes[3..], 1).unwrap();
    assert!(matches!(
        store.read_plain(&delta, Some((bytes.len() - 3) as u64), || Ok(())),
        Err(Error::Refused(_))
    ));
    let key = crate::DataKey {
        key_ref: "synthetic-key".into(),
        key: [7; 32],
    };
    let sealed = store
        .put_sealed(std::io::Cursor::new(&bytes), 1, &key)
        .unwrap();
    assert!(matches!(
        store.read_plain(&sealed, Some(bytes.len() as u64), || Ok(())),
        Err(Error::Refused(_))
    ));
}

#[test]
fn zero_output_blocks_check_cancellation_while_reading_compressed_input() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path(), vec![], 0o600).unwrap();
    let hash = store.put(&b""[..], 1).unwrap();
    let path = store.locate(&hash).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert_eq!(&original[4..6], &[0x24, 0]);
    let table = original.len() - 25;
    let padding = 3 * 262144;
    let mut encoded = Vec::with_capacity(original.len() + padding);
    encoded.extend_from_slice(&original[..6]);
    encoded.resize(6 + padding, 0);
    encoded.extend_from_slice(&original[6..]);
    encoded[table + padding + 8..table + padding + 12]
        .copy_from_slice(&((table + padding) as u32).to_le_bytes());
    std::fs::write(path, encoded).unwrap();
    let enabled = Arc::new(AtomicBool::new(false));
    let active = Arc::clone(&enabled);
    let mut polls = 0;
    let mut reader = store
        .read_plain(&hash, Some(0), move || {
            if active.load(Ordering::Relaxed) {
                polls += 1;
                if polls == 4 {
                    return Err(Error::Refused("synthetic cancellation".into()));
                }
            }
            Ok(())
        })
        .unwrap();
    enabled.store(true, Ordering::Relaxed);
    let error = reader.read(&mut [0; 1]).unwrap_err();
    assert!(
        matches!(error.get_ref().unwrap().downcast_ref::<Error>(), Some(Error::Refused(reason)) if reason == "synthetic cancellation")
    );
    assert!(
        reader.stats().compressed_bytes < (table + padding) as u64 * 2,
        "cancellation must stop before a full second compressed pass"
    );
    assert!(reader.read(&mut [0; 1]).is_err());
}
