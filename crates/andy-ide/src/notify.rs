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
/// The setting is the person's and nothing else is: a notification is shown when the agent asks or
/// finishes, wherever the window happens to be and whatever stands in front of it. Asking whether the
/// panel is already saying it would be deciding for the person what they can see.
pub fn wanted(setting: bool) -> bool {
    setting
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

/// How long a notification stands, when the desk is not asked for a time of its own.
///
/// The desk's own default is a few seconds, chosen for a person already looking at the screen. These
/// notifications exist for the opposite case — someone in another window, or away from the machine —
/// and a notice nobody can read is not a notice. Ten seconds is long enough to notice and short enough
/// not to stand in the way.
pub const STANDING: std::time::Duration = std::time::Duration::from_secs(10);

/// The desk's own notifications, through the service the session provides.
#[derive(Default)]
pub struct System;

impl Surface for System {
    fn show(&self, title: &str, body: &str) -> Result<u32, String> {
        // No name is given: the desk takes the sender's own, which is what a call from a terminal
        // gets and what the desk has been seen to show. Naming the application was a guess at why a
        // notification was not drawn, and it was a guess that cost a day — the probe that appeared and
        // the window that did not differed in this and in nothing else, and the probe was the one
        // without it.
        match notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .timeout(notify_rust::Timeout::Milliseconds(
                STANDING.as_millis() as u32
            ))
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
    /// Four notifications that differ only in their words, and say which is which.
    ///
    /// The probe carrying the window's own words was taken with an id and never shown, while
    /// plainer ones appeared. Each here carries its own application name, so the desk cannot take
    /// one for a replacement of another; each stands twenty seconds and comes four seconds after
    /// the last, long enough for a person to write down what arrived, in order, without guessing.
    #[test]
    #[ignore = "needs the desktop"]
    fn a_real_notification_reaches_the_desk() {
        let show = |index: u32, summary: &str, body: &str| {
            println!(
                "{index}: {:?}",
                notify_rust::Notification::new()
                    .appname(&format!("andy-ide-probe-{index}"))
                    .summary(summary)
                    .body(body)
                    .timeout(notify_rust::Timeout::Milliseconds(20_000))
                    .show()
                    .map(|handle| handle.id())
            );
            std::thread::sleep(std::time::Duration::from_millis(4_000));
        };
        show(1, "ПРОБА 1: простой заголовок", "простой текст");
        show(
            2,
            "andy-ide: the agent is waiting for you",
            "ПРОБА 2: заголовок окна, простой текст",
        );
        show(
            3,
            "ПРОБА 3: простой заголовок",
            "Запрос выбора для проверки. Появился ли тост?",
        );
        show(
            4,
            "andy-ide: the agent is waiting for you",
            "Запрос выбора для проверки. Появился ли тост?",
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
    fn a_notice_stands_long_enough_to_be_read() {
        // The desk's own default is meant for somebody already at the screen. These are for somebody
        // who is not, and a notice that is gone before they look is the same as no notice at all.
        assert!(
            STANDING >= std::time::Duration::from_secs(8),
            "a notification stands {STANDING:?}, which is not long enough to be noticed"
        );
    }

    #[test]
    fn the_setting_is_the_only_thing_that_keeps_a_notice_back() {
        assert!(
            !wanted(false),
            "the setting was off and the notice was shown"
        );
        assert!(
            wanted(true),
            "the setting was on and the notice was kept back"
        );
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
