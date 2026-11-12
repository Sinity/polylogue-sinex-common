# hashed-blobs

Content-addressed bytes for Polylogue and Sinex: blobs named by their hash,
stored in append-only segments and read back with their hashes checked. Both
products store every raw byte they keep through this crate, so the byte
mechanics are written once.

The crate holds no policy. When a group commits, which rows cite which
extents, when bytes may be removed, which key seals what, and how long
anything is kept all belong to the product.

## Blobs and names

A **blob** is the bytes of one stored input, named by its hash:

- **plain** blobs are named by the BLAKE3 hash of their content;
- **sealed** blobs are named by a BLAKE3 keyed hash under a naming key derived
  from the data key, so equal content sealed under one key deduplicates, and a
  name says nothing about the content to someone without the key.

A blob is stored as one or more **frames** in a segment. Plain frames are
standard zstd frames, each with its content checksum, so a segment's plain
bytes decode with stock `zstd`. Sealed frames are compressed, then encrypted
with ChaCha20-Poly1305 under a frame key derived from the data key and a
fresh random salt per encoding; the nonce is the frame index, and each frame's
associated data binds its position, its lengths and the blob's frame table, so
a frame moved, cut short or swapped fails authentication. A BLAKE3 hash of the
ciphertext lets integrity be checked without the key. No plaintext of a
sealed blob reaches the disk. The naming key uses HKDF-SHA256 with the
`hashed-blobs name v1` context, preserving byte identities across encodings.

An **extent** is the byte range in one segment that holds one blob's frames.
A product's rows cite extents; a list of extents read in order is one stream.

## Rewritten streams: content-defined chunks

Some inputs are rewritten whole on every change (a JSON session snapshot, a
browser capture of a conversation, a document revision). Stored as one blob
per revision, a file that grows by one turn per revision costs the square of
its size. Such a stream is instead stored as **chunks**: the stream is cut at
content-defined points, each chunk is an ordinary blob, and a revision is the
list of its chunks' extents. A new revision appends only the chunks the
product does not already hold; every unchanged run of bytes cites the extents
stored for an earlier revision.

**Cut points.** Cuts are placed by FastCDC normalised chunking (Xia et al.,
USENIX ATC 2016): a gear rolling hash over the bytes after the minimum length,
a stricter mask (two more bits than the target's) before the target length and
a looser one (two fewer) after it, and a cut at the maximum length if none
came sooner. The defaults are a 16 KiB minimum, 64 KiB target and 256 KiB
maximum. A cut depends only on the bytes from its chunk's start to the cut, so
an edit changes the chunk it falls in and, until the cuts realign, the next
one or two; every chunk before it, and the chunks after realignment, keep
their names. The same bytes always cut the same way, however they are read.

**Sealed chunks.** Plain chunking uses one public gear table, derived from the
context `hashed-blobs chunk gear v1`. Sealed chunking derives the table from
the data key (HKDF-SHA256 with the same context), so the chunk lengths visible
in extents say nothing about the content to someone without the key, and
equal content under one key still cuts and deduplicates the same way.

**Nothing above the extent changes.** A revision reads back as the exact bytes
of that revision through the ordinary reader over its extents in order; chunk
names deduplicate like any blob's; every revision stays readable. The planner
returns the whole stream's name (plain or keyed, as a blob's would be) and
length, so a product can recognise an unchanged revision without comparing
chunks. Whether a location is stored this way is the product's declaration;
the crate never detects it.

## The segment container

A **segment** is an append-only file of frames. Each product has one writer,
and that writer is the only process appending to its segments.

**Writing a group.** The writer collects a group of new blobs and:

1. appends their frames to the open segment;
2. syncs once for the whole group, on a dedicated thread, while the next group
   is being prepared;
3. hands the new extents back, and the product commits them in the same
   database transaction as the rows that cite them.

One sync per group rather than per blob is the design because syncs, not
compression, set the pace for small blobs. Extents become real only when the
product commits them; until then they are an uncommitted tail.

**Deduplication.** Before appending, the writer checks the product's hash
index (committed blobs) and the set of hashes appended in groups that have not
committed yet. A blob already present in either is not appended again; the
new row cites the existing extent.

**Opening a segment.** The product passes the end of the last extent it
committed. The segment opens there and truncates anything after it. That
truncation is the ordinary open path, not a recovery step: a crash at any
point between append, sync, commit and truncate leaves at most an uncommitted
tail, and opening removes it.

**Removing bytes.** The product can name committed extents to punch: their
bytes are released with a hole punch, and every other extent reads unchanged.
The product decides when a punch is allowed (for example only after nothing
else reads those bytes, and never under an open backup hold). A punch the
product recorded but did not finish before a crash is finished when the
segment opens, from the list the product passes in. Punching an extent twice
is harmless.

**One format.** Segments are the only storage layout. A blob is never patched
against another blob, and there is no second file-per-blob layout to read; a
chunked stream is a list of ordinary blobs.

## Reading

- A read takes extents and returns their decoded bytes, as a whole value or as
  a `Read + Seek` stream over the extents in order. Range reads decode only the
  frames the range touches.
- Every frame read is checked: plain frames against their checksums and the
  blob against its name, sealed frames by authentication. A mismatch fails the
  read with an error naming the segment and the extent. Unchecked bytes are
  never returned, and a damaged extent is never replaced by bytes from
  elsewhere.
- Sealed reads take the data key from the caller for the duration of the read.
  A reader holds at most its current decoded frame. Decrypted frames stay
  within that reader and are dropped with it; a cancellation callback runs
  before each read and seek. No plaintext is shared between readers.
- Reads never block the writer: committed extents never move, so a reader
  needs no lock beyond the segment file it holds open.

## Checking stored bytes

There is no scheduled check. Reads check what they read, and one on-demand
scan reads every frame of every segment, checks it, and reports each mismatch
with its segment and extent. The scan repairs nothing and rewrites nothing; a
damaged extent is restored from a backup by the operator.

## What a product supplies

- the record of committed extents and the hash index used for deduplication;
- the transaction that commits extents with the rows that cite them;
- the committed end of each segment, and the recorded punches, when opening;
- the decision to punch, and when;
- data keys and their references for sealed blobs;
- its durability order around these steps (where derived stores and source
  positions are saved).

## Rust API and durability order

`Preparation::new(scratch)` creates anonymous scratch files, independently of
any store. `plain(Read, level)` and `sealed(Read + Seek, level, &DataKey)`
return `Prepared`; their checked forms accept cancellation callbacks. The
sealed input is read twice from its current position, with its identity
checked across passes. Only encrypted frames reach the scratch file.
Dropping a preparation closes its already unlinked file. Preparation never
syncs a blob separately. Frame size defaults to 4 MiB.

Prepared encodings can be handed to another owner before it has a segment location. `EncodedBlob` binds the codec version, decoded hash and length, encoded length, frame index hash and whole encoded hash. It has no path or offset. `Prepared::descriptor` exports it and `into_file` returns the encoded inode. `Preparation::plain_into` and `sealed_into` write the existing codec into a caller-owned file; its product can sync and persist that inode without copying plaintext. Their checked forms retain cancellation callbacks. `plain_writer(file, level)` implements `Write` for serializers, and `PlainWriter::finish` returns the completed `Prepared`; dropping an unfinished writer abandons it. The caller supplies a new file and owns its persistence and immutability.

`EncodedBlob::open(file, key, limits)` and `Prepared::reader` provide the same bounded `Read + Seek` decoder for inspection or owner sealing. `Prepared::from_encoded` checks the complete encoded digest and length, the bound index, every frame and the complete decoded name and length before returning an appendable preparation. It neither copies nor recompresses the file. `Prepared::inspect` derives the descriptor from one locally owned encoded file and verifies it, allowing an encoded journal to retain its single-file atomic replacement. A remote caller still binds that encoding to its independently declared logical name and length. `Prepared::from_own_encoding` adopts an encoding the caller prepared itself and kept private, reopened from where it put it, with the descriptor its preparation returned: it checks the length and leaves the digest to the append, so a product can close every prepared file until its append without decoding each one a second time. The segment writer checks the encoded digest again while appending, so an externally mutable producer inode cannot change bytes between the check and the durable write. A failed append leaves an uncommitted tail for ordinary reopen. Products own authenticated transfer, storage ownership and encryption policy; a file handoff is never an arbitrary remote path contract.

A product opens `SegmentWriter::open(root, segment, committed_end, punches,
mode)`. The segment identifier is a relative path below `root`. The writer
holds an exclusive file lock until it and its sync thread close. Readers
hold separate file descriptors and take no lock.

```rust,ignore
let pending = writer.append_group(prepared.into_iter().map(Ok), |hash| product_index.lookup(hash))?;
// Preparation of another group can proceed while the dedicated worker syncs.
let group = pending.wait()?;
product_database.commit(&group.extents, group.end)?;
writer.acknowledge(&group)?;
```

The preparation input is a fallible iterator. A product can pin and fully verify one closed encoded member at a time; the writer drops it after appending and still syncs exactly once for the complete group. A preparation failure partway through the iterator poisons further appends; no group is returned for that tail, and ordinary reopen truncates it to the product's last committed end. Earlier synced groups retain their normal ordered acknowledgment semantics.

The returned extents have the same order and multiplicity as the inputs;
deduplication returns an existing descriptor. The database commits the full
descriptors and the group's end together with the rows referring to them.
`Extent::encode` and `Extent::decode` provide a binary column representation.
Acknowledgments must follow append order. An append or sync failure stops
further appends; the product reopens at its database's committed end.
Dropping a pending handle cannot commit its bytes. The product retains the
recorded punch list until its own retention policy allows removing it.

`Reader::new(root)` addresses every operation by `&[Extent]`. `read` verifies
each blob's whole plain or keyed name before returning a value. `open` and
`open_checked` return a `BlobReader` implementing `Read + Seek`;
`read_range` decodes only intersecting frames.
`open_checked_with_limits` accepts operation-specific `ReadLimits` for
encoded index size and frame size, checked before allocating those buffers. Each descriptor authenticates
the frame index, including the blob name; each frame has a digest, and
sealed frames also authenticate their position and a commitment to the complete
length table. That commitment is computed once, so encoding and reading stay
linear in the frame count.
Plain extents end in the standard zstd seek table and decode with stock
zstd. No range read needs a scan or hash lookup. `stats` reports decoded
frames, decoded bytes, encoded bytes and index bytes.

`Reader::scan` takes the product's complete list of live committed extents.
It checks encoded digests and frame indices, decodes plain frames and checks
plain names. Sealed frames are checked by their ciphertext digests without
requiring keys. It returns all mismatches and opens files read only.
`scan_checked` additionally accepts cancellation. Recorded punched extents
are excluded from the live scan list by the product.

`Preparation::chunked(reader, level, ChunkSizes, key, lookup)` plans a stream
as chunks, reading it once with at most twice the maximum chunk length in
memory. `key` seals every chunk and keys the cut points; `None` stores plain
chunks. Each chunk's name is passed to `lookup` (the product's committed hash
index, and `SegmentWriter::uncommitted` for groups awaiting commit): a known
chunk becomes `GroupEntry::Stored(extent)` and is neither compressed nor
written, any other becomes `GroupEntry::Prepared`. A lookup answer whose name,
length or kind differs from the chunk is refused. `chunked_checked` accepts a
cancellation callback. The returned `Chunked` holds the entries, the stream's
name and its length. `append_group` takes any items convertible to
`GroupEntry`, a `Prepared` included; a stored entry is returned unchanged in
its place, so the group's extents for a revision's entries, in order, are that
revision. `Chunker` exposes the cut points alone (`cut`, `chunks`) for products
that need boundaries without storage. `ChunkSizes::new` requires
`0 < min <= target <= max` with the target a power of two of at least 8.

`cargo run -p hashed-blobs --example group_metrics -- 10000` reports material
throughput, group count, input bytes, appended bytes and byte amplification
with a one-second group cadence. These synthetic figures qualify the shared
container. Product first-build amplification is measured by the product's
integration run.

`cargo run --release -p hashed-blobs --example rewrite_metrics -- 200 3000`
stores a synthetic session snapshot rewritten 200 times, gaining a user and an
assistant turn each time, once as one blob per revision and once as chunks,
and reports appended bytes per final and per input byte, blobs appended,
preparation rate (chunking, hashing and compression), time spent committing
one group with its data sync per revision, read rate, and checks every
revision reads back byte-exact. Commit time is the storage device's sync
latency and is reported apart from the hardware-independent byte figures.
