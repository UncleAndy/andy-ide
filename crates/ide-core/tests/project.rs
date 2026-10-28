use ide_core::{EntryKind, Project, TreeOptions};
use std::fs;

fn opts() -> TreeOptions {
    TreeOptions { show_hidden: false }
}

#[test]
fn lists_directories_before_files() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("b.txt"), "").unwrap();
    fs::create_dir(dir.path().join("a")).unwrap();
    let p = Project::open(dir.path()).unwrap();
    let entries = p.list_dir(std::path::Path::new(""), &opts()).unwrap();
    assert_eq!(entries[0].name, "a");
    assert_eq!(entries[0].kind, EntryKind::Dir);
    assert_eq!(entries[1].name, "b.txt");
}

#[test]
fn hides_dotfiles_and_gitignored() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("keep.rs"), "").unwrap();
    fs::write(dir.path().join(".secret"), "").unwrap();
    fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
    fs::create_dir(dir.path().join("target")).unwrap();
    let p = Project::open(dir.path()).unwrap();
    let names: Vec<String> = p
        .list_dir(std::path::Path::new(""), &opts())
        .unwrap()
        .into_iter()
        .map(|e| e.name)
        .collect();
    assert_eq!(names, vec!["keep.rs"]);
}

#[test]
fn opening_nonexistent_root_fails() {
    assert!(Project::open("/nonexistent/andy-ide-test").is_err());
}

#[test]
fn listing_something_that_is_not_a_directory_fails() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("file.rs"), "").unwrap();
    let p = Project::open(dir.path()).unwrap();
    let err = p
        .list_dir(std::path::Path::new("file.rs"), &opts())
        .unwrap_err();
    assert!(err.to_string().contains("not a directory"), "{err}");
}

#[test]
fn nested_directory_is_listed_through_its_relative_path() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/ui")).unwrap();
    fs::write(dir.path().join("src/ui/editor.rs"), "").unwrap();
    let p = Project::open(dir.path()).unwrap();
    let entries = p.list_dir(std::path::Path::new("src/ui"), &opts()).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].rel, std::path::PathBuf::from("src/ui/editor.rs"));
}
