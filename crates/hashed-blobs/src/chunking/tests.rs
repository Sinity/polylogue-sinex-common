use super::*;

/// The gear hash at `position`, computed directly from its definition: the
/// sum of each byte's gear value shifted by its distance from `position`,
/// counting bytes from `min` on. Bytes 64 or more back shift out entirely.
fn direct_hash(chunker: &Chunker, bytes: &[u8], min: usize, position: usize) -> u64 {
    let first = min.max(position.saturating_sub(63));
    (first..=position).fold(0u64, |sum, index| {
        sum.wrapping_add(chunker.gear[bytes[index] as usize] << (position - index))
    })
}
/// An independent reading of FastCDC normalised chunking over a whole buffer.
fn direct_cut(chunker: &Chunker, bytes: &[u8]) -> usize {
    let ChunkSizes { min, target, max } = chunker.sizes;
    if bytes.len() <= min {
        return bytes.len();
    }
    let end = bytes.len().min(max);
    (min..end)
        .find(|&position| {
            let mask = if position < target.min(end) {
                chunker.strict
            } else {
                chunker.loose
            };
            direct_hash(chunker, bytes, min, position) & mask == 0
        })
        .map_or(end, |position| position + 1)
}
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

#[test]
fn rolling_cut_matches_the_direct_definition() {
    for (sizes, seed) in [
        (ChunkSizes::new(64, 256, 1024).unwrap(), 1),
        (ChunkSizes::new(16, 64, 128).unwrap(), 2),
        (ChunkSizes::new(256, 256, 256).unwrap(), 3),
        (ChunkSizes::default(), 4),
    ] {
        let plain = Chunker::plain(sizes);
        let sealed = Chunker::sealed(
            sizes,
            &DataKey {
                key_ref: "k".into(),
                key: [seed as u8; 32],
            },
        );
        for chunker in [plain, sealed] {
            let bytes = noise(sizes.max * 40, seed);
            let mut rest = &bytes[..];
            while !rest.is_empty() {
                let window = &rest[..rest.len().min(sizes.max)];
                let cut = chunker.cut(rest);
                assert_eq!(cut, direct_cut(&chunker, window));
                rest = &rest[cut..];
            }
        }
    }
}

#[test]
fn masks_follow_the_target() {
    let chunker = Chunker::plain(ChunkSizes::default());
    assert_eq!(chunker.strict.count_ones(), 18);
    assert_eq!(chunker.loose.count_ones(), 14);
    assert_eq!(chunker.strict.leading_ones(), 18);
}
