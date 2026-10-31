//! Telling the person, away from the desk, that the panel wants them.
//!
//! Two things are worth a notification: the agent is waiting for an answer, and a turn is over. Both
//! are the moments a person who has walked away from the window wants to hear about, and both are
//! already said inside it — so the notification is for the case where the window is not the thing
//! being looked at, and nothing is shown while the panel is in front of them in a window that has
//! the keyboard.
//!
//! The system's own notifications carry them (`notify-rust`: the desktop's D-Bus service on Linux,
//! `NSUserNotification` on macOS, a WinRT toast on Windows). A machine that cannot show one — no
//! notification service, a permission that was refused — is written to the log and goes on: a
//! notification is a courtesy, never a reason for the window to stop.

/// What the panel wants the person for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Want {
    /// The agent is waiting for an answer: a permission or a choice.
    Asks,
    /// The turn is over.
    Finished,
}

impl Want {
    /// What the notification says, with the detail the panel has for it.
    pub fn message(self, detail: &str) -> (String, String) {
        let title = match self {
            Want::Asks => "andy-ide: the agent is waiting for you",
            Want::Finished => "andy-ide: the agent has finished",
        };
        (title.to_owned(), detail.to_owned())
    }
}

/// Whether a want is worth showing at all.
///
/// The setting is the person's; the rest is the circumstances. A panel in front of them, in a window
/// that has the keyboard, is already saying it — a notification then is the same news twice.
pub fn wanted(setting: bool, focused: bool, panel_in_front: bool) -> bool {
    setting && !(focused && panel_in_front)
}

/// Somewhere a notification can be shown.
pub trait Surface: Send {
    /// Show one, or say why it could not be shown.
    ///
    /// The reason comes back rather than only going to the log, because a notification that was
    /// never shown leaves no other trace: the person is by definition looking elsewhere, so the
    /// window has to be able to say what the desk did when they come back to it.
    /// Returns the id the desk gave the notification, or why it could not be shown.
    fn show(&self, title: &str, body: &str) -> Result<u32, String>;
}

/// The desk's own notifications, through the service the session provides.
#[derive(Default)]
pub struct System;

impl Surface for System {
    fn show(&self, title: &str, body: &str) -> Result<u32, String> {
        // The name is what the desk attributes the notification to, and a notification it cannot
        // attribute is one it may drop without a word — which is a failure that looks exactly like
        // success from here, because the call itself returns an id.
        match notify_rust::Notification::new()
            .appname("andy-ide")
            .summary(title)
            .body(body)
            .show()
        {
            Ok(handle) => {
                // Which bus took it, and under which id: an id means a daemon answered, and a
                // notification that answered but never appeared means the daemon is not the one
                // drawing the desk. That difference is invisible without the address.
                // At info, not debug: when a notification is taken and never seen, this line is the
                // only way to learn which bus answered — and the default filter would hide it.
                tracing::info!(
                    id = handle.id(),
                    bus = %std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
                    "the desk took the notification"
                );
                Ok(handle.id())
            }
            Err(error) => {
                tracing::warn!(%error, "the notification could not be shown");
                Err(error.to_string())
            }
        }
    }
}

#[cfg(test)]
mod try_the_desk {
    /// A real notification, shown on the desktop this check runs under. Not part of the suite: it
    /// needs a session bus and a person looking at the screen. Run it to learn what the desk does.
    #[test]
    #[ignore = "needs the desktop"]
    fn a_real_notification_reaches_the_desk() {
        use super::Surface;
        let shown = notify_rust::Notification::new()
            .summary("andy-ide: check of the desk")
            .body("if you can read this, the system notifications work")
            .show();
        println!("RESULT: {shown:?}");
        println!("BUS: {:?}", std::env::var("DBUS_SESSION_BUS_ADDRESS"));
        println!("RUNTIME: {:?}", std::env::var("XDG_RUNTIME_DIR"));
        println!(
            "SURFACE: {:?}",
            super::System.show("andy-ide: through the surface", "the window's own path")
        );
    }
}

/// A desk that refuses every notification, the way a session with no bus in reach does.
#[cfg(test)]
pub struct Refused;

#[cfg(test)]
impl Surface for Refused {
    fn show(&self, _title: &str, _body: &str) -> Result<u32, String> {
        Err("no session bus in reach".to_owned())
    }
}

/// A surface that writes them down instead: what the checks use to ask what the window said, and
/// what a build with no desk under it can be given.
#[cfg(test)]
#[derive(Default)]
pub struct Written {
    shown: std::sync::Mutex<Vec<(String, String)>>,
}

#[cfg(test)]
impl Written {
    /// Everything shown, in the order it was shown.
    pub fn shown(&self) -> Vec<(String, String)> {
        self.shown
            .lock()
            .expect("the lock over what was shown is not poisoned")
            .clone()
    }
}

#[cfg(test)]
impl Surface for Written {
    fn show(&self, title: &str, body: &str) -> Result<u32, String> {
        self.shown
            .lock()
            .expect("the lock over what was shown is not poisoned")
            .push((title.to_owned(), body.to_owned()));
        Ok(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panel_in_front_of_a_person_is_its_own_notification() {
        assert!(
            !wanted(true, true, true),
            "the panel was in front and the window had the keyboard: the news is already there"
        );
    }

    #[test]
    fn a_person_looking_elsewhere_is_told() {
        assert!(
            wanted(true, false, true),
            "the window does not have the keyboard: this is the case the notifications are for"
        );
        assert!(
            wanted(true, true, false),
            "the window is in front but the panel is not: the agent is waiting out of sight"
        );
    }

    #[test]
    fn the_setting_turns_the_notifications_off() {
        assert!(!wanted(false, false, false));
        assert!(!wanted(false, true, false));
        assert!(!wanted(false, false, true));
    }

    #[test]
    fn each_want_says_what_it_is_about() {
        let (title, body) = Want::Asks.message("Run the tests?");
        assert!(
            title.contains("waiting"),
            "a question does not say that it waits: {title}"
        );
        assert_eq!(
            body, "Run the tests?",
            "the question itself is not the body"
        );

        let (title, body) = Want::Finished.message("ready");
        assert!(
            title.contains("finished"),
            "a finished turn does not say so: {title}"
        );
        assert_eq!(body, "ready");
    }

    #[test]
    fn a_machine_with_no_notification_service_is_not_a_reason_to_stop() {
        // There is no session bus under the checks, and a machine may have none either: the surface
        // says so in the log and the window goes on. What is asked here is that it does not panic —
        // and that it does not sit there waiting for a service that is not coming.
        let (title, body) = Want::Finished.message("ready");
        let started = std::time::Instant::now();
        let _ = System.show(&title, &body);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "the surface waited {:?} for a service that is not there",
            started.elapsed()
        );
    }

    #[test]
    fn a_written_surface_keeps_what_it_was_asked_to_show() {
        let surface = Written::default();
        let _ = surface.show("a title", "a body");
        let _ = surface.show("another", "and its body");
        assert_eq!(
            surface.shown(),
            vec![
                ("a title".to_owned(), "a body".to_owned()),
                ("another".to_owned(), "and its body".to_owned()),
            ]
        );
    }
}
