use hashed_blobs::{Error, Hash, Store, Verified};
use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

fn store(root: &Path) -> Store {
    Store::open(root, vec![], 0o600).unwrap().with_frame_size(7)
}

fn destination(root: &Path, hash: Hash) -> PathBuf {
    let hex = hash.hex();
    root.join("blobs")
        .join(&hex[..2])
        .join(format!("{hex}.zst"))
}

fn stages(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root.join("blobs/tmp"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

#[test]
fn hash_and_length_are_known_before_install_then_reads_match() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    for bytes in [&b""[..], &b"synthetic body crossing several frames"[..]] {
        let prepared = s.prepare_plain(bytes, 1).unwrap();
        let hash = prepared.hash();
        assert_eq!(hash, Hash::of(bytes));
        assert_eq!(prepared.decoded_len(), bytes.len() as u64);
        assert!(!s.contains(&hash));
        assert!(matches!(s.read(&hash), Err(Error::Missing(_))));
        assert_eq!(stages(temp.path()).len(), 1);
        assert_eq!(prepared.install().unwrap(), hash);
        let mut full = Vec::new();
        s.read(&hash).unwrap().read_to_end(&mut full).unwrap();
        assert_eq!(full, bytes);
        assert_eq!(s.read_range(&hash, 0, bytes.len() as u64).unwrap(), bytes);
        assert_eq!(s.verify(&hash).unwrap(), Verified::Intact);
        assert!(stages(temp.path()).is_empty());
    }
}

#[test]
fn concurrent_prepared_writers_deduplicate_without_replacing_the_first_encoding() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let bytes = b"two prepared writers share the same decoded body";
    let first = s.prepare_plain(&bytes[..], 1).unwrap();
    let second = s.prepare_plain(&bytes[..], 3).unwrap();
    assert_eq!(first.hash(), second.hash());
    assert_eq!(stages(temp.path()).len(), 2);
    let hash = first.hash();
    std::thread::scope(|scope| {
        let a = scope.spawn(move || first.install());
        let b = scope.spawn(move || second.install_checked::<Error>(|| Ok(())));
        assert_eq!(a.join().unwrap().unwrap(), hash);
        assert_eq!(b.join().unwrap().unwrap(), hash);
    });
    let path = destination(temp.path(), hash);
    let inode = fs::metadata(&path).unwrap().ino();
    let encoding = fs::read(&path).unwrap();
    assert_eq!(s.put(&bytes[..], 2).unwrap(), hash);
    assert_eq!(
        s.put_plain_checked::<Error>(&bytes[..], 2, || Ok(()))
            .unwrap(),
        hash
    );
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::read(path).unwrap(), encoding);
    assert_eq!(s.verify(&hash).unwrap(), Verified::Intact);
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn dropping_or_cancelling_a_stage_preserves_unrelated_stages_and_shared_cas() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let bytes = b"already referenced by another writer";
    let hash = s.put(&bytes[..], 1).unwrap();
    let path = destination(temp.path(), hash);
    let before = fs::read(&path).unwrap();
    let unrelated = s.prepare_plain(&b"other uninstalled bytes"[..], 1).unwrap();
    let aborted = s.prepare_plain(&bytes[..], 1).unwrap();
    drop(aborted);
    assert_eq!(stages(temp.path()).len(), 1);
    let cancelled = s.prepare_plain(&bytes[..], 1).unwrap();
    assert!(cancelled
        .install_checked::<Error>(|| Err(Error::Refused("cancelled".into())))
        .is_err());
    assert_eq!(stages(temp.path()).len(), 1);
    assert_eq!(fs::read(&path).unwrap(), before);
    unrelated.install().unwrap();
    assert!(stages(temp.path()).is_empty());
}

struct FailingInput(bool);
impl Read for FailingInput {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.0 {
            return Err(io::Error::other("synthetic input fault"));
        }
        self.0 = true;
        out[0] = b'x';
        Ok(1)
    }
}

#[test]
fn preparation_read_fault_and_midstream_cancellation_clean_only_their_stage() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    fs::write(
        temp.path().join("blobs/tmp/unrelated.part"),
        b"owned elsewhere",
    )
    .unwrap();
    assert!(s.prepare_plain(FailingInput(false), 1).is_err());
    let mut polls = 0;
    let result = s.prepare_plain_checked::<Error>(&b"synthetic body across frames"[..], 1, || {
        polls += 1;
        if polls == 6 {
            Err(Error::Refused("cancelled".into()))
        } else {
            Ok(())
        }
    });
    assert!(result.is_err());
    assert_eq!(stages(temp.path()).len(), 1);
    assert_eq!(
        fs::read(temp.path().join("blobs/tmp/unrelated.part")).unwrap(),
        b"owned elsewhere"
    );
}

#[test]
fn corrupt_existing_cas_is_refused_and_preserved_while_stage_is_discarded() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let bytes = b"bytes with a damaged existing copy";
    let hash = s.put(&bytes[..], 1).unwrap();
    let path = destination(temp.path(), hash);
    fs::write(&path, b"corrupt synthetic file").unwrap();
    assert!(s.prepare_plain(&bytes[..], 1).unwrap().install().is_err());
    assert!(s
        .prepare_plain(&bytes[..], 1)
        .unwrap()
        .install_checked::<Error>(|| Ok(()))
        .is_err());
    assert_eq!(fs::read(path).unwrap(), b"corrupt synthetic file");
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn a_sync_error_after_linking_keeps_the_installed_blob() {
    // The declared checks run as an unprivileged user: write/search allows the
    // hard link, while lack of directory read permission faults its later fsync.
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let bytes = b"linked before a directory sync failure";
    let prepared = s.prepare_plain(&bytes[..], 1).unwrap();
    let hash = prepared.hash();
    let path = destination(temp.path(), hash);
    let dir = path.parent().unwrap();
    fs::create_dir(dir).unwrap();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o300)).unwrap();
    let directory_read = fs::File::open(dir);
    let result = prepared.install();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(directory_read.is_err(), "requires directory read denial");
    assert!(matches!(result, Err(Error::Io(_))));
    assert!(s.contains(&hash));
    assert_eq!(s.verify(&hash).unwrap(), Verified::Intact);
    assert!(stages(temp.path()).is_empty());
    assert_eq!(s.put(&bytes[..], 1).unwrap(), hash);
}

#[test]
fn owned_preparation_keeps_its_origin_after_store_is_dropped() {
    let origin = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let bytes = b"owned preparation remains bound to origin";
    let prepared = store(origin.path()).prepare_plain(&bytes[..], 1).unwrap();
    let hash = prepared.hash();
    let other_store = store(other.path());
    prepared.install().unwrap();
    assert!(store(origin.path()).contains(&hash));
    assert!(!other_store.contains(&hash));
}

#[test]
fn cancellation_during_existing_verification_preserves_the_good_body() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let bytes = b"good existing body crossing many frames for checked verification";
    let hash = s.put(&bytes[..], 1).unwrap();
    let path = destination(temp.path(), hash);
    let before = fs::read(&path).unwrap();
    let mut polls = 0;
    let result = s
        .prepare_plain(&bytes[..], 1)
        .unwrap()
        .install_checked::<Error>(|| {
            polls += 1;
            if polls == 5 {
                Err(Error::Refused("cancel during verification".into()))
            } else {
                Ok(())
            }
        });
    assert!(matches!(result, Err(Error::Refused(_))));
    assert_eq!(polls, 5);
    assert_eq!(fs::read(path).unwrap(), before);
    assert_eq!(s.verify(&hash).unwrap(), Verified::Intact);
    assert!(stages(temp.path()).is_empty());
}

#[test]
fn checked_install_refuses_existing_delta_while_normal_install_reuses_it() {
    let temp = tempfile::tempdir().unwrap();
    let s = store(temp.path());
    let base = s
        .put(&b"synthetic base for the delta encoding"[..], 1)
        .unwrap();
    let bytes = b"synthetic changed body";
    let hash = s.put_delta(&base, &bytes[..], 1).unwrap();
    let path = destination(temp.path(), hash);
    let before = fs::read(&path).unwrap();
    assert!(s
        .prepare_plain(&bytes[..], 1)
        .unwrap()
        .install_checked::<Error>(|| Ok(()))
        .is_err());
    assert_eq!(
        s.prepare_plain(&bytes[..], 1).unwrap().install().unwrap(),
        hash
    );
    assert_eq!(fs::read(path).unwrap(), before);
    assert!(stages(temp.path()).is_empty());
}
