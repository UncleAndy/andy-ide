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
        // Plain words, because the notification is read through a glance at a corner of the screen:
        // what the turn is over means is carried by the fact that it appeared at all.
        let title = match self {
            Want::Asks => "Ждём вашего ответа",
            Want::Finished => "Ответ готов",
        };
        let body = match self {
            Want::Asks => detail.to_owned(),
            Want::Finished => String::new(),
        };
        (title.to_owned(), body)
    }
}

/// Sends one notification from this very process, before anything else is drawn.
///
/// Kept for the question the probes from a terminal cannot answer: whether the desk is willing to show
/// a notification sent by *this* process. The probes appear, the window's own do not, and the two
/// differ in their sender — so the same call is made here, by the window, at a moment when nothing
/// else is happening. Run the window with `ANDY_IDE_NOTIFY_PROBE=1` and see whether it appears.
pub fn probe() {
    let sent = notify_rust::Notification::new()
        .summary("ПРОБА из процесса окна")
        .body("это уведомление отправлено самим andy-ide")
        .show()
        .map(|handle| handle.id());
    tracing::info!(
        sent = ?sent,
        bus = %std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
        "the window's own probe was sent"
    );
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
        // Exactly the call the probe makes — summary, body, nothing else. No name of our own and no
        // time asked for: the probe from a terminal is drawn by the desk, the window's own is not, and
        // every difference between the two is a suspect until the two are the same call.
        match notify_rust::Notification::new()
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
    fn each_want_says_what_it_is_about() {
        let (title, body) = Want::Asks.message("Run the tests?");
        assert_eq!(
            title, "Ждём вашего ответа",
            "a question does not say that it waits: {title}"
        );
        assert_eq!(
            body, "Run the tests?",
            "the question itself is not the body"
        );

        let (title, body) = Want::Finished.message("end_turn");
        assert_eq!(
            title, "Ответ готов",
            "a finished turn does not say so: {title}"
        );
        assert!(
            body.is_empty(),
            "the name of the event is not something to read: {body:?}"
        );
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
