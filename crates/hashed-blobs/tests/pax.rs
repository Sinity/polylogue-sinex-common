use hashed_blobs::{PaxArchive, PaxSparseExtent, PaxTime};
use std::io::{Cursor, Read, Seek, SeekFrom};

fn octal(field: &mut [u8], value: u64) {
    let value = format!("{:0width$o}\0", value, width = field.len() - 1);
    field.copy_from_slice(value.as_bytes());
}
fn header(kind: u8, name: &[u8], size: u64) -> [u8; 512] {
    let mut h = [0; 512];
    h[..name.len()].copy_from_slice(name);
    octal(&mut h[100..108], 0o640);
    octal(&mut h[108..116], 0);
    octal(&mut h[116..124], 0);
    octal(&mut h[124..136], size);
    octal(&mut h[136..148], 42);
    h[156] = kind;
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    checksum(&mut h);
    h
}
fn checksum(h: &mut [u8; 512]) {
    h[148..156].fill(b' ');
    let sum: u64 = h.iter().map(|b| u64::from(*b)).sum();
    let value = format!("{sum:06o}\0 ");
    h[148..156].copy_from_slice(value.as_bytes());
}
fn append(tar: &mut Vec<u8>, kind: u8, name: &[u8], data: &[u8]) -> u64 {
    let at = tar.len() as u64;
    tar.extend(header(kind, name, data.len() as u64));
    tar.extend(data);
    tar.resize(tar.len().div_ceil(512) * 512, 0);
    at
}
fn pax(tar: &mut Vec<u8>, kind: u8, fields: &[(&[u8], &[u8])]) {
    let mut data = Vec::new();
    for (key, value) in fields {
        let rest = key.len() + value.len() + 3;
        let mut len = rest + 1;
        loop {
            let next = rest + len.to_string().len();
            if next == len {
                break;
            }
            len = next;
        }
        data.extend(format!("{len} ").as_bytes());
        data.extend(*key);
        data.push(b'=');
        data.extend(*value);
        data.push(b'\n');
    }
    append(tar, kind, b"PaxHeaders/member", &data);
}
fn finish(mut tar: Vec<u8>) -> Vec<u8> {
    tar.extend([0; 1024]);
    tar
}

#[test]
fn raw_names_metadata_sparse_ranges_and_original_offsets_survive() {
    let name = b"native\xff\nname";
    let mut tar = Vec::new();
    pax(
        &mut tar,
        b'x',
        &[
            (b"hdrcharset", b"BINARY"),
            (b"path", name),
            (b"uid", b"123456789"),
            (b"gid", b"987654321"),
            (b"uname", b"owner\xff"),
            (b"gname", b"group\xfe"),
            (b"mtime", b"-0.125000001"),
            (b"atime", b"123.123456789"),
            (b"ctime", b"-12.000000001"),
            (b"SCHILY.xattr.user.binary", b"value\0\n\xff"),
            (b"SCHILY.xattr.user.empty", b""),
            (b"SCHILY.acl.access", b"user::rw-,group::r--,other::---"),
            (b"GNU.sparse.map", b"2,3,12,2,20,0"),
            (b"GNU.sparse.size", b"20"),
            (b"GNU.sparse.numblocks", b"3"),
        ],
    );
    let at = append(&mut tar, b'0', b"placeholder", b"abcde");
    let mut archive = PaxArchive::open(Cursor::new(finish(tar))).unwrap();
    let member = &archive.members()[0];
    assert_eq!(member.name, name);
    assert_eq!((member.header_offset, member.data_offset), (at, at + 512));
    assert_eq!(
        (member.mode, member.uid, member.gid),
        (0o640, 123456789, 987654321)
    );
    assert_eq!(member.uname, b"owner\xff");
    assert_eq!(member.gname, b"group\xfe");
    assert_eq!(
        member.mtime,
        Some(PaxTime {
            seconds: -1,
            nanoseconds: 874999999
        })
    );
    assert_eq!(
        member.atime,
        Some(PaxTime {
            seconds: 123,
            nanoseconds: 123456789
        })
    );
    assert_eq!(
        member.ctime,
        Some(PaxTime {
            seconds: -13,
            nanoseconds: 999999999
        })
    );
    assert_eq!(member.xattrs[b"user.binary".as_slice()], b"value\0\n\xff");
    assert_eq!(member.xattrs[b"user.empty".as_slice()], b"");
    assert_eq!(
        member.acls[b"access".as_slice()],
        b"user::rw-,group::r--,other::---"
    );
    let mut expected = vec![0; 20];
    expected[2..5].copy_from_slice(b"abc");
    expected[12..14].copy_from_slice(b"de");
    assert_eq!(archive.read_range(0, 0, usize::MAX).unwrap(), expected);
    for from in 0..=20 {
        for len in 0..=24 {
            assert_eq!(
                archive.read_range(0, from, len).unwrap(),
                expected[from as usize..(from as usize + len).min(20)]
            );
        }
    }
    assert!(archive.read_range(0, 21, 0).is_err());
    assert!(archive.read_range(1, 0, 0).is_err());
}

#[test]
fn global_local_deletions_raw_links_ustar_prefix_and_base256_numbers() {
    let mut tar = Vec::new();
    pax(
        &mut tar,
        b'g',
        &[
            (b"uid", b"1000"),
            (b"path", b"global"),
            (b"vendor.fact", b"retained"),
        ],
    );
    pax(
        &mut tar,
        b'x',
        &[
            (b"uid", b"2000"),
            (b"path", b""),
            (b"linkpath", b"target\xff"),
        ],
    );
    let mut h = header(b'2', b"link", 0);
    h[345..351].copy_from_slice(b"prefix");
    // GNU base-256 negative timestamps are signed, while uid remains positive.
    h[136..148].fill(0xff);
    checksum(&mut h);
    tar.extend(h);
    append(&mut tar, b'0', b"ignored", b"data");
    pax(&mut tar, b'g', &[(b"uid", b""), (b"path", b"")]);
    let mut h = header(b'0', b"last", 0);
    h[108..116].fill(0);
    h[108] = 0x80;
    h[114] = 1;
    h[115] = 2;
    checksum(&mut h);
    tar.extend(h);
    let mut archive = PaxArchive::open(Cursor::new(finish(tar))).unwrap();
    let members = archive.members();
    assert_eq!(members.len(), 3);
    assert_eq!(members[0].name, b"prefix/link");
    assert_eq!(members[0].link_name, b"target\xff");
    assert_eq!(members[0].uid, 2000);
    assert_eq!(
        members[0].mtime,
        Some(PaxTime {
            seconds: -1,
            nanoseconds: 0
        })
    );
    assert_eq!(members[1].name, b"global");
    assert_eq!(members[1].uid, 1000);
    assert_eq!(members[2].uid, 258);
    assert_eq!(members[2].name, b"last");
    assert_eq!(members[2].fields[b"vendor.fact".as_slice()], b"retained");
    assert_eq!(archive.read_range(1, 1, 100).unwrap(), b"ata");
}

#[test]
fn repeated_sparse_00_records_and_sparse_10_body_maps_expand() {
    let mut tar = Vec::new();
    pax(
        &mut tar,
        b'x',
        &[
            (b"GNU.sparse.size", b"12"),
            (b"GNU.sparse.numblocks", b"2"),
            (b"GNU.sparse.offset", b"1"),
            (b"GNU.sparse.numbytes", b"2"),
            (b"GNU.sparse.offset", b"10"),
            (b"GNU.sparse.numbytes", b"2"),
        ],
    );
    append(&mut tar, b'0', b"old-map", b"abcd");
    pax(
        &mut tar,
        b'x',
        &[
            (b"GNU.sparse.major", b"1"),
            (b"GNU.sparse.minor", b"0"),
            (b"GNU.sparse.realsize", b"12"),
            (b"GNU.sparse.name", b"body-map"),
        ],
    );
    let mut data = b"2\n1\n2\n10\n2\n".to_vec();
    data.resize(512, 0);
    data.extend(b"abcd");
    append(&mut tar, b'0', b"placeholder", &data);
    let mut archive = PaxArchive::open(Cursor::new(finish(tar))).unwrap();
    assert_eq!(archive.members()[1].name, b"body-map");
    assert_eq!(archive.members()[1].stored_size, 4);
    assert_eq!(
        archive.members()[0].sparse,
        Some(vec![
            PaxSparseExtent { offset: 1, len: 2 },
            PaxSparseExtent { offset: 10, len: 2 }
        ])
    );
    assert_eq!(
        archive.read_range(0, 0, 12).unwrap(),
        b"\0ab\0\0\0\0\0\0\0cd"
    );
    assert_eq!(
        archive.read_range(1, 0, 12).unwrap(),
        b"\0ab\0\0\0\0\0\0\0cd"
    );
}

#[test]
fn malformed_headers_records_and_sparse_maps_fail_before_bytes_are_returned() {
    let mut tar = Vec::new();
    append(&mut tar, b'0', b"member", b"content");
    let original = finish(tar);
    let mut damaged = original.clone();
    damaged[0] ^= 1;
    assert!(PaxArchive::open(Cursor::new(damaged)).is_err());
    assert!(PaxArchive::open(Cursor::new(original[..600].to_vec())).is_err());
    let mut tar = Vec::new();
    append(&mut tar, b'x', b"bad-pax", b"99 path=short\n");
    append(&mut tar, b'0', b"member", b"content");
    assert!(PaxArchive::open(Cursor::new(finish(tar))).is_err());
    for map in [b"5,2,4,2".as_slice(), b"0,8", b"11,0", b"0", b"0,3"] {
        let mut tar = Vec::new();
        pax(
            &mut tar,
            b'x',
            &[(b"GNU.sparse.map", map), (b"GNU.sparse.size", b"10")],
        );
        append(&mut tar, b'0', b"bad-map", b"data");
        assert!(
            PaxArchive::open(Cursor::new(finish(tar))).is_err(),
            "{map:?}"
        );
    }
}

struct HeaderOnly {
    inner: Cursor<Vec<u8>>,
    forbidden: std::ops::Range<u64>,
}
impl Read for HeaderOnly {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        assert!(
            !self.forbidden.contains(&self.inner.position()),
            "indexing read file content"
        );
        self.inner.read(bytes)
    }
}
impl Seek for HeaderOnly {
    fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(from)
    }
}
#[test]
fn indexing_seeks_over_file_content() {
    let mut tar = Vec::new();
    append(&mut tar, b'0', b"member", &[1; 4096]);
    let input = HeaderOnly {
        inner: Cursor::new(finish(tar)),
        forbidden: 512..4608,
    };
    assert_eq!(PaxArchive::open(input).unwrap().members()[0].size, 4096);
}

#[cfg(target_os = "linux")]
mod gnu_tar {
    use super::*;
    use std::ffi::{CString, OsString};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
    use std::path::Path;
    use std::process::Command;

    fn available() -> bool {
        let available = Command::new("tar")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success() && o.stdout.starts_with(b"tar (GNU tar)"));
        if !available {
            eprintln!("GNU tar unavailable; extraction oracle skipped");
        }
        available
    }
    fn command(args: &[&str], archive: &Path, directory: &Path) {
        let output = Command::new("tar")
            .args(args)
            .arg("--file")
            .arg(archive)
            .arg("--directory")
            .arg(directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn acl() -> Vec<u8> {
        let mut value = 2u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in [
            (1u16, 6u16, u32::MAX),
            (2, 4, 12345),
            (4, 4, u32::MAX),
            (16, 4, u32::MAX),
            (32, 0, u32::MAX),
        ] {
            value.extend(tag.to_le_bytes());
            value.extend(permissions.to_le_bytes());
            value.extend(id.to_le_bytes());
        }
        value
    }
    fn xattr(path: &Path, name: &[u8]) -> Vec<u8> {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = CString::new(name).unwrap();
        let size = unsafe { libc::getxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        assert!(size >= 0, "{}", std::io::Error::last_os_error());
        let mut value = vec![0; size as usize];
        let size = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                value.as_mut_ptr().cast(),
                value.len(),
            )
        };
        assert!(size >= 0, "{}", std::io::Error::last_os_error());
        value.truncate(size as usize);
        value
    }
    fn set_xattr(path: &Path, name: &[u8], value: &[u8]) {
        let path = CString::new(path.as_os_str().as_bytes()).unwrap();
        let name = CString::new(name).unwrap();
        assert_eq!(
            unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            },
            0,
            "{}",
            std::io::Error::last_os_error()
        );
    }
    fn assert_extracted(archive: &mut PaxArchive<Cursor<Vec<u8>>>, member: usize, path: &Path) {
        let meta = std::fs::metadata(path).unwrap();
        let entry = &archive.members()[member];
        assert_eq!(meta.mode() & 0o7777, entry.mode as u32);
        assert_eq!(u64::from(meta.uid()), entry.uid);
        assert_eq!(u64::from(meta.gid()), entry.gid);
        assert_eq!(meta.len(), entry.size);
        assert_eq!(
            entry.mtime,
            Some(PaxTime {
                seconds: meta.mtime(),
                nanoseconds: meta.mtime_nsec() as u32
            })
        );
        assert!(meta.blocks() * 512 < meta.len());
        for (name, value) in &entry.xattrs {
            assert_eq!(&xattr(path, name), value);
        }
        let expected_acl = acl();
        assert_eq!(xattr(path, b"system.posix_acl_access"), expected_acl);
        // GNU tar writes newline-separated ACL entries. Other producers use
        // commas and may append a numeric qualifier after the permissions.
        let access_acl = &entry.acls[b"access".as_slice()];
        assert_eq!(access_acl, &entry.fields[b"SCHILY.acl.access".as_slice()]);
        let entries: Vec<Vec<u8>> = access_acl
            .split(|b| *b == b'\n' || *b == b',')
            .filter(|entry| !entry.is_empty())
            .map(|entry| {
                entry
                    .split(|b| *b == b':')
                    .take(3)
                    .collect::<Vec<_>>()
                    .join(&b":"[..])
            })
            .collect();
        assert_eq!(
            entries,
            [
                b"user::rw-".to_vec(),
                b"user:12345:r--".to_vec(),
                b"group::r--".to_vec(),
                b"mask::r--".to_vec(),
                b"other::---".to_vec()
            ]
        );
        let actual = archive.read_range(member, 0, usize::MAX).unwrap();
        let expected = std::fs::read(path).unwrap();
        let first_difference = actual.iter().zip(&expected).position(|(a, b)| a != b);
        assert!(
            actual == expected,
            "reader length {} tar length {}, first difference {:?}, reader BLAKE3 {} tar BLAKE3 {}",
            actual.len(),
            expected.len(),
            first_difference,
            blake3::hash(&actual),
            blake3::hash(&expected)
        );
    }

    #[test]
    fn pax_members_match_gnu_tar_names_metadata_xattrs_acls_and_sparse_bytes() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let extracted = root.path().join("extracted");
        std::fs::create_dir(&extracted).unwrap();
        let identity = std::fs::metadata(&extracted).unwrap();
        let uid = identity.uid().to_string();
        let gid = identity.gid().to_string();
        let mut name = vec![b'n'; 160];
        name.extend(b"\npercent%\xff.bin");
        let access_acl = acl();
        let mut tar = Vec::new();
        pax(
            &mut tar,
            b'x',
            &[
                (b"hdrcharset", b"BINARY"),
                (b"path", &name),
                (b"uid", uid.as_bytes()),
                (b"gid", gid.as_bytes()),
                (b"mtime", b"-0.125000001"),
                (b"atime", b"123.000000004"),
                (b"ctime", b"456.000000007"),
                (b"SCHILY.xattr.user.synthetic", b"binary\0value\n"),
                (b"SCHILY.xattr.user.empty", b""),
                (b"SCHILY.xattr.system.posix_acl_access", &access_acl),
                (
                    b"SCHILY.acl.access",
                    b"user::rw-,user:12345:r--,group::r--,mask::r--,other::---",
                ),
                (b"GNU.sparse.size", b"2097152"),
                (b"GNU.sparse.numblocks", b"3"),
                (b"GNU.sparse.name", &name),
                (b"GNU.sparse.map", b"0,512,1048579,4,2097152,0"),
            ],
        );
        // GNU tar produces block-sized interior fragments. Its extractor
        // advances to the next 512-byte block after each fragment, so this
        // oracle fixture follows that convention. The standalone map tests
        // above still cover arbitrary unaligned ranges without a GNU oracle.
        let mut packed = b"head".to_vec();
        packed.resize(512, 0);
        packed.extend(b"tail");
        append(&mut tar, b'0', b"GNUSparseFile/data", &packed);
        let tar = finish(tar);
        let file = root.path().join("archive.tar");
        std::fs::write(&file, &tar).unwrap();
        command(
            &["--extract", "--xattrs", "--acls", "--same-permissions"],
            &file,
            &extracted,
        );
        let mut archive = PaxArchive::open(Cursor::new(tar)).unwrap();
        assert_eq!(archive.members()[0].name, name);
        assert_eq!(
            archive.members()[0].atime,
            Some(PaxTime {
                seconds: 123,
                nanoseconds: 4
            })
        );
        assert_eq!(
            archive.members()[0].ctime,
            Some(PaxTime {
                seconds: 456,
                nanoseconds: 7
            })
        );
        let path = extracted.join(OsString::from_vec(name));
        assert_extracted(&mut archive, 0, &path);
    }

    #[test]
    fn gnu_tar_generated_pax_sparse_versions_match_extracted_files() {
        if !available() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        let name = OsString::from_vec(b"native\xff\nfile".to_vec());
        let path = source.join(&name);
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(2 << 20).unwrap();
        file.write_all_at(b"head", 0).unwrap();
        file.write_all_at(b"tail", (1 << 20) + 3).unwrap();
        file.set_permissions(std::fs::Permissions::from_mode(0o640))
            .unwrap();
        set_xattr(&path, b"user.synthetic", b"binary\0value\n");
        set_xattr(&path, b"system.posix_acl_access", &acl());
        file.set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_nanos(125000001))
            .unwrap();
        for version in ["0.0", "0.1", "1.0"] {
            let archive_file = root.path().join(format!("archive-{version}.tar"));
            let output = Command::new("tar")
                .args(["--create", "--format=pax", "--sparse", "--xattrs", "--acls"])
                .arg(format!("--sparse-version={version}"))
                .arg("--file")
                .arg(&archive_file)
                .arg("--directory")
                .arg(&source)
                .arg("--")
                .arg(&name)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let extracted = root.path().join(format!("extracted-{version}"));
            std::fs::create_dir(&extracted).unwrap();
            command(
                &["--extract", "--xattrs", "--acls", "--same-permissions"],
                &archive_file,
                &extracted,
            );
            let mut archive =
                PaxArchive::open(Cursor::new(std::fs::read(archive_file).unwrap())).unwrap();
            assert_eq!(archive.members().len(), 1);
            assert_eq!(archive.members()[0].name, name.as_bytes());
            assert!(archive.members()[0].sparse.is_some());
            assert_extracted(&mut archive, 0, &extracted.join(&name));
        }
    }
}
