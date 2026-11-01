use hashed_blobs::{
    ChunkSizes, Chunked, Chunker, DataKey, Extent, GroupEntry, Hash, Preparation, Reader,
    SegmentWriter,
};
use std::{collections::HashMap, io::Read, path::Path};

fn noise(len: usize, mut state: u64) -> Vec<u8> {
    (0..len)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}
fn chunks(chunker: &Chunker, bytes: &[u8]) -> Vec<Vec<u8>> {
    chunker.chunks(bytes).map(Result::unwrap).collect()
}
fn small() -> ChunkSizes {
    ChunkSizes::new(64, 256, 1024).unwrap()
}
fn key(byte: u8) -> DataKey {
    DataKey {
        key_ref: format!("key-{byte}"),
        key: [byte; 32],
    }
}

/// A product stand-in: one segment, a committed hash index, commit per group.
struct Store {
    preparation: Preparation,
    writer: SegmentWriter,
    index: HashMap<Hash, Extent>,
    appended_bytes: u64,
}
impl Store {
    fn new(root: &Path) -> Self {
        Self {
            preparation: Preparation::new(root.join("scratch")).unwrap(),
            writer: SegmentWriter::open(root, "segment.zst", 0, &[], 0o600).unwrap(),
            index: HashMap::new(),
            appended_bytes: 0,
        }
    }
    fn plan(&self, bytes: &[u8], sizes: ChunkSizes, key: Option<&DataKey>) -> Chunked {
        self.preparation
            .chunked(bytes, 3, sizes, key, |hash| {
                Ok(self.index.get(hash).cloned())
            })
            .unwrap()
    }
    fn commit(&mut self, entries: Vec<GroupEntry>) -> Vec<Extent> {
        let group = self
            .writer
            .append_group(entries.into_iter().map(Ok), |hash| {
                Ok(self.index.get(hash).cloned())
            })
            .unwrap()
            .wait()
            .unwrap();
        self.writer.acknowledge(&group).unwrap();
        self.appended_bytes += group.metrics.appended_bytes;
        for extent in &group.extents {
            self.index.insert(extent.hash, extent.clone());
        }
        group.extents
    }
    fn store(&mut self, bytes: &[u8], sizes: ChunkSizes, key: Option<&DataKey>) -> Vec<Extent> {
        let planned = self.plan(bytes, sizes, key);
        self.commit(planned.entries)
    }
}

/// A synthetic conversation: a JSON document whose messages array grows.
fn message(number: usize, len: usize) -> String {
    let words = [
        "the", "segment", "reads", "chunk", "writer", "commits", "a", "group", "of", "frames",
        "and", "returns", "extents", "in", "order", "while", "readers",
    ];
    let mut text = String::new();
    let mut state = (number as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    while text.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        text.push_str(words[(state % words.len() as u64) as usize]);
        text.push(' ');
        if state.is_multiple_of(11) {
            text.push_str(&format!("{} ", state % 100_000));
        }
    }
    text.truncate(len);
    format!("{{\"id\":{number},\"role\":\"assistant\",\"text\":\"{text}\"}}")
}
fn document(messages: &[String]) -> Vec<u8> {
    format!("{{\"messages\":[{}]}}", messages.join(",")).into_bytes()
}

#[test]
fn rewrite_costs_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::new(dir.path());
    let sizes = ChunkSizes::default();
    let mut messages: Vec<String> = (0..512).map(|n| message(n, 2048)).collect();
    let mut revisions = Vec::new();
    for revision in 0..300 {
        messages.push(message(512 + revision, 2048));
        let bytes = document(&messages);
        let extents = store.store(&bytes, sizes, None);
        revisions.push((bytes, extents));
    }
    let final_size = revisions.last().unwrap().0.len() as u64;
    assert!(final_size > 1 << 20);
    let bound = final_size * 12 / 10 + 300 * sizes.max() as u64;
    assert!(
        store.appended_bytes <= bound,
        "{} new blob bytes exceed {bound}",
        store.appended_bytes
    );
    let reader = Reader::new(dir.path());
    for (bytes, extents) in &revisions {
        assert_eq!(&reader.read(extents, None).unwrap(), bytes);
    }
}

#[test]
fn chunks_cover_the_stream_within_bounds() {
    let sizes = small();
    let chunker = Chunker::plain(sizes);
    let lengths = [
        0,
        1,
        63,
        64,
        65,
        255,
        256,
        1023,
        1024,
        1025,
        3 * 1024 + 7,
        50_000,
    ];
    for (seed, &len) in lengths.iter().enumerate() {
        let bytes = noise(len, seed as u64 + 1);
        let pieces = chunks(&chunker, &bytes);
        assert_eq!(pieces.concat(), bytes);
        if len == 0 {
            assert_eq!(pieces, vec![Vec::<u8>::new()]);
            continue;
        }
        let (last, body) = pieces.split_last().unwrap();
        assert!(!last.is_empty() && last.len() <= sizes.max());
        for piece in body {
            assert!((sizes.min()..=sizes.max()).contains(&piece.len()));
        }
    }
    // A run of one byte value cuts identically every time, so it is stored once.
    let zeros = vec![0u8; 10 * 1024 + 5];
    let pieces = chunks(&chunker, &zeros);
    assert_eq!(pieces.concat(), zeros);
    let (_, body) = pieces.split_last().unwrap();
    assert!(body.len() >= 10 && body.iter().all(|piece| piece == &body[0]));
}

/// Hands out one to `step` bytes per read, cycling.
struct Trickle<'a> {
    bytes: &'a [u8],
    step: usize,
    next: usize,
}
impl Read for Trickle<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        self.next = self.next % self.step + 1;
        let n = self.next.min(out.len()).min(self.bytes.len());
        out[..n].copy_from_slice(&self.bytes[..n]);
        self.bytes = &self.bytes[n..];
        Ok(n)
    }
}

#[test]
fn cut_points_depend_only_on_content() {
    let chunker = Chunker::plain(small());
    let bytes = noise(40_000, 7);
    let whole = chunks(&chunker, &bytes);
    for step in [1, 3, 100, 4096] {
        let trickled: Vec<_> = chunker
            .chunks(Trickle {
                bytes: &bytes,
                step,
                next: 0,
            })
            .map(Result::unwrap)
            .collect();
        assert_eq!(trickled, whole);
    }
    // Appending to a stream keeps every chunk of the prefix but its last.
    let prefix = chunks(&chunker, &bytes[..25_000]);
    assert_eq!(whole[..prefix.len() - 1], prefix[..prefix.len() - 1]);
}

#[test]
fn an_edit_changes_only_nearby_chunks() {
    let chunker = Chunker::plain(ChunkSizes::default());
    let bytes = noise(4 << 20, 11);
    let before: std::collections::HashSet<_> = chunks(&chunker, &bytes).into_iter().collect();
    let middle = bytes.len() / 2;
    let inserted = [&bytes[..middle], &noise(100, 12)[..], &bytes[middle..]].concat();
    let removed = [&bytes[..middle], &bytes[middle + 100..]].concat();
    let overwritten = [
        &bytes[..middle],
        &noise(100, 13)[..],
        &bytes[middle + 100..],
    ]
    .concat();
    for edited in [inserted, removed, overwritten] {
        let after = chunks(&chunker, &edited);
        let new = after
            .iter()
            .filter(|chunk| !before.contains(*chunk))
            .count();
        assert!((1..=3).contains(&new), "{new} chunks changed");
    }
}

#[test]
fn sealed_cut_points_are_keyed() {
    let sizes = small();
    let bytes = noise(50_000, 5);
    let lengths = |chunker: &Chunker| -> Vec<usize> {
        chunks(chunker, &bytes).iter().map(Vec::len).collect()
    };
    let first = lengths(&Chunker::sealed(sizes, &key(1)));
    assert_eq!(first, lengths(&Chunker::sealed(sizes, &key(1))));
    assert_ne!(first, lengths(&Chunker::sealed(sizes, &key(2))));
    assert_ne!(first, lengths(&Chunker::plain(sizes)));
}

#[test]
fn sealed_chunked_revisions_read_back_with_their_key() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::new(dir.path());
    let data_key = key(9);
    let first = noise(20_000, 21);
    let second = [&first[..], &noise(500, 22)[..]].concat();
    let one = store.store(&first, small(), Some(&data_key));
    let planned = store.plan(&second, small(), Some(&data_key));
    assert_eq!(planned.hash, Preparation::sealed_name(&data_key, &second));
    let (new, _) = planned.new_chunks();
    assert!(new < planned.entries.len());
    let two = store.commit(planned.entries);
    let reader = Reader::new(dir.path());
    assert_eq!(reader.read(&one, Some(&data_key)).unwrap(), first);
    assert_eq!(reader.read(&two, Some(&data_key)).unwrap(), second);
    assert!(reader.read(&two, None).is_err());
}

#[test]
fn stored_chunks_are_not_prepared_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::new(dir.path());
    let bytes = noise(30_000, 31);
    let first = store.plan(&bytes, small(), None);
    assert_eq!(first.hash, Hash::of(&bytes));
    assert_eq!(first.len, bytes.len() as u64);
    assert_eq!(first.new_chunks().0, first.entries.len());
    let extents = store.commit(first.entries);
    let again = store.plan(&bytes, small(), None);
    assert_eq!(again.new_chunks(), (0, 0));
    let appended = store.appended_bytes;
    assert_eq!(store.commit(again.entries), extents);
    assert_eq!(store.appended_bytes, appended);

    // Chunks of a group appended but not yet acknowledged are found through
    // the writer, so a burst of revisions prepares each chunk once.
    let other = noise(30_000, 32);
    let planned = store.plan(&other, small(), None);
    let pending = store
        .writer
        .append_group(planned.entries.into_iter().map(Ok), |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    let burst = store
        .preparation
        .chunked(&other[..], 3, small(), None, |hash| {
            Ok(store.writer.uncommitted(hash))
        })
        .unwrap();
    assert_eq!(burst.new_chunks(), (0, 0));
    let group = store
        .writer
        .append_group(burst.entries.into_iter().map(Ok), |_| Ok(None))
        .unwrap()
        .wait()
        .unwrap();
    assert_eq!(group.metrics.appended_bytes, 0);
    assert_eq!(group.extents, pending.extents);
}

#[test]
fn a_wrong_index_answer_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::new(dir.path());
    let stored = store.store(b"some other blob", small(), None);
    let result = store
        .preparation
        .chunked(&noise(5_000, 41)[..], 3, small(), None, |_| {
            Ok(Some(stored[0].clone()))
        });
    assert!(result.is_err());
}

#[test]
fn chunk_sizes_are_checked() {
    assert!(ChunkSizes::new(0, 256, 1024).is_err());
    assert!(ChunkSizes::new(300, 256, 1024).is_err());
    assert!(ChunkSizes::new(64, 300, 1024).is_err());
    assert!(ChunkSizes::new(64, 256, 128).is_err());
    assert!(ChunkSizes::new(1, 4, 8).is_err());
    let sizes = ChunkSizes::default();
    assert_eq!(
        (sizes.min(), sizes.target(), sizes.max()),
        (16 << 10, 64 << 10, 256 << 10)
    );
}
