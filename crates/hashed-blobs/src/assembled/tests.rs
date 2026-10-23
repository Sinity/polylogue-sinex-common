//! Synthetic work controls for assembled reads. Counters describe completed
//! whole-name verification, not elapsed time, RSS or physical disk IO.
use super::*;
use crate::VerificationStats;
use std::cell::RefCell;
use std::fs::OpenOptions;
use std::io::Write;

#[derive(Debug, Default, Clone, Copy)]
struct Work {
    opens: u64,
    decoded: u64,
    compressed: u64,
}
thread_local! {
    static WORK: RefCell<Option<Work>> = const { RefCell::new(None) };
}
pub(crate) fn record_verification(stats: VerificationStats) {
    WORK.with(|work| {
        if let Some(w) = work.borrow_mut().as_mut() {
            w.opens += 1;
            w.decoded += stats.decoded_bytes;
            w.compressed += stats.compressed_bytes;
        }
    });
}
fn measured<T>(run: impl FnOnce() -> T) -> (T, Work) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            WORK.with(|w| *w.borrow_mut() = None);
        }
    }
    WORK.with(|work| {
        assert!(work.borrow().is_none());
        *work.borrow_mut() = Some(Work::default());
    });
    let _reset = Reset;
    let result = run();
    let work = WORK.with(|w| w.borrow().unwrap());
    (result, work)
}

fn data(seed: u64, len: usize) -> Vec<u8> {
    let mut x = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        x = x
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        out.extend_from_slice(
            format!("record {} value {}\n", x >> 40, (x >> 20) & 0xff).as_bytes(),
        );
    }
    out.truncate(len);
    out
}

#[test]
fn alternating_extents_verify_each_retained_blob_once() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(16 << 10);
    let base = data(71, 256 << 10);
    let patch = data(72, 64);
    let a = s.put(&base[..], 1).unwrap();
    let b = s.put(&patch[..], 1).unwrap();
    let mut extents = Vec::new();
    let mut expected = Vec::new();
    for n in 0..32 {
        extents.push(Extent {
            blob: a,
            offset: n * 128,
            len: 128,
        });
        extents.push(Extent {
            blob: b,
            offset: 0,
            len: 64,
        });
        expected.extend_from_slice(&base[n as usize * 128..(n as usize + 1) * 128]);
        expected.extend_from_slice(&patch);
    }
    let ((actual, decoded), work) = measured(|| {
        let mut assembled = s.assemble(&extents).unwrap();
        let mut actual = Vec::new();
        assembled.read_to_end(&mut actual).unwrap();
        let decoded = assembled
            .opened
            .iter()
            .map(|p| p.stats.decoded_bytes)
            .sum::<u64>();
        cache_bound(&assembled);
        (actual, decoded)
    });
    assert_eq!(actual, expected);
    eprintln!(
        "ASSEMBLED whole_verifications={} verification_decoded={} verification_compressed={}",
        work.opens, work.decoded, work.compressed
    );
    assert_eq!(
        work.opens, 2,
        "whole verification must follow retained blob identity, not extent switches"
    );
    assert_eq!(work.decoded, (base.len() + patch.len()) as u64);
    // One frame shared between alternating sources still decodes the base
    // frame on every return, and the patch frame after its first use.
    assert_eq!(
        decoded,
        work.decoded + 32 * (16 << 10) + 31 * patch.len() as u64
    );
    eprintln!(
        "ASSEMBLED all_decoded={decoded} postverification_decoded={}",
        decoded - work.decoded
    );
}

fn cache_bound(assembled: &Assembled<'_>) {
    assert!(assembled.opened.len() <= OPEN_PLAIN);
    assert!(assembled.opened.iter().all(|p| p.current.is_none()));
    assert!(assembled
        .frame
        .as_ref()
        .is_none_or(|(_, f)| f.bytes.len() <= super::super::DEFAULT_FRAME));
}

#[test]
fn many_tiny_extents_and_more_than_capacity_keep_the_hot_base_without_growing_opens() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(4096);
    let base = data(81, 32 << 10);
    let a = s.put(&base[..], 1).unwrap();
    let patches: Vec<_> = (0..OPEN_PLAIN * 4)
        .map(|n| {
            let mut bytes = data(n as u64 + 200, 13);
            bytes[0] = n as u8;
            (s.put(&bytes[..], 1).unwrap(), bytes)
        })
        .collect();
    let mut extents = Vec::new();
    let mut expected = Vec::new();
    for (n, (hash, bytes)) in patches.iter().enumerate() {
        extents.push(Extent {
            blob: a,
            offset: n as u64,
            len: 1,
        });
        expected.push(base[n]);
        extents.push(Extent {
            blob: *hash,
            offset: 0,
            len: bytes.len() as u64,
        });
        expected.extend_from_slice(bytes);
    }
    let ((), work) = measured(|| {
        let mut assembled = s.assemble(&extents).unwrap();
        for wanted in &expected {
            let mut byte = [0];
            assembled.read_exact(&mut byte).unwrap();
            assert_eq!(byte[0], *wanted);
            cache_bound(&assembled);
        }
        assert_eq!(assembled.read(&mut [0]).unwrap(), 0);
        assert_eq!(assembled.opened.len(), OPEN_PLAIN);
    });
    assert_eq!(work.opens, 1 + patches.len() as u64);
    assert_eq!(
        work.decoded,
        base.len() as u64 + patches.iter().map(|(_, b)| b.len() as u64).sum::<u64>()
    );
}

#[test]
fn wide_repeated_cycles_reverify_evicted_blobs_and_preserve_seek_bytes() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(32);
    let blobs: Vec<_> = (0..OPEN_PLAIN + 2)
        .map(|n| {
            let mut bytes = data(n as u64 + 300, 65);
            bytes[0] = n as u8;
            (s.put(&bytes[..], 1).unwrap(), bytes)
        })
        .collect();
    let extents: Vec<_> = blobs
        .iter()
        .map(|(blob, b)| Extent {
            blob: *blob,
            offset: 0,
            len: b.len() as u64,
        })
        .collect();
    let expected: Vec<_> = blobs.iter().flat_map(|(_, b)| b.iter().copied()).collect();
    let ((), work) = measured(|| {
        let mut assembled = s.assemble(&extents).unwrap();
        for _ in 0..2 {
            assembled.seek(SeekFrom::Start(0)).unwrap();
            let mut got = Vec::new();
            assembled.read_to_end(&mut got).unwrap();
            assert_eq!(got, expected);
            cache_bound(&assembled);
        }
        assert_eq!(assembled.opened.len(), OPEN_PLAIN);
    });
    assert_eq!(work.opens, 2 * blobs.len() as u64);
    let mut assembled = s.assemble(&extents).unwrap();
    for start in (0..expected.len()).step_by(7) {
        assembled.seek(SeekFrom::Start(start as u64)).unwrap();
        let n = 33.min(expected.len() - start);
        let mut got = vec![0; n];
        assembled.read_exact(&mut got).unwrap();
        assert_eq!(got, expected[start..start + n]);
        cache_bound(&assembled);
    }
    assert!(assembled
        .seek(SeekFrom::End(-(expected.len() as i64) - 1))
        .is_err());
    assert_eq!(
        assembled.seek(SeekFrom::End(1)).unwrap(),
        expected.len() as u64 + 1
    );
    assert_eq!(assembled.read(&mut [0]).unwrap(), 0);
}

#[test]
fn changed_retained_or_corrupt_first_later_and_evicted_representations_expose_no_bad_bytes() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(32);
    let a = s.put(&b"first synthetic blob"[..], 1).unwrap();
    let b = s.put(&b"second synthetic blob"[..], 1).unwrap();
    let extents = [
        Extent {
            blob: a,
            offset: 0,
            len: 4,
        },
        Extent {
            blob: b,
            offset: 0,
            len: 4,
        },
        Extent {
            blob: a,
            offset: 4,
            len: 4,
        },
    ];
    let mut assembled = s.assemble(&extents).unwrap();
    let mut first = [0; 4];
    assembled.read_exact(&mut first).unwrap();
    assert_eq!(&first, b"firs");
    // Length changes cannot hide inside timestamp granularity.
    OpenOptions::new()
        .append(true)
        .open(Store::path_in(&s.write_root, &a))
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let mut second = [0; 4];
    assembled.read_exact(&mut second).unwrap();
    assert_eq!(&second, b"seco");
    let mut untouched = [0x55; 4];
    assert!(assembled.read(&mut untouched).is_err());
    assert_eq!(untouched, [0x55; 4]);
    assert!(!assembled.opened.iter().any(|p| p.hash == a));
    assert!(assembled.frame.is_none());
    cache_bound(&assembled);
    for bad_first in [true, false] {
        let extents = if bad_first {
            vec![Extent {
                blob: a,
                offset: 0,
                len: 4,
            }]
        } else {
            vec![
                Extent {
                    blob: b,
                    offset: 0,
                    len: 4,
                },
                Extent {
                    blob: a,
                    offset: 0,
                    len: 4,
                },
            ]
        };
        let mut assembled = s.assemble(&extents).unwrap();
        if !bad_first {
            assembled.read_exact(&mut [0; 4]).unwrap();
        }
        let mut untouched = [0x66; 4];
        assert!(assembled.read(&mut untouched).is_err());
        assert_eq!(untouched, [0x66; 4]);
    }
    // Reopening an evicted representation is a complete verification, not
    // permission to trust a remembered pathname or prior metadata.
    let mut extents = vec![Extent {
        blob: b,
        offset: 0,
        len: 4,
    }];
    for n in 0..OPEN_PLAIN {
        let hash = s.put(&data(500 + n as u64, 16)[..], 1).unwrap();
        extents.push(Extent {
            blob: hash,
            offset: 0,
            len: 1,
        });
    }
    extents.push(Extent {
        blob: b,
        offset: 0,
        len: 4,
    });
    let mut assembled = s.assemble(&extents).unwrap();
    assembled.read_exact(&mut [0; 4]).unwrap();
    for _ in 0..OPEN_PLAIN {
        assembled.read_exact(&mut [0]).unwrap();
    }
    assert!(!assembled.opened.iter().any(|p| p.hash == b));
    OpenOptions::new()
        .append(true)
        .open(Store::path_in(&s.write_root, &b))
        .unwrap()
        .write_all(b"x")
        .unwrap();
    let mut untouched = [0x77; 4];
    assert!(assembled.read(&mut untouched).is_err());
    assert_eq!(untouched, [0x77; 4]);
}

#[test]
fn delta_assembly_keeps_legacy_bytes_and_sealed_assembly_still_requires_a_key() {
    use crate::DataKey;
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path(), vec![], 0o600)
        .unwrap()
        .with_frame_size(64 << 10);
    let base = data(600, 3 * 64 * 1024);
    let mut next = base.clone();
    next[65536..65576].fill(b'x');
    next.extend_from_slice(b"synthetic tail");
    let a = s.put(&base[..], 3).unwrap();
    let delta = s.put_delta(&a, &next[..], 3).unwrap();
    assert_eq!(s.base_of(&delta).unwrap(), Some(a));
    let extents = [
        Extent {
            blob: a,
            offset: 0,
            len: 8,
        },
        Extent {
            blob: delta,
            offset: 65534,
            len: 80,
        },
        Extent {
            blob: a,
            offset: 32,
            len: 8,
        },
    ];
    let mut expected = base[..8].to_vec();
    expected.extend_from_slice(&next[65534..65614]);
    expected.extend_from_slice(&base[32..40]);
    let mut assembled = s.assemble(&extents).unwrap();
    let mut actual = Vec::new();
    assembled.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, expected);
    cache_bound(&assembled);
    assembled.seek(SeekFrom::Start(9)).unwrap();
    let mut part = [0; 12];
    assembled.read_exact(&mut part).unwrap();
    assert_eq!(part, &expected[9..21]);
    let key = DataKey {
        key_ref: "synthetic-key".into(),
        key: [7; 32],
    };
    let sealed = s
        .put_sealed(&b"synthetic sealed bytes"[..], 1, &key)
        .unwrap();
    let mut assembled = s
        .assemble(&[Extent {
            blob: sealed,
            offset: 0,
            len: 4,
        }])
        .unwrap();
    let mut untouched = [0x88; 4];
    let error = assembled.read(&mut untouched).unwrap_err();
    assert!(
        matches!(error.get_ref().and_then(|e|e.downcast_ref::<Error>()),Some(Error::Locked(h)) if *h==sealed)
    );
    assert_eq!(untouched, [0x88; 4]);
    assert!(assembled.opened.is_empty());
    assert!(assembled.frame.is_none());
}
