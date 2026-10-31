use ide_core::Tabs;
use std::path::PathBuf;

#[test]
fn opening_same_path_activates_existing_tab() {
    let mut t = Tabs::new();
    let a = t.open(PathBuf::from("/a.rs"), 1);
    t.open(PathBuf::from("/b.rs"), 2);
    let again = t.open(PathBuf::from("/a.rs"), 1);
    assert_eq!(a, again);
    assert_eq!(t.len(), 2);
    assert_eq!(t.active(), Some(again));
}

#[test]
fn closing_dirty_tab_requires_confirmation() {
    let mut t = Tabs::new();
    let i = t.open(PathBuf::from("/a.rs"), 1);
    t.dirty_flag(1, true);
    assert!(!t.close(i));
    assert_eq!(t.len(), 1);
    t.close_forced(i);
    assert_eq!(t.len(), 0);
    assert_eq!(t.active(), None);
}

#[test]
fn closing_active_tab_selects_neighbour() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 1);
    let b = t.open(PathBuf::from("/b.rs"), 2);
    t.set_active(b);
    t.close_forced(b);
    assert_eq!(t.active(), Some(0));
}

#[test]
fn closing_last_tab_falls_back_to_the_left() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 1);
    let b = t.open(PathBuf::from("/b.rs"), 2);
    t.set_active(b);
    t.close_forced(b);
    assert_eq!(t.len(), 1);
    assert_eq!(t.active(), Some(0));
    assert_eq!(t.items()[0].path, PathBuf::from("/a.rs"));
}

#[test]
fn dirty_flag_is_cleared_and_tab_becomes_closable() {
    let mut t = Tabs::new();
    let i = t.open(PathBuf::from("/a.rs"), 7);
    t.dirty_flag(7, true);
    assert!(t.dirty(i));
    t.dirty_flag(7, false);
    assert!(!t.dirty(i));
    assert!(t.close(i));
}

#[test]
fn set_active_ignores_out_of_range_index() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 1);
    t.set_active(99);
    assert_eq!(t.active(), Some(0));
}

#[test]
fn buffer_id_is_kept_per_tab() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 11);
    t.open(PathBuf::from("/b.rs"), 22);
    assert_eq!(t.items()[0].buffer_id, 11);
    assert_eq!(t.items()[1].buffer_id, 22);
}

#[test]
fn stepping_walks_the_strip_and_wraps_at_both_ends() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 1);
    t.open(PathBuf::from("/b.rs"), 2);
    t.open(PathBuf::from("/c.rs"), 3);
    t.set_active(0);

    t.step(true);
    assert_eq!(t.active(), Some(1));
    t.step(true);
    t.step(true);
    assert_eq!(t.active(), Some(0), "the step did not wrap forward");

    t.step(false);
    assert_eq!(t.active(), Some(2), "the step did not wrap back");
}

#[test]
fn stepping_with_one_tab_stays_on_it() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 1);
    t.step(true);
    t.step(false);
    assert_eq!(t.active(), Some(0));
}

#[test]
fn stepping_with_nothing_open_does_nothing() {
    let mut t = Tabs::new();
    t.step(true);
    t.step(false);
    assert_eq!(t.active(), None);
}

#[test]
fn stepping_with_nothing_in_front_takes_the_first_tab() {
    let mut t = Tabs::new();
    t.open(PathBuf::from("/a.rs"), 1);
    t.open(PathBuf::from("/b.rs"), 2);
    t.close_forced(t.active().expect("the tab that was opened last"));

    // Both are still open in the strip; the step has to land somewhere rather than do nothing.
    assert_eq!(t.len(), 1);
    t.step(true);
    assert_eq!(t.active(), Some(0));
}
