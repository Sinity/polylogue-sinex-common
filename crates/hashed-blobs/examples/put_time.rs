//! Compare storing the same generated input through preparation and `put`.
use hashed_blobs::Store;
use std::time::Instant;

fn main() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = (0..(64 << 20)).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    for prepare in [true, false] {
        let store = Store::open(directory.path().join(prepare.to_string()), vec![], 0o600).unwrap();
        let first = Instant::now();
        let hash = if prepare {
            store
                .prepare_plain(bytes.as_slice(), 3)
                .unwrap()
                .install()
                .unwrap()
        } else {
            store.put(bytes.as_slice(), 3).unwrap()
        };
        let first = first.elapsed();
        let repeat = Instant::now();
        let repeated = if prepare {
            store
                .prepare_plain(bytes.as_slice(), 3)
                .unwrap()
                .install()
                .unwrap()
        } else {
            store.put(bytes.as_slice(), 3).unwrap()
        };
        assert_eq!(repeated, hash);
        println!(
            "prepare={prepare} bytes={} first={first:?} repeat={:?}",
            bytes.len(),
            repeat.elapsed()
        );
    }
}
