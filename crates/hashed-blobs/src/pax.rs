//! POSIX PAX member tables over a seekable archive, preserving native byte names.
//!
//! Member offsets refer to the original stream. Sparse maps describe logical
//! data ranges; range reads fetch only their intersections and fill holes with
//! zeroes. Extension values remain bytes, including binary extended attributes.
use crate::{Error, Result};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};

type Fields = BTreeMap<Vec<u8>, Vec<u8>>;
type Records = Vec<(Vec<u8>, Vec<u8>)>;

/// A normalized Unix timestamp, including instants before the Unix epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaxTime {
    pub seconds: i64,
    pub nanoseconds: u32,
}

/// A data range in a sparse member's logical file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PaxSparseExtent {
    pub offset: u64,
    pub len: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaxMember {
    pub name: Vec<u8>,
    pub link_name: Vec<u8>,
    /// Offset of the member's ordinary tar header, after extension headers.
    pub header_offset: u64,
    /// Offset of packed file data, after a GNU sparse 1.0 map if present.
    pub data_offset: u64,
    /// Packed data length, excluding a GNU sparse 1.0 map.
    pub stored_size: u64,
    /// Logical length after sparse expansion.
    pub size: u64,
    /// The tar type flag (regular files use `b'0'` or zero).
    pub kind: u8,
    pub mode: u64,
    pub uid: u64,
    pub gid: u64,
    pub uname: Vec<u8>,
    pub gname: Vec<u8>,
    pub mtime: Option<PaxTime>,
    pub atime: Option<PaxTime>,
    pub ctime: Option<PaxTime>,
    /// Attribute names exclude the `SCHILY.xattr.` prefix.
    pub xattrs: Fields,
    /// ACL names exclude the `SCHILY.acl.` prefix (for example `access`).
    pub acls: Fields,
    /// Effective PAX fields, including vendor extensions this reader does not interpret.
    pub fields: Fields,
    pub sparse: Option<Vec<PaxSparseExtent>>,
}

pub struct PaxArchive<R> {
    reader: R,
    members: Vec<PaxMember>,
}

impl<R: Read + Seek> PaxArchive<R> {
    /// Index a tar stream from its beginning without reading regular file data.
    pub fn open(mut reader: R) -> Result<Self> {
        let end = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let mut members = Vec::new();
        let mut global = Fields::new();
        let mut local = Records::new();
        let mut long_name = None;
        let mut long_link = None;
        let mut at = 0;
        loop {
            let next = add(at, 512)?;
            if next > end {
                return invalid("tar stream has no complete end marker");
            }
            reader.seek(SeekFrom::Start(at))?;
            let mut header = [0; 512];
            reader.read_exact(&mut header)?;
            if header.iter().all(|b| *b == 0) {
                if add(next, 512)? > end {
                    return invalid("tar end marker is incomplete");
                }
                reader.read_exact(&mut header)?;
                if header.iter().any(|b| *b != 0) {
                    return invalid("tar end marker contains a nonzero block");
                }
                if !local.is_empty() || long_name.is_some() || long_link.is_some() {
                    return invalid("tar extension header has no member");
                }
                break;
            }
            checksum(&header)?;
            let header_size = unsigned(&header[124..136])?;
            let kind = header[156];
            if matches!(kind, b'x' | b'g' | b'L' | b'K') {
                let after = add(next, padded(header_size)?)?;
                if after > end {
                    return invalid("tar extension extends past the stream");
                }
                let bytes = read_vec(&mut reader, header_size)?;
                match kind {
                    b'x' => local.extend(records(&bytes)?),
                    b'g' => {
                        for (key, value) in records(&bytes)? {
                            if value.is_empty() {
                                global.remove(&key);
                            } else {
                                global.insert(key, value);
                            }
                        }
                    }
                    b'L' => long_name = Some(long_value(bytes)),
                    b'K' => long_link = Some(long_value(bytes)),
                    _ => unreachable!(),
                }
                at = after;
                continue;
            }
            if kind == b'S' {
                return invalid("old GNU sparse headers are not POSIX PAX members");
            }
            let mut fields = global.clone();
            for (key, value) in &local {
                if value.is_empty() && !key.starts_with(b"SCHILY.xattr.") {
                    fields.remove(key);
                } else {
                    fields.insert(key.clone(), value.clone());
                }
            }
            let stored = value_u64(&fields, b"size")?.unwrap_or(header_size);
            let after = add(next, padded(stored)?)?;
            if after > end {
                return invalid("tar member extends past the stream");
            }
            let mut name = nul(&header[..100]).to_vec();
            if &header[257..263] == b"ustar\0" {
                let prefix = nul(&header[345..500]);
                if !prefix.is_empty() {
                    name = [prefix, b"/", name.as_slice()].concat();
                }
            }
            name = fields
                .get(b"path".as_slice())
                .cloned()
                .or(long_name.take())
                .unwrap_or(name);
            let link_name = fields
                .get(b"linkpath".as_slice())
                .cloned()
                .or(long_link.take())
                .unwrap_or_else(|| nul(&header[157..257]).to_vec());
            let mut member = PaxMember {
                name,
                link_name,
                header_offset: at,
                data_offset: next,
                stored_size: stored,
                size: stored,
                kind,
                mode: unsigned(&header[100..108])?,
                uid: value_u64(&fields, b"uid")?.unwrap_or(unsigned(&header[108..116])?),
                gid: value_u64(&fields, b"gid")?.unwrap_or(unsigned(&header[116..124])?),
                uname: fields
                    .get(b"uname".as_slice())
                    .cloned()
                    .unwrap_or_else(|| nul(&header[265..297]).to_vec()),
                gname: fields
                    .get(b"gname".as_slice())
                    .cloned()
                    .unwrap_or_else(|| nul(&header[297..329]).to_vec()),
                mtime: Some(match fields.get(b"mtime".as_slice()) {
                    Some(value) => timestamp(value)?,
                    None => PaxTime {
                        seconds: i64::try_from(number(&header[136..148])?)
                            .map_err(|_| bad("mtime exceeds timestamp range"))?,
                        nanoseconds: 0,
                    },
                }),
                atime: fields
                    .get(b"atime".as_slice())
                    .map(|v| timestamp(v))
                    .transpose()?,
                ctime: fields
                    .get(b"ctime".as_slice())
                    .map(|v| timestamp(v))
                    .transpose()?,
                xattrs: prefixed(&fields, b"SCHILY.xattr."),
                acls: prefixed(&fields, b"SCHILY.acl."),
                sparse: None,
                fields,
            };
            sparse(&mut reader, &mut member, &local)?;
            local.clear();
            members.push(member);
            at = after;
        }
        Ok(Self { reader, members })
    }

    pub fn members(&self) -> &[PaxMember] {
        &self.members
    }
    pub fn into_inner(self) -> R {
        self.reader
    }

    /// Read up to `len` logical bytes; reads reaching EOF return a shorter value.
    pub fn read_range(&mut self, member: usize, offset: u64, len: usize) -> Result<Vec<u8>> {
        let entry = self
            .members
            .get(member)
            .ok_or_else(|| bad("unknown tar member"))?;
        let count = range_len(entry, offset, len)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(count)
            .map_err(|e| bad(&format!("cannot allocate member range: {e}")))?;
        bytes.resize(count, 0);
        self.read_range_into(member, offset, &mut bytes)?;
        Ok(bytes)
    }

    /// Fill a buffer from logical member bytes without allocating a file-sized value.
    pub fn read_range_into(
        &mut self,
        member: usize,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let entry = self
            .members
            .get(member)
            .ok_or_else(|| bad("unknown tar member"))?;
        let count = range_len(entry, offset, bytes.len())?;
        let bytes = &mut bytes[..count];
        if let Some(ranges) = &entry.sparse {
            bytes.fill(0);
            let end = add(offset, count as u64)?;
            let mut packed = entry.data_offset;
            for range in ranges {
                let range_end = add(range.offset, range.len)?;
                let start = offset.max(range.offset);
                let stop = end.min(range_end);
                if start < stop {
                    self.reader
                        .seek(SeekFrom::Start(add(packed, start - range.offset)?))?;
                    let from = (start - offset) as usize;
                    let to = (stop - offset) as usize;
                    self.reader.read_exact(&mut bytes[from..to])?;
                }
                packed = add(packed, range.len)?;
            }
        } else if count != 0 {
            self.reader
                .seek(SeekFrom::Start(add(entry.data_offset, offset)?))?;
            self.reader.read_exact(bytes)?;
        }
        Ok(count)
    }
}

fn range_len(member: &PaxMember, offset: u64, len: usize) -> Result<usize> {
    let remaining = member
        .size
        .checked_sub(offset)
        .ok_or_else(|| bad("member range starts after EOF"))?;
    Ok(len.min(usize::try_from(remaining).unwrap_or(usize::MAX)))
}

fn sparse<R: Read + Seek>(reader: &mut R, member: &mut PaxMember, local: &Records) -> Result<()> {
    let fields = &member.fields;
    let has_sparse = fields.keys().any(|key| key.starts_with(b"GNU.sparse."));
    if !has_sparse {
        return Ok(());
    }
    member.size = value_u64(fields, b"GNU.sparse.realsize")?
        .or(value_u64(fields, b"GNU.sparse.size")?)
        .ok_or_else(|| bad("sparse member has no logical size"))?;
    if let Some(name) = fields.get(b"GNU.sparse.name".as_slice()) {
        member.name = name.clone();
    }
    if let Some(major) = fields.get(b"GNU.sparse.major".as_slice()) {
        if major != b"0" && major != b"1" {
            return invalid("unsupported GNU sparse version");
        }
    }
    let ranges = if fields
        .get(b"GNU.sparse.major".as_slice())
        .is_some_and(|v| v == b"1")
    {
        if fields
            .get(b"GNU.sparse.minor".as_slice())
            .is_none_or(|v| v != b"0")
        {
            return invalid("unsupported GNU sparse version");
        }
        reader.seek(SeekFrom::Start(member.data_offset))?;
        let mut consumed = 0;
        let count = sparse_line(reader, &mut consumed, member.stored_size)?;
        let mut ranges = Vec::new();
        for _ in 0..count {
            let offset = sparse_line(reader, &mut consumed, member.stored_size)?;
            let len = sparse_line(reader, &mut consumed, member.stored_size)?;
            ranges.push(PaxSparseExtent { offset, len });
        }
        let map_size = padded(consumed)?;
        member.stored_size = member
            .stored_size
            .checked_sub(map_size)
            .ok_or_else(|| bad("sparse map exceeds member"))?;
        member.data_offset = add(member.data_offset, map_size)?;
        ranges
    } else if let Some(map) = fields.get(b"GNU.sparse.map".as_slice()) {
        let mut values = map.split(|b| *b == b',');
        let mut ranges = Vec::new();
        if !map.is_empty() {
            while let Some(offset) = values.next() {
                let len = values
                    .next()
                    .ok_or_else(|| bad("sparse map has an unmatched offset"))?;
                ranges.push(PaxSparseExtent {
                    offset: decimal(offset)?,
                    len: decimal(len)?,
                });
            }
        }
        ranges
    } else {
        let mut ranges = Vec::new();
        let mut offset = None;
        for (key, value) in local {
            match key.as_slice() {
                b"GNU.sparse.offset" => {
                    if offset.replace(decimal(value)?).is_some() {
                        return invalid("sparse offset has no length");
                    }
                }
                b"GNU.sparse.numbytes" => ranges.push(PaxSparseExtent {
                    offset: offset
                        .take()
                        .ok_or_else(|| bad("sparse length has no offset"))?,
                    len: decimal(value)?,
                }),
                _ => (),
            }
        }
        if offset.is_some() {
            return invalid("sparse offset has no length");
        }
        if !fields.contains_key(b"GNU.sparse.numblocks".as_slice()) {
            return invalid("sparse member has no map");
        }
        ranges
    };
    if let Some(count) = value_u64(fields, b"GNU.sparse.numblocks")? {
        if count != ranges.len() as u64 {
            return invalid("sparse range count disagrees with map");
        }
    }
    let mut end = 0;
    let mut packed = 0;
    for range in &ranges {
        if range.offset < end {
            return invalid("sparse ranges overlap or are out of order");
        }
        end = add(range.offset, range.len)?;
        if end > member.size {
            return invalid("sparse range extends past logical EOF");
        }
        packed = add(packed, range.len)?;
    }
    if packed != member.stored_size {
        return invalid("sparse map disagrees with packed data size");
    }
    member.sparse = Some(ranges);
    Ok(())
}

fn sparse_line<R: Read>(reader: &mut R, consumed: &mut u64, limit: u64) -> Result<u64> {
    let mut value = 0u64;
    let mut digits = false;
    loop {
        if *consumed == limit {
            return invalid("truncated sparse map");
        }
        let mut byte = [0];
        reader.read_exact(&mut byte)?;
        *consumed += 1;
        if byte[0] == b'\n' && digits {
            return Ok(value);
        }
        if !byte[0].is_ascii_digit() {
            return invalid("invalid sparse map number");
        }
        digits = true;
        value = value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(byte[0] - b'0')))
            .ok_or_else(|| bad("sparse map number exceeds addressable range"))?;
    }
}

fn prefixed(fields: &Fields, prefix: &[u8]) -> Fields {
    fields
        .iter()
        .filter_map(|(key, value)| {
            key.strip_prefix(prefix)
                .map(|name| (name.to_vec(), value.clone()))
        })
        .collect()
}

fn records(bytes: &[u8]) -> Result<Records> {
    let mut at = 0usize;
    let mut fields = Vec::new();
    while at < bytes.len() {
        let space = bytes[at..]
            .iter()
            .position(|b| *b == b' ')
            .ok_or_else(|| bad("PAX record has no length separator"))?
            + at;
        let len = usize::try_from(decimal(&bytes[at..space])?)
            .map_err(|_| bad("PAX record exceeds addressable size"))?;
        let end = at
            .checked_add(len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| bad("PAX record extends past header"))?;
        if end <= space + 1 || bytes[end - 1] != b'\n' {
            return invalid("invalid PAX record boundary");
        }
        let entry = &bytes[space + 1..end - 1];
        let equals = entry
            .iter()
            .position(|b| *b == b'=')
            .filter(|n| *n != 0)
            .ok_or_else(|| bad("PAX record has no keyword"))?;
        fields.push((entry[..equals].to_vec(), entry[equals + 1..].to_vec()));
        at = end;
    }
    Ok(fields)
}

fn timestamp(bytes: &[u8]) -> Result<PaxTime> {
    let (negative, bytes) = match bytes.first() {
        Some(b'-') => (true, &bytes[1..]),
        Some(b'+') => (false, &bytes[1..]),
        _ => (false, bytes),
    };
    let mut parts = bytes.split(|b| *b == b'.');
    let whole = decimal(parts.next().unwrap_or_default())?;
    let fraction = parts.next().unwrap_or_default();
    if parts.next().is_some() || fraction.iter().any(|b| !b.is_ascii_digit()) {
        return invalid("invalid PAX timestamp");
    }
    let mut nanos = 0u32;
    for at in 0..9 {
        nanos = nanos * 10 + u32::from(fraction.get(at).copied().unwrap_or(b'0') - b'0');
    }
    // PAX allows finer precision; Unix nanosecond metadata truncates it toward zero.
    let total = i128::from(whole) * 1_000_000_000 + i128::from(nanos);
    let total = if negative { -total } else { total };
    Ok(PaxTime {
        seconds: i64::try_from(total.div_euclid(1_000_000_000))
            .map_err(|_| bad("PAX timestamp exceeds Unix timestamp range"))?,
        nanoseconds: total.rem_euclid(1_000_000_000) as u32,
    })
}
fn value_u64(fields: &Fields, key: &[u8]) -> Result<Option<u64>> {
    fields.get(key).map(|value| decimal(value)).transpose()
}
fn decimal(bytes: &[u8]) -> Result<u64> {
    if bytes.is_empty() {
        return invalid("empty decimal number");
    }
    bytes.iter().try_fold(0u64, |value, b| {
        if !b.is_ascii_digit() {
            return invalid("invalid decimal number");
        }
        value
            .checked_mul(10)
            .and_then(|v| v.checked_add(u64::from(b - b'0')))
            .ok_or_else(|| bad("decimal number exceeds addressable range"))
    })
}
fn unsigned(bytes: &[u8]) -> Result<u64> {
    u64::try_from(number(bytes)?).map_err(|_| bad("negative or overflowing tar number"))
}
fn number(bytes: &[u8]) -> Result<i128> {
    if bytes[0] & 0x80 != 0 {
        let mut value = i128::from(bytes[0] & 0x7f);
        if bytes[0] & 0x40 != 0 {
            value -= 128;
        }
        for byte in &bytes[1..] {
            value = value
                .checked_mul(256)
                .and_then(|v| v.checked_add(i128::from(*byte)))
                .ok_or_else(|| bad("tar number overflow"))?;
        }
        return Ok(value);
    }
    let begin = bytes
        .iter()
        .position(|b| *b != 0 && *b != b' ')
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| *b != 0 && *b != b' ')
        .map_or(begin, |i| i + 1);
    let bytes = &bytes[begin..end];
    bytes.iter().try_fold(0i128, |value, b| {
        if !(b'0'..=b'7').contains(b) {
            return invalid("invalid tar octal number");
        }
        value
            .checked_mul(8)
            .and_then(|v| v.checked_add(i128::from(b - b'0')))
            .ok_or_else(|| bad("tar number overflow"))
    })
}
fn checksum(header: &[u8; 512]) -> Result<()> {
    let expected = unsigned(&header[148..156])?;
    let unsigned_sum: u64 = header
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                32
            } else {
                u64::from(*b)
            }
        })
        .sum();
    let signed_sum: i64 = header
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                32
            } else {
                i64::from(*b as i8)
            }
        })
        .sum();
    if expected != unsigned_sum && i64::try_from(expected).ok() != Some(signed_sum) {
        return invalid("tar header checksum mismatch");
    }
    Ok(())
}
fn nul(bytes: &[u8]) -> &[u8] {
    &bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len())]
}
fn long_value(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.last() == Some(&0) {
        bytes.pop();
    }
    bytes
}
fn read_vec(reader: &mut impl Read, size: u64) -> Result<Vec<u8>> {
    let size = usize::try_from(size).map_err(|_| bad("PAX header exceeds addressable memory"))?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|e| bad(&format!("cannot allocate PAX header: {e}")))?;
    bytes.resize(size, 0);
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}
fn padded(size: u64) -> Result<u64> {
    add(size, 511).map(|n| n / 512 * 512)
}
fn add(a: u64, b: u64) -> Result<u64> {
    a.checked_add(b).ok_or_else(|| bad("tar offset overflow"))
}
fn bad(message: &str) -> Error {
    Error::Refused(format!("PAX archive: {message}"))
}
fn invalid<T>(message: &str) -> Result<T> {
    Err(bad(message))
}
