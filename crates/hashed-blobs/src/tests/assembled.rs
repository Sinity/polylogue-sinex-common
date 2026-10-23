//! Synthetic work controls for assembled reads. Counters describe completed
//! whole-name verification, not elapsed time, RSS or physical disk IO.
use super::*;
use std::cell::RefCell;

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
    let (actual, work) = measured(|| {
        let mut assembled = s.assemble(&extents).unwrap();
        let mut actual = Vec::new();
        assembled.read_to_end(&mut actual).unwrap();
        actual
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
}
