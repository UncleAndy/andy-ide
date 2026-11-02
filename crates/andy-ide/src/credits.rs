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
}
