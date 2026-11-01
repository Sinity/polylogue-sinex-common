//! Storage and speed of a stream rewritten whole on every change, stored as one
//! blob per revision and as content-defined chunks.
//!
//! `cargo run --release -p hashed-blobs --example rewrite_metrics -- [revisions] [turn_bytes]`
//!
//! The workload is a synthetic conversation file in the shape of a session
//! snapshot: a JSON document with an update time near its start and a messages
//! array that gains one user and one assistant turn per revision.
use hashed_blobs::{ChunkSizes, Extent, GroupEntry, Hash, Preparation, Reader, SegmentWriter};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

fn text(seed: u64, len: usize) -> String {
    let words = [
        "the",
        "file",
        "was",
        "rewritten",
        "and",
        "each",
        "revision",
        "keeps",
        "every",
        "earlier",
        "turn",
        "so",
        "only",
        "its",
        "tail",
        "changes",
        "when",
        "a",
        "model",
        "answers",
        "with",
        "code",
        "fn",
        "main",
        "let",
        "value",
        "returns",
        "error",
    ];
    let mut out = String::with_capacity(len + 16);
    let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.push_str(words[(state % words.len() as u64) as usize]);
        out.push(if state.is_multiple_of(13) { '\n' } else { ' ' });
        if state.is_multiple_of(17) {
            out.push_str(&format!("{:x} ", state >> 20));
        }
    }
    out.truncate(len);
    out.replace('\n', "\\n")
}
fn document(revision: usize, turns: &[String]) -> Vec<u8> {
    format!(
        "{{\n  \"sessionId\": \"00000000-0000-4000-8000-000000000001\",\n  \"startTime\": \"2026-01-01T00:00:00.000Z\",\n  \"lastUpdated\": \"2026-01-01T{:02}:{:02}:{:02}.000Z\",\n  \"messages\": [\n{}\n  ]\n}}\n",
        revision / 3600 % 24,
        revision / 60 % 60,
        revision % 60,
        turns.join(",\n")
    )
    .into_bytes()
}

struct Store {
    writer: SegmentWriter,
    index: HashMap<Hash, Extent>,
    appended_bytes: u64,
    appended_blobs: u64,
}
impl Store {
    fn new(root: &std::path::Path) -> Result<Self, hashed_blobs::Error> {
        Ok(Self {
            writer: SegmentWriter::open(root, "segment.zst", 0, &[], 0o600)?,
            index: HashMap::new(),
            appended_bytes: 0,
            appended_blobs: 0,
        })
    }
    fn commit(&mut self, entries: Vec<GroupEntry>) -> Result<Vec<Extent>, hashed_blobs::Error> {
        let index = &self.index;
        let group = self
            .writer
            .append_group(entries.into_iter().map(Ok), |hash| {
                Ok(index.get(hash).cloned())
            })?
            .wait()?;
        self.writer.acknowledge(&group)?;
        self.appended_bytes += group.metrics.appended_bytes;
        self.appended_blobs += group.metrics.appended_materials;
        for extent in &group.extents {
            self.index.insert(extent.hash, extent.clone());
        }
        Ok(group.extents)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let revisions: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(200);
    let turn_bytes: usize = args.next().map(|s| s.parse()).transpose()?.unwrap_or(3000);
    let sizes = ChunkSizes::default();

    let mut turns = Vec::new();
    let mut files = Vec::with_capacity(revisions);
    for revision in 0..revisions {
        for (role, seed) in [("user", 2 * revision), ("model", 2 * revision + 1)] {
            let len = if role == "user" {
                turn_bytes / 4
            } else {
                turn_bytes
            };
            turns.push(format!(
                "    {{\"id\": \"{seed}\", \"type\": \"{role}\", \"content\": \"{}\"}}",
                text(seed as u64, len)
            ));
        }
        files.push(document(revision, &turns));
    }
    let input_bytes: u64 = files.iter().map(|f| f.len() as u64).sum();
    let final_bytes = files.last().map_or(0, |f| f.len() as u64);

    let mut report = Vec::new();
    for chunked in [false, true] {
        let dir = tempfile::tempdir()?;
        let preparation = Preparation::new(dir.path().join("scratch"))?;
        let mut store = Store::new(dir.path())?;
        let mut stored = Vec::with_capacity(revisions);
        let mut prepare_time = Duration::ZERO;
        let mut commit_time = Duration::ZERO;
        let mut new_chunk_bytes = 0u64;
        for bytes in &files {
            let started = Instant::now();
            let entries = if chunked {
                let index = &store.index;
                let planned = preparation.chunked(&bytes[..], 3, sizes, None, |hash| {
                    Ok(index.get(hash).cloned())
                })?;
                new_chunk_bytes += planned.new_chunks().1;
                planned.entries
            } else {
                new_chunk_bytes += bytes.len() as u64;
                vec![preparation.plain(&bytes[..], 3)?.into()]
            };
            prepare_time += started.elapsed();
            // Each revision is its own group: one append and one data sync.
            let started = Instant::now();
            let extents = store.commit(entries)?;
            commit_time += started.elapsed();
            stored.push(extents);
        }
        let reader = Reader::new(dir.path());
        let started = Instant::now();
        for (bytes, extents) in files.iter().zip(&stored) {
            if &reader.read(extents, None)? != bytes {
                return Err("a revision did not read back byte-exact".into());
            }
        }
        let read_time = started.elapsed();
        let extents: usize = stored.iter().map(Vec::len).sum();
        report.push(format!(
            "layout={} appended_bytes={} appended_blobs={} new_decoded_bytes={} extents_cited={} appended_bytes_per_final_byte={:.3} appended_bytes_per_input_byte={:.5} prepare_seconds={:.3} prepare_input_mb_per_second={:.1} commit_seconds={:.3} read_all_seconds={:.3} read_mb_per_second={:.1}",
            if chunked { "chunked" } else { "whole" },
            store.appended_bytes,
            store.appended_blobs,
            new_chunk_bytes,
            extents,
            store.appended_bytes as f64 / final_bytes as f64,
            store.appended_bytes as f64 / input_bytes as f64,
            prepare_time.as_secs_f64(),
            input_bytes as f64 / 1e6 / prepare_time.as_secs_f64(),
            commit_time.as_secs_f64(),
            read_time.as_secs_f64(),
            input_bytes as f64 / 1e6 / read_time.as_secs_f64(),
        ));
    }
    println!(
        "revisions={revisions} turn_bytes={turn_bytes} final_bytes={final_bytes} input_bytes={input_bytes} chunk_min={} chunk_target={} chunk_max={}",
        sizes.min(),
        sizes.target(),
        sizes.max()
    );
    for line in report {
        println!("{line}");
    }
    Ok(())
}
