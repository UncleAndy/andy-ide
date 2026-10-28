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

/// The window is remembered as part of the same file as the zoom and the theme.
#[test]
fn the_maximized_window_survives_a_round_trip() {
    let mut cfg = Config::default();
    assert!(!cfg.window_maximized, "a new install opens in a window");

    cfg.window_maximized = true;
    let text = toml::to_string_pretty(&cfg).unwrap();
    let back: Config = toml::from_str(&text).unwrap();
    assert!(
        back.window_maximized,
        "the flag was lost on the way: {text}"
    );
}

#[test]
fn a_config_without_the_flag_is_not_maximized() {
    let cfg: Config = toml::from_str("font_size = 20.0\n").unwrap();
    assert!(!cfg.window_maximized);
}
