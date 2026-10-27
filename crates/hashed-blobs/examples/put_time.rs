//! Compare first and repeated puts of the same generated input.
use hashed_blobs::{Hash, Store};
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant};

struct Stream(File);
impl Read for Stream {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.0.read(out)
    }
}

fn put(store: &Store, input: &Path, mode: &str) -> Hash {
    let file = File::open(input).unwrap();
    match mode {
        "prepare" => store.prepare_plain(file, 3).unwrap().install().unwrap(),
        "seekable" => store.put_seekable(file, 3).unwrap(),
        "stream" => store.put(Stream(file), 3).unwrap(),
        _ => unreachable!(),
    }
}

fn main() {
    let directory = tempfile::tempdir().unwrap();
    let bytes = (0..(64 << 20)).map(|n| (n % 251) as u8).collect::<Vec<_>>();
    let input = directory.path().join("input");
    std::fs::write(&input, &bytes).unwrap();
    let modes = ["prepare", "seekable", "stream"];
    let mut firsts = [Vec::new(), Vec::new(), Vec::new()];
    let mut repeats = firsts.clone();
    for round in 0..5 {
        for offset in 0..3 {
            let index = (round + offset) % 3;
            let mode = modes[index];
            let store = Store::open(
                directory.path().join(format!("{round}-{mode}")),
                vec![],
                0o600,
            )
            .unwrap();
            let first = Instant::now();
            let hash = put(&store, &input, mode);
            firsts[index].push(first.elapsed());
            let repeat = Instant::now();
            let repeated = put(&store, &input, mode);
            repeats[index].push(repeat.elapsed());
            assert_eq!(repeated, hash);
            println!(
                "round={round} mode={mode} bytes={} first={:?} repeat={:?}",
                bytes.len(),
                firsts[index].last().unwrap(),
                repeats[index].last().unwrap()
            );
        }
    }
    for (index, mode) in modes.iter().enumerate() {
        firsts[index].sort();
        repeats[index].sort();
        println!(
            "median mode={mode} bytes={} first_ms={:.3} repeat_ms={:.3}",
            bytes.len(),
            milliseconds(firsts[index][2]),
            milliseconds(repeats[index][2])
        );
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
