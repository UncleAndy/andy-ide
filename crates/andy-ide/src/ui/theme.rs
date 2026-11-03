//! The colours of the window itself: what egui draws panels, fields and the editor on.
//!
//! egui's own light theme is white on white — `window_fill`, `panel_fill`, `faint_bg_color` and
//! `extreme_bg_color` all land within a few points of `#FFFFFF`, so a person cannot tell where a
//! panel ends and the editor begins and the window reads as one blank sheet. The light theme is laid
//! out here the way a light IDE is: the window is the darkest paper, the panels stand a step above
//! it, rows and fields take a tint of their own, and the editor is white — the brightest thing on
//! screen, because it is what is read. The dark theme stays egui's own.

use egui::Color32;
use ide_core::Theme;

/// The window behind the panels.
const WINDOW: Color32 = Color32::from_rgb(0xE3, 0xE6, 0xEB);
/// The panels themselves: the menu bar, the status bar, the tools.
const PANEL: Color32 = Color32::from_rgb(0xEF, 0xF1, 0xF4);
/// Rows that alternate, a code span, the stripe behind a list.
const FAINT: Color32 = Color32::from_rgb(0xE0, 0xE4, 0xEA);
/// Text fields, the editor, the terminal: the paper a person writes on.
const PAPER: Color32 = Color32::from_rgb(0xFF, 0xFF, 0xFF);
/// The lines between things.
const LINE: Color32 = Color32::from_rgb(0xC3, 0xC9, 0xD2);
/// The menu bar across the top: the band a person reaches for first, a step darker than a panel,
/// the way a window's own menu strip is.
const MENU: Color32 = Color32::from_rgb(0xE1, 0xE5, 0xEA);
/// The strip the tabs stand on. Darker than a tab, so tabs read as things set on it, and darker
/// than the page: the space beside the tabs used to be the brightest colour in the window.
const BAR: Color32 = Color32::from_rgb(0xDC, 0xE0, 0xE6);
/// A tab that is not in front, and the same tab under the pointer.
const TAB: Color32 = Color32::from_rgb(0xE9, 0xEC, 0xF1);
const TAB_HOVER: Color32 = Color32::from_rgb(0xF3, 0xF5, 0xF8);
/// What a person picked, what the pointer is on, and what is being pressed.
const SELECTION: Color32 = Color32::from_rgb(0xC7, 0xDB, 0xF4);
const HOVER: Color32 = Color32::from_rgb(0xDD, 0xE2, 0xE9);
const PRESSED: Color32 = Color32::from_rgb(0xCD, 0xD4, 0xDE);
const LINK: Color32 = Color32::from_rgb(0x1B, 0x5E, 0xA8);

/// The colours the window is drawn in.
pub fn visuals(theme: Theme) -> egui::Visuals {
    match theme {
        Theme::Dark => egui::Visuals::dark(),
        Theme::Light => {
            let mut visuals = egui::Visuals::light();
            visuals.window_fill = WINDOW;
            visuals.panel_fill = PANEL;
            visuals.faint_bg_color = FAINT;
            visuals.extreme_bg_color = PAPER;
            visuals.code_bg_color = FAINT;
            visuals.hyperlink_color = LINK;
            visuals.window_stroke.color = LINE;
            // A separator, a frame, the outline of a widget: one line, not a hairline of white.
            visuals.widgets.noninteractive.bg_stroke.color = LINE;
            visuals.widgets.noninteractive.bg_fill = PANEL;
            visuals.widgets.noninteractive.weak_bg_fill = PANEL;
            // A button rests a shade off the panel and comes forward under the pointer.
            visuals.widgets.inactive.weak_bg_fill = PANEL;
            visuals.widgets.inactive.bg_stroke.color = LINE;
            visuals.widgets.hovered.weak_bg_fill = HOVER;
            visuals.widgets.hovered.bg_stroke.color = LINE;
            visuals.widgets.active.weak_bg_fill = PRESSED;
            visuals.selection.bg_fill = SELECTION;
            visuals.selection.stroke.color = LINK;
            visuals
        }
    }
}

/// The fill of the menu bar.
///
/// The dark theme keeps egui's own: its panels already stand off the window.
pub fn menu_fill(theme: Theme) -> Color32 {
    match theme {
        Theme::Dark => egui::Visuals::dark().panel_fill,
        Theme::Light => MENU,
    }
}

/// The fill of the strip the tabs stand on.
pub fn tab_bar_fill(theme: Theme) -> Color32 {
    match theme {
        Theme::Dark => egui::Visuals::dark().extreme_bg_color,
        Theme::Light => BAR,
    }
}

/// The look of the dock: the tab bar, the tabs and the body under them.
///
/// egui_dock paints the tab bar in `extreme_bg_color`, which is the white of a text field, and the
/// body in the window fill; in the light theme that left the space beside the tabs as the brightest
/// thing on screen. The light theme gets the strip and the tabs from here, the dark one egui's own.
pub fn dock_style(style: &egui::Style, theme: Theme) -> egui_dock::Style {
    let mut dock = egui_dock::Style::from_egui(style);
    // The strip is the same grey in both themes; for the dark one that is egui's own colour.
    dock.tab_bar.bg_fill = tab_bar_fill(theme);
    if theme == Theme::Light {
        dock.tab_bar.hline_color = LINE;
        dock.tab.tab_body.bg_fill = PANEL;
        dock.tab.inactive.bg_fill = TAB;
        dock.tab.inactive_with_kb_focus.bg_fill = TAB;
        dock.tab.hovered.bg_fill = TAB_HOVER;
        dock.tab.active.bg_fill = PANEL;
        dock.tab.focused.bg_fill = PANEL;
        dock.tab.active_with_kb_focus.bg_fill = PANEL;
        dock.tab.focused_with_kb_focus.bg_fill = PANEL;
        for tab in [
            &mut dock.tab.inactive,
            &mut dock.tab.inactive_with_kb_focus,
            &mut dock.tab.hovered,
        ] {
            tab.outline_color = LINE;
            tab.text_color = Color32::from_rgb(0x3C, 0x42, 0x4A);
        }
        for tab in [
            &mut dock.tab.active,
            &mut dock.tab.focused,
            &mut dock.tab.active_with_kb_focus,
            &mut dock.tab.focused_with_kb_focus,
        ] {
            tab.outline_color = LINE;
            tab.text_color = Color32::from_rgb(0x17, 0x1A, 0x1E);
        }
    }
    dock
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How far apart two colours read, in the channel that differs most.
    fn apart(a: Color32, b: Color32) -> f32 {
        let gap = |x: u8, y: u8| (x as f32 - y as f32).abs();
        gap(a.r(), b.r())
            .max(gap(a.g(), b.g()))
            .max(gap(a.b(), b.b()))
    }

    /// WCAG contrast: the ratio an eye is measured by.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn channel(value: u8) -> f32 {
            let value = value as f32 / 255.0;
            if value <= 0.039_28 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        fn luminance(colour: Color32) -> f32 {
            0.2126 * channel(colour.r())
                + 0.7152 * channel(colour.g())
                + 0.0722 * channel(colour.b())
        }
        let (a, b) = (luminance(a), luminance(b));
        let (light, dark) = if a > b { (a, b) } else { (b, a) };
        (light + 0.05) / (dark + 0.05)
    }

    #[test]
    fn the_light_theme_is_not_paper_on_paper() {
        let visuals = visuals(Theme::Light);
        for (a, b, what) in [
            (
                visuals.window_fill,
                visuals.panel_fill,
                "the window and the panels",
            ),
            (
                visuals.panel_fill,
                visuals.extreme_bg_color,
                "the panels and the editor",
            ),
            (
                visuals.faint_bg_color,
                visuals.panel_fill,
                "the rows and the panels",
            ),
            (
                visuals.window_fill,
                visuals.extreme_bg_color,
                "the window and the editor",
            ),
        ] {
            assert!(
                apart(a, b) >= 6.0,
                "{what} are the same colour: {a:?} against {b:?}"
            );
        }
        assert!(
            contrast(visuals.extreme_bg_color, Color32::BLACK) > 15.0,
            "the editor is not paper: {:?}",
            visuals.extreme_bg_color
        );
    }

    #[test]
    fn the_light_theme_keeps_its_words_readable() {
        let visuals = visuals(Theme::Light);
        for (colour, what) in [
            (visuals.text_color(), "the text"),
            (visuals.weak_text_color(), "the weak text"),
            (visuals.hyperlink_color, "a link"),
            (visuals.selection.stroke.color, "the selection"),
        ] {
            for (background, where_is) in [
                (visuals.extreme_bg_color, "the editor"),
                (visuals.panel_fill, "a panel"),
            ] {
                assert!(
                    contrast(colour, background) >= 3.0,
                    "{what} on {where_is} reads {:.2}:1",
                    contrast(colour, background)
                );
            }
        }
    }

    #[test]
    fn the_tabs_stand_on_a_grey_strip() {
        // egui_dock paints the tab bar in the colour of a text field, so the space beside the tabs
        // was the brightest thing in a light window. The strip is its own grey, a tab that is not in
        // front is lighter than the strip, the tab in front is the colour of the body under it.
        let style = egui::Style::default();
        let dark = dock_style(&style, Theme::Dark);
        let stock = egui_dock::Style::from_egui(&style);
        assert_eq!(dark.tab_bar.bg_fill, stock.tab_bar.bg_fill);
        assert_eq!(dark.tab.inactive.bg_fill, stock.tab.inactive.bg_fill);

        let light = dock_style(&style, Theme::Light);
        let paper = visuals(Theme::Light).extreme_bg_color;
        assert_eq!(light.tab_bar.bg_fill, BAR);
        assert_ne!(
            light.tab_bar.bg_fill, paper,
            "the strip the tabs stand on is the page's white"
        );
        assert!(
            apart(light.tab.inactive.bg_fill, light.tab_bar.bg_fill) >= 6.0,
            "a tab is not told apart from the strip it stands on"
        );
        // The tab in front belongs to the body under it, not to the strip behind it.
        assert_eq!(light.tab.active.bg_fill, light.tab.tab_body.bg_fill);
        assert!(
            apart(light.tab.tab_body.bg_fill, paper) >= 10.0,
            "the body of a panel is the colour of the page: {light:?}"
        );
        // The words on every tab are read against the tab they stand on.
        for (tab, what) in [
            (&light.tab.inactive, "a tab that is not in front"),
            (&light.tab.active, "the tab in front"),
        ] {
            assert!(
                contrast(tab.text_color, tab.bg_fill) >= 4.5,
                "{what} reads {:.2}:1",
                contrast(tab.text_color, tab.bg_fill)
            );
        }
    }

    #[test]
    fn the_menu_strip_is_greyer_than_the_panels() {
        let light = visuals(Theme::Light);
        let menu = menu_fill(Theme::Light);
        assert_ne!(menu, light.panel_fill, "the menu band is a panel");
        assert_ne!(menu, light.extreme_bg_color, "the menu band is the page");
        assert!(
            apart(menu, light.panel_fill) >= 6.0,
            "the menu band does not stand off the panels"
        );
        // The words of the menu are read on the band, and the band is not the page's white.
        assert!(
            contrast(light.text_color(), menu) >= 4.5,
            "the menu reads {:.2}:1 on its band",
            contrast(light.text_color(), menu)
        );
        // The dark theme keeps egui's own band.
        assert_eq!(
            menu_fill(Theme::Dark),
            egui::Visuals::dark().panel_fill,
            "the dark theme's menu band was changed"
        );
    }

    #[test]
    fn the_dark_theme_stays_eguis_own() {
        let ours = visuals(Theme::Dark);
        let stock = egui::Visuals::dark();
        assert_eq!(ours.panel_fill, stock.panel_fill);
        assert_eq!(ours.extreme_bg_color, stock.extreme_bg_color);
        assert_eq!(ours.window_fill, stock.window_fill);
    }
}
