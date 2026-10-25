# hashed-blobs

A content-addressed file store for Unix, shared by Polylogue and Sinex. It needs stable Rust 1.98.1 or newer and a C compiler for zstd. Releases are tags of this repository; see its README.

The crate holds no policy. Inline-versus-file size, when to use a delta, snapshots, page maps, joining, the excision rewrite, privacy classes and key management belong to the caller.

| Layout | Name | Reads |
|---|---|---|
| plain | BLAKE3 of the content | zstd frames (4 MiB, each with its content checksum) plus a seek table in the zstd seekable format; stock `zstd -d` decompresses the file; `Store::read_range` checks all frames; `Reader` verifies the whole name once, then reuses its index and one cached frame |
| delta | BLAKE3 of the content | a skippable header naming the base, then one zstd frame made the way `zstd --patch-from` makes one: the base as prefix, a window over base and content, long-distance matching; base always plain (at most two blobs per read); not seekable |
| sealed | BLAKE3 keyed hash under HKDF(data key, "hashed-blobs name v1") | frames under ChaCha20-Poly1305 with a per-encoding frame key HKDF-SHA256(data key, salt = random 32-byte encoding salt, info = "hashed-blobs frame v2" ‖ name) and nonce = frame index; the AAD binds the encoding context and the frame's table entry; a footer carries the frame table and a BLAKE3 of the ciphertext, so `verify` works without the key |

## Writing

- `put` and `Batch` cannot be cancelled. `put_plain_checked` polls a caller callback while reading and compressing input and while verifying an existing copy; it refuses an existing delta or sealed blob of the same name. A blocking read or a single compression call is not interrupted.
- A new file is written to `blobs/tmp/`, synced and hard-linked into place, so concurrent writers never overwrite each other. If the name already exists, the existing file is fully verified first (with the key, for sealed data). An invalid existing file is refused and left alone; nothing is repaired silently. A temporary file is removed when its write fails, is cancelled or is installed; only a crash leaves one behind.
- `prepare_plain`, `prepare_plain_checked` and `prepare_sealed` write the blob to `blobs/tmp/` and return a handle with its `hash()` and `decoded_len()` before the blob is under its name. A caller that keeps its own record of blobs writes that record first, then calls `install()` or `install_checked()`; dropping the handle discards the temporary file. A sealed handle keeps a copy of the data key, so the caller can release its reader and key; sealed input implements `Read + Seek`, starts at offset zero and stays unchanged across the naming and encryption passes, and no plaintext reaches the disk.
- `names()` streams the names of the blob files in the write root, one shard directory at a time; it skips `blobs/tmp/` and extra roots, and reports an entry that is not a blob file as an `InvalidData` error and goes on. A caller that keeps its own record of blobs uses it to find files that record does not name, and deletes those only while nothing else writes.
- An install can fail after the hard link, in a directory sync; the blob is then installed and stays. Nothing rolls back installed names, because another reference may use them.
- A `Batch` shares syncs; it is not atomic. On error some names may be installed and the rest are discarded. On success every returned name is durable. Retrying is safe. Record references only after the commit succeeds. `contains` reports presence only.

## Reading and verification

- Supported plain files are the ones this writer makes: known-size, checksummed zstd frames without a dictionary, and a seek table without checksums. Anything else is refused as corrupt. Decoding checks every frame's length and extent.
- `Store::read_range` decodes and checks every plain frame before returning, so it is slow for repeated small reads. It does not check the whole blob against its name, so another valid blob placed under that name would be returned.
- `Reader` holds one open plain blob and one decoded frame of up to 4 MiB. The first range verifies the whole name; `verify_plain` does the same with a callback that sees each chunk, which must not pass chunks on before success. Later ranges reuse the open file and frame. Switching blobs drops the old one. Each concurrent call may hold its own blob and frame, so the caller limits concurrency. The lock is released before IO and callbacks. A change to the open file is refused; replacing the path never redirects the open file. `Reader` is `Send + Sync`.
- Callbacks run before metadata work, per frame header, per read of at most 64 KiB, and between decoded chunks of at most 64 KiB, so cancellation works even on blocks that decode to nothing. Memory is the requested output, the seek table, one frame, fixed buffers and the zstd window (window log up to 31). Frames over 4 MiB are streamed, not cached, so each range decodes the whole frame again. `read` returns the whole content in memory.

## Sealed encoding

Every encoding draws a fresh 32-byte salt from the OS random source before encrypting its first frame, including repeated puts of the same bytes and encodings later dropped by dedup. The salt is public. Naming does not use the salt, so dedup still finds the existing copy; but since dedup happens after encryption, the salt is what prevents key and nonce reuse. If the random source fails, the put fails.

The frame key is HKDF-SHA256 with the data key as input, the salt as HKDF salt, and `hashed-blobs frame v2` followed by the 32-byte keyed name as info. A nonce is eight zero bytes followed by the frame index as a big-endian u32.

The file is ciphertext frames followed by a footer: `HBSEAL2\0` (8 bytes), salt (32), key-reference length (u16 LE), key reference (UTF-8), full/delta flag (u8, 0 or 1), base keyed name (32, delta only), total decoded length (u64 LE), frame count (u32 LE), frame entries (ciphertext length and decoded length, u32 LE each), ciphertext BLAKE3 (32), footer length excluding the last eight bytes (u32 LE), and `HBS2` (4). There is at least one frame, even for empty content. The unkeyed ciphertext hash covers the frames and the footer through the frame table; it detects damage without the key but does not authenticate the footer.

Each frame's AAD is the footer up to the total decoded length, then the keyed name (32), frame index (u32 LE), frame count (u32 LE), last-frame flag (u8), the frame's ciphertext and decoded lengths (u32 LE each) and its decoded start offset (u64 LE). Changing any table entry or earlier length therefore fails authentication, while a range read still reads only the footer and the frames it needs.

`Store::assemble` exposes extents in their original order through `Read + Seek`.
It keeps at most four verified plain blobs open (four file descriptors and
their seek tables), least recently used first out, with one decoded frame of at
most 4 MiB shared between them. Returning to an open blob does not verify it
again; a blob closed to make room is verified again before its bytes are
returned. Memory also grows with the number of extents and the open blobs' frame
counts, plus the decoder window and each returned range (up to 1 MiB). An extent
over a delta blob reads the whole delta. Sealed blobs need the keyed read
functions. No plaintext is written to disk or cached between calls.

## Measurements

Deltas need the patch-from parameters (2026-10-12, synthetic text, a 100-byte change). A raw-content dictionary saved nothing from a 4 MiB base up. `zstd --patch-from` settings gave 441 B at 4 MiB, 7 KB at 64 MiB, 29 KB at 256 MiB and 123 KB at 1 GiB. Decoding needs about twice the base in memory (0.5 GB at 256 MiB), so the caller limits base size; Polylogue's limit is 640 MiB. The test `delta_against_a_large_base_stays_small` fails if the window is capped at zstd's default.

`examples/pt5.rs` measured real volumes on the operator's QLC drive (2026-10-12):

| Input | Mode | Rate |
|---|---|---|
| a day of media, 1.4 GiB in 6,880 files (Opus, WebP) | one durable `put` each, level 1 | 8 MiB/s |
| the same | `Batch`, `Durability::PerFile`, 1,000 files a batch | 25 MiB/s |
| the same | `Batch`, `Durability::Filesystem` | 397 MiB/s |
| a 15 GiB ZIP export | one `put`, level 1 | 75 MiB/s |

Syncs, not compression, set the pace for small blobs. `Durability::Filesystem` calls `syncfs` once before the renames and once after; because that also flushes other programs' writes, use it for bulk imports, and individual puts or small `PerFile` batches for live capture. Media stored at 94% of its size and level 3 gained nothing over level 1, so media is written at level 1. On the 15 GiB export, random 64 KiB range reads took 10 ms median and 70 ms at p99, `verify` streamed at 530 MiB/s and peak memory was 11 MB.

## Tests

`src/tests.rs` covers round trips, layouts, ranges, deltas, `assemble`, sealed blobs and corruption detected by the crate itself (the Polylogue archive is No_COW, so btrfs checksums are off). By hand: turning off frame checksums fails the raw-block test, and removing the frame count and last flag from the AAD fails the truncation range-read test.

`tests/sealed_lifetime.rs` shows how a caller can revoke in-flight sealed reads: after a lock, expiry, relock or purge, a read that unwrapped its key earlier must not cache or return plaintext. Bytes already returned to a client stay returned. The unlock owner there is a test stand-in, not Sinex's unlock service.

`tests/prepared_plain.rs` and `tests/prepared_sealed.rs` cover the prepared handles: hash and length before install, reads after it, concurrent deduplication, temporary-file cleanup on drop, error and cancellation, key-reference mismatch, a corrupt existing copy, and a sync failure after the hard link.
