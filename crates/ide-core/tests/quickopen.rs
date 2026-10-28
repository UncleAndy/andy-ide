use ide_core::FileIndex;
use std::fs;

#[test]
fn finds_file_by_partial_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/ui")).unwrap();
    fs::write(dir.path().join("src/ui/editor_widget.rs"), "").unwrap();
    fs::write(dir.path().join("src/main.rs"), "").unwrap();
    let idx = FileIndex::build(dir.path(), false).unwrap();
    let hits = idx.search("edwid", 10);
    assert!(!hits.is_empty());
    assert!(
        hits[0].rel.to_string_lossy().contains("editor_widget"),
        "{:?}",
        hits.iter().map(|h| &h.rel).collect::<Vec<_>>()
    );
}

#[test]
fn empty_query_returns_bounded_list() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..30 {
        fs::write(dir.path().join(format!("f{i}.txt")), "").unwrap();
    }
    let idx = FileIndex::build(dir.path(), false).unwrap();
    assert_eq!(idx.search("", 5).len(), 5);
    assert_eq!(idx.len(), 30);
}

#[test]
fn directories_are_not_indexed_and_gitignore_is_honoured() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    fs::create_dir_all(dir.path().join("target/debug")).unwrap();
    fs::write(dir.path().join("target/debug/artifact.rs"), "").unwrap();
    fs::write(dir.path().join("src/lib.rs"), "").unwrap();
    let idx = FileIndex::build(dir.path(), false).unwrap();
    let paths: Vec<String> = idx
        .search("", 100)
        .into_iter()
        .map(|h| h.rel.to_string_lossy().into_owned())
        .collect();
    assert_eq!(paths, vec!["src/lib.rs"]);
}

#[test]
fn no_match_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("alpha.rs"), "").unwrap();
    let idx = FileIndex::build(dir.path(), false).unwrap();
    assert!(idx.search("zzzqqq", 10).is_empty());
}

#[test]
fn limit_zero_returns_nothing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("alpha.rs"), "").unwrap();
    let idx = FileIndex::build(dir.path(), false).unwrap();
    assert!(idx.search("alpha", 0).is_empty());
}
