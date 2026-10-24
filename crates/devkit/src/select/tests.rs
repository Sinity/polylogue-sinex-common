use super::*;

fn packages() -> Vec<(String, PathBuf)> {
    vec![
        ("model".into(), PathBuf::from("/w/crates/model")),
        ("store".into(), PathBuf::from("/w/crates/store")),
        ("xtask".into(), PathBuf::from("/w/xtask")),
    ]
}

fn files(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn changed_files_map_to_their_packages() {
    let root = Path::new("/w");
    let selection = select(
        root,
        &files(&["crates/store/src/lib.rs", "crates/model/README.md"]),
        &packages(),
    );
    assert_eq!(
        selection,
        Selection::Packages(vec!["model".into(), "store".into()])
    );
    assert_eq!(
        selection.filterset().unwrap(),
        "rdeps(=model) | rdeps(=store)"
    );
}

#[test]
fn a_file_outside_every_package_selects_the_workspace() {
    let root = Path::new("/w");
    assert_eq!(
        select(
            root,
            &files(&["crates/store/src/lib.rs", "flake.nix"]),
            &packages()
        ),
        Selection::Workspace
    );
    assert_eq!(
        select(root, &files(&["docs/development.md"]), &packages()),
        Selection::Nothing
    );
}
