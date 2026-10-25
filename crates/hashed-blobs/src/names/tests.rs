use std::collections::BTreeSet;
use std::fs;
use std::io;

use crate::{Error, Hash, Store};

fn store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("root"), vec![], 0o600).unwrap();
    (directory, store)
}

fn listed(store: &Store) -> BTreeSet<Hash> {
    store.names().unwrap().map(Result::unwrap).collect()
}

#[test]
fn an_empty_store_lists_nothing() {
    let (_directory, store) = store();
    assert!(listed(&store).is_empty());
}

#[test]
fn every_installed_blob_is_listed_once() {
    let (_directory, store) = store();
    let mut expected = BTreeSet::new();
    for seed in 0..40u32 {
        expected.insert(store.put(&seed.to_le_bytes()[..], 1).unwrap());
    }
    let base = store
        .put(&b"a base long enough to patch against"[..], 1)
        .unwrap();
    expected.insert(base);
    expected.insert(
        store
            .put_delta(
                &base,
                &b"a base long enough to patch against, changed"[..],
                1,
            )
            .unwrap(),
    );
    // Writing the same bytes again adds no file.
    store.put(&0u32.to_le_bytes()[..], 1).unwrap();
    assert_eq!(listed(&store), expected);
}

#[test]
fn prepared_blobs_and_extra_roots_are_not_listed() {
    let (directory, store) = store();
    let installed = store.put(&b"installed"[..], 1).unwrap();
    let _prepared = store
        .prepare_plain(&b"prepared, not installed"[..], 1)
        .unwrap();
    let extra = Store::open(directory.path().join("extra"), vec![], 0o600).unwrap();
    extra.put(&b"only in the extra root"[..], 1).unwrap();
    let reading_both = Store::open(
        directory.path().join("root"),
        vec![directory.path().join("extra")],
        0o600,
    )
    .unwrap();
    assert_eq!(listed(&reading_both), BTreeSet::from([installed]));
}

#[test]
fn a_deleted_blob_is_no_longer_listed() {
    let (_directory, store) = store();
    let kept = store.put(&b"kept"[..], 1).unwrap();
    let deleted = store.put(&b"deleted"[..], 1).unwrap();
    store.delete(&deleted).unwrap();
    assert_eq!(listed(&store), BTreeSet::from([kept]));
}

#[test]
fn a_stray_file_is_reported_and_the_listing_goes_on() {
    let (directory, store) = store();
    let blob = store.put(&b"a blob"[..], 1).unwrap();
    let blobs = directory.path().join("root").join("blobs");
    let shard = blobs.join(&blob.hex()[..2]);
    fs::write(shard.join("notes.txt"), b"").unwrap();
    // A correct name in the wrong shard is not where a read would look.
    let other = Hash([blob.0[0].wrapping_add(1); 32]);
    fs::write(shard.join(format!("{}.zst", other.hex())), b"").unwrap();
    fs::write(blobs.join("stray"), b"").unwrap();

    let mut found = Vec::new();
    let mut refused = Vec::new();
    for item in store.names().unwrap() {
        match item {
            Ok(hash) => found.push(hash),
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::InvalidData => {
                refused.push(error.to_string())
            }
            Err(error) => panic!("unexpected error: {error}"),
        }
    }
    assert_eq!(found, vec![blob]);
    refused.sort();
    assert_eq!(refused.len(), 3, "{refused:?}");
    assert!(refused.iter().any(|message| message.contains("notes.txt")));
    assert!(refused.iter().any(|message| message.contains("stray")));
    assert!(refused.iter().any(|message| message.contains(&other.hex())));
}

#[test]
fn hex_parsing_matches_how_names_are_written() {
    let hash = Hash::of(b"round trip");
    assert_eq!(
        super::blob_name(format!("{}.zst", hash.hex()).as_ref()),
        Some(hash)
    );
    assert_eq!(super::blob_name(hash.hex().as_ref()), None);
    let upper = format!("{}.zst", hash.hex().to_uppercase());
    assert_eq!(super::blob_name(upper.as_ref()), None);
    assert_eq!(super::shard_prefix("0f".as_ref()), Some(0x0f));
    assert_eq!(super::shard_prefix("0F".as_ref()), None);
    assert_eq!(super::shard_prefix("tmp".as_ref()), None);
}
