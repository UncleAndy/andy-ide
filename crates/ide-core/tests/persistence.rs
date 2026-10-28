use ide_core::{Config, Recent, Session, TabState, Theme, fs::write_atomic};

#[test]
fn config_roundtrip_keeps_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = \"Light\"\nfuture_key = 42\n").unwrap();
    let mut cfg = Config::load_or_default(&path);
    assert!(matches!(cfg.theme, Theme::Light));
    cfg.font_size = 16.0;
    cfg.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("future_key"));
    assert!(text.contains("16"));
}

#[test]
fn missing_config_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::load_or_default(&dir.path().join("absent.toml"));
    assert_eq!(cfg, Config::default());
    assert!(matches!(cfg.theme, Theme::Dark));
    assert_eq!(cfg.tab_width, 4);
}

#[test]
fn broken_config_gives_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "theme = = broken\n").unwrap();
    let cfg = Config::load_or_default(&path);
    assert_eq!(cfg, Config::default());
}

#[test]
fn recent_is_capped_and_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recent.json");
    let mut r = Recent::load(&path);
    for i in 0..25 {
        r.add(std::path::PathBuf::from(format!("/p/{i}")));
    }
    r.add(std::path::PathBuf::from("/p/24"));
    assert_eq!(r.list().len(), 20);
    assert_eq!(r.list()[0], std::path::PathBuf::from("/p/24"));
}

#[test]
fn recent_survives_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("recent.json");
    let mut r = Recent::load(&path);
    r.add(std::path::PathBuf::from("/p/one"));
    r.save(&path).unwrap();
    let back = Recent::load(&path);
    assert_eq!(back.list(), r.list());
}

#[test]
fn session_roundtrip_and_atomic_write_replaces_content() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.json");
    let s = Session {
        tabs: vec![TabState {
            path: "/x.rs".into(),
            cursor_line: 3,
            cursor_col: 5,
        }],
        active: Some(0),
        layout: Some("dock".into()),
    };
    s.save(&path).unwrap();
    let back = Session::load(&path);
    assert_eq!(back.tabs.len(), 1);
    assert_eq!(back.tabs[0].cursor_col, 5);

    let f = dir.path().join("a.txt");
    write_atomic(&f, b"one").unwrap();
    write_atomic(&f, b"two").unwrap();
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "two");
    let leftovers: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn atomic_write_creates_missing_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/dir/file.txt");
    write_atomic(&path, b"content").unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "content");
}
