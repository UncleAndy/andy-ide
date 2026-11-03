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
        (!label.is_empty() && amount.chars().any(|c| c.is_ascii_digit()))
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
