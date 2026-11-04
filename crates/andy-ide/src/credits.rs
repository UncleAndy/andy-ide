//! What an agent says is left on its account, turned into the one figure the status line shows.
//!
//! The agent sends its provider's own words — the very lines `/usage` prints — and this picks the
//! balance out of them. The choice is a preference and not a parse of unknown text: a top-up first,
//! then anything that names what is left, and otherwise the first line carrying a figure at all.
//! Nothing is thrown away: the whole block is shown on hover, so a provider whose form nobody has
//! written code for yet is still read correctly by the person looking at it.

/// The labels the panel prefers, most wanted first.
const WANTED: [&str; 6] = [
    "top-up",
    "top up",
    "balance",
    "remaining",
    "usable",
    "credits",
];

/// The balance in a provider's lines: its label, and what it says. `None` when no line carries a
/// figure — a provider with nothing to report says nothing, and nothing is not zero.
pub fn balance(lines: &[String]) -> Option<(String, String)> {
    let split = |line: &str| -> Option<(String, String)> {
        let (label, amount) = line.split_once(':')?;
        let (label, amount) = (label.trim(), amount.trim());
        // A window of a limit is not money: `Weekly: 99% remaining (1% used)` is what is left of a
        // plan, and the row shows it as a limit rather than as a balance.
        (!label.is_empty()
            && amount.chars().any(|c| c.is_ascii_digit())
            && percent_left(amount).is_none())
        .then(|| (label.to_owned(), amount.to_owned()))
    };
    for wanted in WANTED {
        let found = lines.iter().find_map(|line| {
            split(line).filter(|(label, _)| label.to_lowercase().contains(wanted))
        });
        if found.is_some() {
            return found;
        }
    }
    lines.iter().find_map(|line| split(line))
}

/// A limit a provider reports: the window it belongs to, and how much of it is still there.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limit {
    /// The window in the provider own words: `Rolling window`, `Weekly`, `Monthly`.
    pub label: String,
    /// What is left of it, in percent.
    pub left: u8,
}

impl Limit {
    /// Whether this is the window of the day: the one a provider calls rolling.
    fn is_rolling(&self) -> bool {
        self.label.to_ascii_lowercase().starts_with("rolling")
    }

    /// The window in one letter: `Rolling window` is `R`, `Weekly` is `W`.
    pub fn initial(&self) -> char {
        self.label
            .chars()
            .next()
            .map(|letter| letter.to_ascii_uppercase())
            .unwrap_or('?')
    }
}

/// The limits in a block of account lines: `Weekly: 99% remaining (1% used) • resets in 5d 7h`.
///
/// A subscription reports no balance and does report windows, and each window is a labelled line like
/// any other fact. The share is read from the words the provider itself uses — `N% remaining`, or
/// `N% used` turned around — so a window that prints only what was spent still lands right. A line
/// whose figure is money (`Top-up credits: $14.06`) is not a limit and is left to `balance`.
pub fn limits(lines: &[String]) -> Vec<Limit> {
    lines
        .iter()
        .filter_map(|line| {
            let (label, value) = line.split_once(':')?;
            let (label, value) = (label.trim(), value.trim());
            let left = percent_left(value)?;
            (!label.is_empty()).then(|| Limit {
                label: label.to_owned(),
                left,
            })
        })
        .collect()
}

/// The windows as the row shows them: `Limits: 96% (w:99% m:98%)`.
///
/// The figure in front is the rolling window, without a letter: it answers "can I work now", and it
/// is the one a person wants without reading brackets — so it keeps the front place even when a
/// longer window is tighter, and the week and the month follow in the provider order, each with its
/// letter. A subscription that reports no rolling window is led by the window that runs out first,
/// because then the tightest is what the front place can say at all. One window alone needs no
/// parentheses.
pub fn limits_figure(limits: &[Limit]) -> Option<String> {
    let front = limits.iter().position(Limit::is_rolling).or_else(|| {
        limits
            .iter()
            .enumerate()
            .min_by_key(|(_, limit)| limit.left)
            .map(|(index, _)| index)
    })?;
    let mut others: Vec<String> = Vec::new();
    for (index, limit) in limits.iter().enumerate() {
        if index != front {
            others.push(format!(
                "{}:{}%",
                limit.initial().to_ascii_lowercase(),
                limit.left
            ));
        }
    }
    let mut figure = format!("Limits: {}%", limits[front].left);
    if !others.is_empty() {
        figure.push_str(&format!(" ({})", others.join(" ")));
    }
    Some(figure)
}

/// How much of a window is left, read from the words of the provider: `99% remaining`, `42% left`,
/// or `57% used` turned around. `None` when the line says nothing about a share of a window.
fn percent_left(value: &str) -> Option<u8> {
    let lowered = value.to_lowercase();
    let mut rest = lowered.as_str();
    while let Some(at) = rest.find('%') {
        let head = &rest[..at];
        let number = head
            .rsplit(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or("");
        let tail = &rest[at + 1..];
        if let Ok(percent) = number.parse::<u8>() {
            let percent = percent.min(100);
            let after: String = tail.trim_start().chars().take(9).collect();
            if after.starts_with("remaining") || after.starts_with("left") {
                return Some(percent);
            }
            if after.starts_with("used") || after.starts_with("spent") {
                return Some(100 - percent);
            }
        }
        rest = tail;
    }
    None
}

/// The provider a model id belongs to: `nous:anthropic/claude` and `opencode-go:deepseek` both carry
/// it before the first separator, and an id with no separator names nobody in particular.
pub fn provider_of(model: &str) -> Option<&str> {
    let key = model.split([':', '/']).next().unwrap_or_default().trim();
    (key.len() < model.len()).then_some(key)
}

/// The provider a block of the account's lines names in itself — `Provider: nous` — if it names one.
///
/// The label is the block's own and not a guess at prose: a provider that says nothing about itself
/// leaves this empty, and then the agent's own word for the block is all there is to go on.
pub fn named_provider(lines: &[String]) -> Option<&str> {
    lines.iter().find_map(|line| {
        let (label, value) = line.split_once(':')?;
        (label.trim().eq_ignore_ascii_case("provider") && !value.trim().is_empty())
            .then(|| value.trim())
    })
}

/// Whether a block of account lines is the account of `provider`.
///
/// A block that names a provider of its own is that provider's, whatever the agent called it beside
/// the lines; a block that names none is the one the agent said it was. Nothing said at all is taken
/// as this provider's: an agent that keeps no such record still shows what it sent.
pub fn belongs(lines: &[String], said: Option<&str>, provider: &str) -> bool {
    if let Some(named) = named_provider(lines) {
        return same_provider(named, provider);
    }
    said.map(|said| same_provider(said, provider))
        .unwrap_or(true)
}

/// Whether two names are the same provider: the key in front of the model id and the key the account
/// block prints are the same word, and a display name in front of it (`Nous Portal`) still starts with
/// it. Case is not part of a name.
fn same_provider(one: &str, other: &str) -> bool {
    fn key(name: &str) -> String {
        name.split_whitespace()
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|c| c.is_alphanumeric())
            .collect::<String>()
            .to_lowercase()
    }
    let (one, other) = (key(one), key(other));
    !one.is_empty()
        && !other.is_empty()
        && (one == other || one.starts_with(&other) || other.starts_with(&one))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(of: &[&str]) -> Vec<String> {
        of.iter().map(|line| (*line).to_owned()).collect()
    }

    #[test]
    fn the_top_up_balance_is_the_figure_that_line_shows() {
        // The lines Nous sends for Andy's account, as they arrive over ACP.
        let nous = lines(&[
            "📈 Nous credits",
            "Provider: nous",
            "Subscription credits: $0.00",
            "Top-up credits: $14.06",
            "Total usable: $14.06",
        ]);
        assert_eq!(
            balance(&nous),
            Some(("Top-up credits".to_owned(), "$14.06".to_owned()))
        );
    }

    #[test]
    fn a_provider_that_says_it_another_way_is_still_read() {
        // The point of taking lines rather than fields: a provider nobody has written code for yet
        // puts its balance in its own words, and the panel shows what it said.
        assert_eq!(
            balance(&lines(&[
                "Plan: plus",
                "Balance: 12.5 EUR",
                "Spent this month: 3.10 EUR"
            ])),
            Some(("Balance".to_owned(), "12.5 EUR".to_owned()))
        );
        assert_eq!(
            balance(&lines(&["Weekly window", "Left: 42%"])),
            Some(("Left".to_owned(), "42%".to_owned()))
        );
    }

    #[test]
    fn what_is_left_beats_a_named_purse_that_is_empty() {
        // Without a top-up line, "Total usable" is the account's figure; a subscription purse that
        // happens to be empty is not it.
        assert_eq!(
            balance(&lines(&[
                "Subscription credits: $0.00",
                "Total usable: $14.06"
            ])),
            Some(("Total usable".to_owned(), "$14.06".to_owned()))
        );
    }

    #[test]
    fn nothing_to_report_is_not_a_balance_of_zero() {
        assert_eq!(balance(&[]), None);
        assert_eq!(balance(&lines(&["Provider: nous", "Nous credits"])), None);
    }

    #[test]
    fn a_window_of_a_limit_is_a_limit_and_not_a_balance() {
        // What OpenCode Go answers on /zen/go/v1/usage, as the agent renders it for the status line.
        let go = lines(&[
            "\u{1f4c8} Account limits",
            "Provider: opencode-go",
            "Rolling window: 99% remaining (1% used) • resets in 3h 10m (2026-11-03 19:40 UTC)",
            "Weekly: 99% remaining (1% used) • resets in 5d 7h (2026-11-09 00:00 UTC)",
            "Monthly: 100% remaining (0% used) • resets in 28d 20h (2026-12-02 12:31 UTC)",
        ]);
        assert_eq!(balance(&go), None, "a window was read as money");
        assert_eq!(
            limits(&go),
            vec![
                Limit {
                    label: "Rolling window".to_owned(),
                    left: 99
                },
                Limit {
                    label: "Weekly".to_owned(),
                    left: 99
                },
                Limit {
                    label: "Monthly".to_owned(),
                    left: 100
                },
            ]
        );
        assert_eq!(
            limits_figure(&limits(&go)),
            Some("Limits: 99% (w:99% m:100%)".to_owned())
        );
    }

    #[test]
    fn a_window_that_says_what_was_spent_is_turned_around() {
        assert_eq!(percent_left("57% used"), Some(43));
        assert_eq!(percent_left("57% spent of 100"), Some(43));
        assert_eq!(percent_left("43% left"), Some(43));
        assert_eq!(percent_left("unavailable"), None);
        // Money is not a share of a window.
        assert_eq!(percent_left("$14.06"), None);
    }

    #[test]
    fn every_window_is_in_the_row_in_the_order_the_provider_sent_them() {
        let block = lines(&[
            "Weekly: 40% remaining",
            "Monthly: 5% remaining",
            "Rolling window: 80% remaining",
        ]);
        // The rolling window in front, unlabelled - it is the one that answers what is left today -
        // and the others in the provider order with their letters, the tightest among them included:
        // a week that runs out first is still written down, in the brackets.
        assert_eq!(
            limits_figure(&limits(&block)),
            Some("Limits: 80% (w:40% m:5%)".to_owned())
        );
        // A subscription with no rolling window is led by the one that runs out first.
        assert_eq!(
            limits_figure(&limits(&lines(&[
                "Weekly: 40% remaining",
                "Monthly: 5% remaining",
            ]))),
            Some("Limits: 5% (w:40%)".to_owned())
        );
        // A window on its own needs no parentheses.
        assert_eq!(
            limits_figure(&limits(&lines(&["Monthly: 40% remaining"]))),
            Some("Limits: 40%".to_owned())
        );
        assert_eq!(limits_figure(&[]), None);
    }

    #[test]
    fn a_model_id_names_its_provider_before_the_first_separator() {
        assert_eq!(provider_of("nous:anthropic/claude-opus-5"), Some("nous"));
        assert_eq!(
            provider_of("opencode-go:deepseek-v4.1-flash"),
            Some("opencode-go")
        );
        assert_eq!(provider_of("nous/hermes-4"), Some("nous"));
        // An id that is only a name belongs to nobody in particular.
        assert_eq!(provider_of("hermes-4"), None);
    }

    #[test]
    fn a_block_of_lines_names_its_own_provider() {
        // What Hermes sends beside the money: the account says whose it is, in its own line.
        let nous = lines(&[
            "\u{1f4c8} Nous credits",
            "Provider: nous",
            "Top-up credits: $3.56",
        ]);
        assert_eq!(named_provider(&nous), Some("nous"));
        assert_eq!(named_provider(&lines(&["Top-up credits: $3.56"])), None);
    }

    #[test]
    fn a_block_that_names_another_provider_is_not_this_models_account() {
        let nous = lines(&["Provider: nous", "Top-up credits: $3.56"]);
        // The agent named the block after the model in front, and the block itself says otherwise:
        // the lines win, because they are the provider's own words about itself.
        assert!(!belongs(&nous, Some("opencode-go"), "opencode-go"));
        assert!(belongs(&nous, Some("opencode-go"), "nous"));
        // A block with nothing to say about itself is the one the agent said it was.
        let quiet = lines(&["Top-up credits: $3.56"]);
        assert!(belongs(&quiet, Some("opencode-go"), "opencode-go"));
        assert!(!belongs(&quiet, Some("nous"), "opencode-go"));
        // And an agent that says nothing either way still shows what it sent.
        assert!(belongs(&quiet, None, "opencode-go"));
    }

    #[test]
    fn a_display_name_in_front_of_the_key_is_the_same_provider() {
        assert!(belongs(&lines(&["Provider: Nous Portal"]), None, "nous"));
        assert!(belongs(&lines(&["Provider: nous"]), None, "Nous Portal"));
        assert!(belongs(&[], Some("OpenCode Go"), "opencode-go"));
        assert!(!belongs(&[], Some("nous"), "opencode-go"));
    }
}
