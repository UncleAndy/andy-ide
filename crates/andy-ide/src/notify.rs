//! Telling the person, away from the desk, that the panel wants them.
//!
//! Two things are worth a notification: the agent is waiting for an answer, and a turn is over. Both
//! are the moments a person who has walked away from the window wants to hear about, and both are
//! already said inside it — so the notification is for the case where the window is not the thing
//! being looked at. Nothing is asked first: a want that arrives is told, every time.
//!
//! Who tells is the thread the agent speaks on (`agent::work`), not the window: egui draws on demand,
//! and a window that is not on screen — another workspace, a person away from the desk — is given no
//! frames at all, so a notice left to the panel would wait for the person to come back to the window.
//! The panel is told what the desk answered (`Reply::Told`) and says it in the status line.
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

/// Where the person is, as far as the window can tell.
///
/// A notification is for someone who is not looking at the window, so this is what decides whether the
/// desk is told at all. It is asked by the thread of the agent — the thread that tells — and the window
/// fills it in on every frame it draws.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Looking {
    /// The window has the keyboard.
    pub focused: bool,
    /// The agent's panel is the tab on screen, in a leaf that is not collapsed.
    pub panel_in_front: bool,
}

/// Whether a notice is worth sending to a person who is where `looking` says.
///
/// Not sent while they are looking at the panel itself: the question is already on the screen in front of
/// them, and a notice about something being read is noise. Everything else is told — including a window
/// that has the keyboard with the panel behind another tab, where the agent waits out of sight, and a
/// window nobody is at. The default (`Looking::default`) is the last of those on purpose: a window that
/// has not drawn a frame yet cannot claim the person is looking at it.
pub fn should_tell(looking: Looking) -> bool {
    !(looking.focused && looking.panel_in_front)
}

/// Sends one notification from this very process, before anything else is drawn.
///
/// Kept for the question the probes from a terminal cannot answer: whether the desk is willing to show
/// a notification sent by *this* process. The probes appear, the window's own do not, and the two
/// differ in their sender — so the same call is made here, by the window, at a moment when nothing
/// else is happening. Run the window with `ANDY_IDE_NOTIFY_PROBE=1` and see whether it appears.
pub fn probe() {
    // Two, differing in one thing: the urgency, which is the only lever a desk honours for how
    // long a notice stands. Sent from this process before it has a window, so the answer is about
    // the urgency and not about the sender.
    let show = |label: &str, urgent: bool| {
        let mut note = notify_rust::Notification::new();
        note.summary(label).body(label);
        if urgent {
            note.urgency(notify_rust::Urgency::Critical);
        }
        let sent = note.show().map(|handle| handle.id());
        tracing::info!(label, urgent, sent = ?sent, "the probe was sent");
        std::thread::sleep(std::time::Duration::from_millis(3_000));
    };
    // Eight seconds first, so the person can start the window and walk to another workspace: the
    // question is what a notice does when it arrives where the window is not.
    std::thread::sleep(std::time::Duration::from_secs(8));
    show("ПРОБА 1 обычная", false);
    show("ПРОБА 2 срочная", true);
    tracing::info!(
        bus = %std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
        "the window probe is done"
    );
}

/// Somewhere a notification can be shown.
pub trait Surface: Send + Sync {
    /// Show one, or say why it could not be shown.
    ///
    /// The reason comes back rather than only going to the log, because a notification that was
    /// never shown leaves no other trace: the person is by definition looking elsewhere, so the
    /// window has to be able to say what the desk did when they come back to it.
    /// Returns the id the desk gave the notification, or why it could not be shown.
    fn show(&self, title: &str, body: &str) -> Result<u32, String>;
}

/// How long a notification stands, and how long its connection is kept open for it.
///
/// A minute, because the person these are for is away from the desk. The desk draws a banner for a
/// few seconds of its own choosing and GNOME ignores any time asked of it, hiding a banner four
/// seconds after it appears (`messageTray.js::NOTIFICATION_TIMEOUT`) — there the only way to be read
/// is to be **critical**, which stands until it is put away; on a desk that honours the time asked,
/// the minute is the minute. It is also how long the handle is held: the desk destroys a notification
/// whose sender it can no longer see, and the sender is this process.
pub const STANDING: std::time::Duration = std::time::Duration::from_secs(60);

/// The connections the notifications were sent on, and when they were sent.
///
/// `notify-rust` opens a connection per call and closes it as soon as the handle is dropped. The desk
/// resolves a sender that owns a window to an application, and an application's source is destroyed the
/// moment its sender leaves the bus — taking the notification with it, banner and all. That is why a
/// probe from a terminal appears (there is no window to resolve, so the source is a plain one that
/// survives) while the window's own does not, and why the very same window appears when it has no
/// window yet: the handle has to be kept, not thrown away with the id.
static HELD: std::sync::Mutex<Vec<(std::time::Instant, notify_rust::NotificationHandle)>> =
    std::sync::Mutex::new(Vec::new());

/// Whether a handle sent at `sent` is still worth holding at `now`.
fn worth_holding(sent: std::time::Instant, now: std::time::Instant) -> bool {
    now.duration_since(sent) < STANDING
}

/// Keeps the handle alive for as long as the notice stands, and lets go of the ones that are over.
fn hold(handle: notify_rust::NotificationHandle, now: std::time::Instant) {
    let Ok(mut held) = HELD.lock() else {
        return;
    };
    held.push((now, handle));
    held.retain(|(sent, _)| worth_holding(*sent, now));
}

/// The desk's own notifications, through the service the session provides.
#[derive(Default)]
pub struct System;

impl Surface for System {
    fn show(&self, title: &str, body: &str) -> Result<u32, String> {
        // No name of our own — the desk takes the name of the sender, which is what the probes that are
        // drawn carry. Urgent, and standing a minute: here the call leaves the probe behind on
        // purpose, because a notice nobody can read in the four seconds the desk gives it is not a
        // notice, and urgency is the only lever the protocol has that a desk honours. It also
        // passes a desk setting of do not disturb, which is the right way round for the one thing the
        // person is being told: the agent is waiting for them.
        match notify_rust::Notification::new()
            .urgency(notify_rust::Urgency::Critical)
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
                let id = handle.id();
                tracing::info!(
                    id,
                    bus = %std::env::var("DBUS_SESSION_BUS_ADDRESS").unwrap_or_default(),
                    "the desk took the notification"
                );
                hold(handle, std::time::Instant::now());
                Ok(id)
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
    fn a_notice_is_for_someone_who_is_not_looking() {
        // The expected answers are written out, not computed the way the code computes them.
        let cases = [
            (
                Looking {
                    focused: true,
                    panel_in_front: true,
                },
                false,
                "the person is looking at the panel",
            ),
            (
                Looking {
                    focused: true,
                    panel_in_front: false,
                },
                true,
                "the panel is behind another tab in a window that has the keyboard",
            ),
            (
                Looking {
                    focused: false,
                    panel_in_front: true,
                },
                true,
                "the window shows the panel and nobody is at it",
            ),
            (
                Looking {
                    focused: false,
                    panel_in_front: false,
                },
                true,
                "nobody is at the window",
            ),
        ];
        for (looking, expected, why) in cases {
            assert_eq!(should_tell(looking), expected, "{why}: {looking:?}");
        }

        // A window that has drawn no frame cannot claim the person is looking at it, so a notice that
        // arrives before the first frame is still sent.
        assert!(
            should_tell(Looking::default()),
            "the default held a notice back"
        );
    }

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
    fn a_connection_is_held_for_as_long_as_the_notice_stands() {
        // A handle thrown away with its id is a sender gone from the bus, and the desk destroys the
        // notification of a sender it cannot see. Held for the standing time, the notice lives.
        let sent = std::time::Instant::now();
        assert!(
            worth_holding(sent, sent),
            "the handle was let go the moment it was taken"
        );
        assert!(
            worth_holding(sent, sent + STANDING / 2),
            "the handle was let go while the notice still stood"
        );
        assert!(
            !worth_holding(sent, sent + STANDING + std::time::Duration::from_secs(1)),
            "the handle was kept long after the notice was over"
        );
        assert!(
            STANDING >= std::time::Duration::from_secs(60),
            "a notice stands {STANDING:?}, and a person away from the desk is not back in a moment"
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
