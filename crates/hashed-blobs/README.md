# hashed-blobs

Content-addressed bytes for Polylogue and Sinex: blobs named by their hash,
stored in append-only segments, read back with their hashes checked, and
directory captures read member by member from PAX tar blobs. Both products
store every raw byte they keep through this crate, so the byte mechanics are
written once.

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
sealed blob reaches the disk.

An **extent** is the byte range in one segment that holds one blob's frames.
A product's rows cite extents; a list of extents read in order is one stream.

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
against another blob, and there is no second file-per-blob layout to read.

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
  Nothing decrypted is cached between calls.
- Reads never block the writer: committed extents never move, so a reader
  needs no lock beyond the segment file it holds open.

## PAX member access

A directory capture is stored as one blob holding a POSIX PAX tar stream:
members keep their raw byte file names (no conversion to UTF-8), mode, owner,
nanosecond times, extended attributes, POSIX ACLs and sparse-file maps in PAX
headers. The crate reads the member table of such a blob and returns any
member's metadata and bytes as a range of the blob, with sparse holes read as
zeros. A product anchors records at member offsets in the tar stream.

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

## Laws

- **crash-leaves-only-a-tail**: killing the writer at every point between
  append, sync, commit and truncate leaves segments that open at their last
  committed extent, with every committed extent reading back byte for byte.
- **open-truncates-uncommitted-tail**: bytes after the committed end passed at
  open are gone after open, and the next append starts there.
- **committed-extents-never-move**: once committed, an extent's offset and
  bytes stay the same until it is punched.
- **one-sync-per-group**: a group of any size issues one data sync.
- **dedup-across-uncommitted-groups**: a blob appended in a group that has not
  committed is not appended again by a later group.
- **read-checks-every-frame**: corrupting one byte of a stored frame makes the
  next read of it fail naming the segment and extent, and no read returns the
  damaged bytes.
- **scan-reports-and-repairs-nothing**: the scan names every damaged extent
  and leaves every segment byte for byte unchanged.
- **punch-removes-only-named-extents**: after a punch the named extents read
  as removed and every other extent reads unchanged.
- **recorded-punch-finishes-at-open**: a punch recorded before a crash and not
  performed is performed when the segment opens; repeating it changes nothing.
- **sealed-no-plaintext-on-disk**: writing and reading a sealed blob leaves no
  plaintext byte in any file.
- **sealed-tamper-fails**: changing, moving or truncating any sealed frame or
  frame-table entry fails authentication on read.
- **sealed-check-without-key**: the scan detects damage to a sealed blob
  without its key.
- **pax-members-match-gnu-tar**: every member read through the crate equals
  GNU tar's extraction of the same stream, including byte names, extended
  attributes, ACLs and sparse maps.
