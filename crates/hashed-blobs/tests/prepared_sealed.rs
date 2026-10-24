use hashed_blobs::{DataKey, Error, Hash, Kind, Store, Verified};
use std::fs;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn store(root: &Path) -> Store {
    Store::open(root, vec![], 0o600)
        .unwrap()
        .with_frame_size(17)
}

fn key() -> DataKey {
    DataKey {
        key_ref: "synthetic-material-key".into(),
        key: [37; 32],
    }
}

fn stages(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root.join("blobs/tmp"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

fn destination(root: &Path, hash: Hash) -> PathBuf {
    let hex = hash.hex();
    root.join("blobs")
        .join(&hex[..2])
        .join(format!("{hex}.zst"))
}

fn read(s: &Store, hash: Hash, key: &DataKey) -> Vec<u8> {
    let mut bytes = Vec::new();
    s.read_with(&hash, &|reference| {
        if reference == key.key_ref {
            Ok(key.key)
        } else {
            Err("wrong reference".into())
        }
    })
    .unwrap()
    .read_to_end(&mut bytes)
    .unwrap();
    bytes
}

#[test]
fn hash_and_length_are_known_before_install_then_keyed_reads_match() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let key = key();
    for bytes in [
        &b""[..],
        &b"synthetic confidential body crossing several independently encrypted frames"[..],
    ] {
        let prepared = s.prepare_sealed(Cursor::new(bytes), 1, &key).unwrap();
        let hash = prepared.hash();
        assert_eq!(hash, Store::sealed_name(&key, bytes));
        assert_eq!(prepared.decoded_len(), bytes.len() as u64);
        assert!(!s.contains(&hash));
        assert!(matches!(s.read(&hash), Err(Error::Missing(_))));
        let staged = stages(temp.path());
        assert_eq!(staged.len(), 1);
        assert_eq!(
            fs::metadata(&staged[0]).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let encoded = fs::read(&staged[0]).unwrap();
        if !bytes.is_empty() {
            assert!(!encoded.windows(bytes.len()).any(|window| window == bytes));
        }
        assert_eq!(prepared.install().unwrap(), hash);
        assert!(stages(temp.path()).is_empty());
        assert_eq!(
            s.kind(&hash).unwrap(),
            Kind::Sealed {
                key_ref: key.key_ref.clone(),
                base: None
            }
        );
        assert!(matches!(s.read(&hash), Err(Error::Locked(_))));
        assert_eq!(s.verify(&hash).unwrap(), Verified::CiphertextIntact);
        assert_eq!(read(&s, hash, &key), bytes);
        assert_eq!(
            s.read_range_with(&hash, 0, bytes.len() as u64, &|_| Ok(key.key))
                .unwrap(),
            bytes
        );
        assert!(s.read_with(&hash, &|_| Ok([99; 32])).is_err());
    }
}

#[test]
fn owned_handle_keeps_origin_and_original_key_after_caller_handles_change() {
    let origin = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let bytes = b"owned encrypted preparation remains at its original store";
    let original = key();
    let mut supplied = original.clone();
    let origin_store = store(origin.path());
    origin_store
        .put_sealed(Cursor::new(&bytes[..]), 1, &original)
        .unwrap();
    let prepared = origin_store
        .prepare_sealed(Cursor::new(&bytes[..]), 1, &supplied)
        .unwrap();
    drop(origin_store);
    let hash = prepared.hash();
    supplied.key_ref = "changed-ref".into();
    supplied.key = [4; 32];
    drop(supplied);
    assert_eq!(
        std::thread::spawn(move || prepared.install())
            .join()
            .unwrap()
            .unwrap(),
        hash
    );
    assert!(!store(other.path()).contains(&hash));
    assert_eq!(read(&store(origin.path()), hash, &original), bytes);
}

#[test]
fn concurrent_preparations_use_fresh_encodings_and_authenticated_dedup() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let key = key();
    let bytes = b"same plaintext and key with independently prepared encodings";
    let a = s.prepare_sealed(Cursor::new(&bytes[..]), 1, &key).unwrap();
    let b = s.prepare_sealed(Cursor::new(&bytes[..]), 3, &key).unwrap();
    let hash = a.hash();
    assert_eq!(b.hash(), hash);
    let staged = stages(temp.path());
    assert_eq!(staged.len(), 2);
    assert_ne!(fs::read(&staged[0]).unwrap(), fs::read(&staged[1]).unwrap());
    let first = std::thread::spawn(move || a.install());
    let second = std::thread::spawn(move || b.install());
    assert_eq!(first.join().unwrap().unwrap(), hash);
    assert_eq!(second.join().unwrap().unwrap(), hash);
    let path = destination(temp.path(), hash);
    let inode = fs::metadata(&path).unwrap().ino();
    let encoding = fs::read(&path).unwrap();
    assert_eq!(
        s.put_sealed(Cursor::new(&bytes[..]), 2, &key).unwrap(),
        hash
    );
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::read(&path).unwrap(), encoding);
    assert_eq!(
        s.verify_with(&hash, &|_| Ok(key.key)).unwrap(),
        Verified::Intact
    );
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn drop_and_preinstall_refusal_discard_only_their_own_stage() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let key = key();
    let good = s
        .put_sealed(Cursor::new(b"already owned body"), 1, &key)
        .unwrap();
    let before = fs::read(destination(temp.path(), good)).unwrap();
    let unrelated = s
        .prepare_sealed(Cursor::new(b"another writer's bytes"), 1, &key)
        .unwrap();
    let aborted = s
        .prepare_sealed(Cursor::new(b"discarded bytes"), 1, &key)
        .unwrap();
    let aborted_hash = aborted.hash();
    drop(aborted);
    assert!(!s.contains(&aborted_hash));
    assert_eq!(stages(temp.path()).len(), 1);
    let cancelled = s
        .prepare_sealed(Cursor::new(b"already owned body"), 1, &key)
        .unwrap();
    let hash = cancelled.hash();
    let mut checks = 0;
    let result = cancelled.install_checked::<Error>(|| {
        checks += 1;
        Err(Error::Refused("cancelled before install".into()))
    });
    assert!(matches!(result, Err(Error::Refused(_))));
    assert_eq!(checks, 1);
    assert_eq!(hash, good);
    assert!(s.contains(&hash));
    assert_eq!(stages(temp.path()).len(), 1);
    assert_eq!(fs::read(destination(temp.path(), good)).unwrap(), before);
    drop(unrelated);
    assert!(stages(temp.path()).is_empty());
    assert_eq!(
        s.prepare_sealed(Cursor::new(b"accepted bytes"), 1, &key)
            .unwrap()
            .install_checked::<Error>(|| Ok(()))
            .unwrap(),
        Store::sealed_name(&key, b"accepted bytes")
    );
}

#[derive(Clone, Copy)]
enum Fault {
    NamingRead,
    Rewind,
    EncryptionRead,
}
struct FailedSource {
    bytes: Cursor<Vec<u8>>,
    fault: Fault,
    second: bool,
}
impl Read for FailedSource {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if matches!(self.fault, Fault::NamingRead)
            || self.second
                && matches!(self.fault, Fault::EncryptionRead)
                && self.bytes.position() >= 17
        {
            return Err(io::Error::other("synthetic source cancellation"));
        }
        self.bytes.read(out)
    }
}
impl Seek for FailedSource {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if matches!(self.fault, Fault::Rewind) {
            return Err(io::Error::other("synthetic rewind failure"));
        }
        self.second = true;
        self.bytes.seek(position)
    }
}

#[test]
fn source_errors_before_and_after_encrypted_frame_creation_remove_only_the_stage() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let key = key();
    let bytes = b"synthetic body long enough to finish one encrypted frame before failing";
    let hash = Store::sealed_name(&key, bytes);
    fs::write(
        temp.path().join("blobs/tmp/unrelated.part"),
        b"another operation's stage",
    )
    .unwrap();
    let good = s
        .put_sealed(Cursor::new(b"good existing body"), 1, &key)
        .unwrap();
    let encoded = fs::read(destination(temp.path(), good)).unwrap();
    for fault in [Fault::NamingRead, Fault::Rewind, Fault::EncryptionRead] {
        assert!(s
            .prepare_sealed(
                FailedSource {
                    bytes: Cursor::new(bytes.to_vec()),
                    fault,
                    second: false
                },
                1,
                &key
            )
            .is_err());
        assert!(!s.contains(&hash));
        assert_eq!(stages(temp.path()).len(), 1);
        assert_eq!(fs::read(destination(temp.path(), good)).unwrap(), encoded);
        assert_eq!(
            fs::read(temp.path().join("blobs/tmp/unrelated.part")).unwrap(),
            b"another operation's stage"
        );
    }
}

#[test]
fn different_key_reference_is_refused_even_when_keyed_name_matches() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let original = key();
    let bytes = b"one key reference";
    let hash = s.put_sealed(Cursor::new(&bytes[..]), 1, &original).unwrap();
    let path = destination(temp.path(), hash);
    let before = fs::read(&path).unwrap();
    let mut other = original.clone();
    other.key_ref = "different-reference".into();
    let prepared = s
        .prepare_sealed(Cursor::new(&bytes[..]), 1, &other)
        .unwrap();
    assert_eq!(prepared.hash(), hash);
    assert!(matches!(prepared.install(), Err(Error::Unwrap(_))));
    assert!(stages(temp.path()).is_empty());
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(read(&s, hash, &original), bytes);
}

#[test]
fn corrupt_sealed_destination_is_preserved_without_installing_over_it() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let key = key();
    let bytes = b"damaged existing sealed body";
    let hash = s.put_sealed(Cursor::new(&bytes[..]), 1, &key).unwrap();
    let path = destination(temp.path(), hash);
    let mut corrupt = fs::read(&path).unwrap();
    corrupt[0] ^= 1;
    fs::write(&path, &corrupt).unwrap();
    assert!(s
        .prepare_sealed(Cursor::new(&bytes[..]), 1, &key)
        .unwrap()
        .install()
        .is_err());
    assert!(stages(temp.path()).is_empty());
    assert_eq!(fs::read(&path).unwrap(), corrupt);
    assert!(s.put_sealed(Cursor::new(&bytes[..]), 1, &key).is_err());
    assert!(stages(temp.path()).is_empty());
    assert_eq!(fs::read(path).unwrap(), corrupt);
}

#[test]
fn a_sync_error_after_linking_keeps_the_installed_sealed_blob() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let key = key();
    let bytes = b"sealed blob linked before a sync failure";
    let prepared = s.prepare_sealed(Cursor::new(&bytes[..]), 1, &key).unwrap();
    let hash = prepared.hash();
    let path = destination(temp.path(), hash);
    let dir = path.parent().unwrap();
    fs::create_dir(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o300)).unwrap();
    let denied = fs::File::open(dir);
    let result = prepared.install();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(denied.is_err(), "requires actual directory read denial");
    assert!(matches!(result, Err(Error::Io(_))));
    assert!(stages(temp.path()).is_empty());
    assert_eq!(read(&s, hash, &key), bytes);
    let inode = fs::metadata(&path).unwrap().ino();
    assert_eq!(
        s.put_sealed(Cursor::new(&bytes[..]), 1, &key).unwrap(),
        hash
    );
    assert_eq!(fs::metadata(path).unwrap().ino(), inode);
}
