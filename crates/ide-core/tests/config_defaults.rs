use ide_core::{Config, Theme};

#[test]
fn absent_keys_fall_back_to_defaults_and_unknown_keys_are_kept() {
    let cfg: Config = toml::from_str("theme = \"Light\"\nfuture_key = 42\n").unwrap();
    assert!(matches!(cfg.theme, Theme::Light));
    assert_eq!(cfg.font_size, 14.0);
    assert_eq!(cfg.tab_width, 4);
    assert!(!cfg.show_hidden);
    assert!(cfg.extra.contains_key("future_key"));
    assert_eq!(cfg.extra["future_key"].as_integer(), Some(42));
}

#[test]
fn empty_file_is_default_config() {
    let cfg: Config = toml::from_str("").unwrap();
    assert_eq!(cfg, Config::default());
}
