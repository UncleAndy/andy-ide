//! A command line, as a person types one, turned into a program and its arguments.
//!
//! The panel lets the command be typed, because there is no list of ACP agents that is
//! ever complete: `hermes acp`, `npx @zed-industries/claude-code-acp`, a path to a
//! binary built an hour ago. What a person types is a command line, so it is read the
//! way a shell reads it — quotes group, a backslash escapes, runs of spaces separate —
//! and then handed to a program *without* a shell in between: `;`, `|` and `$HOME` are
//! filename characters here, not operators. Running the agent through a shell would make
//! every line typed into this field a program of the person's choosing, and an editor
//! that evaluates what is typed into it is an editor with a remote hole in it.

use anyhow::{Result, bail};

use crate::session::AgentSpec;

/// The name an agent gets when only a command was given: the program's own file name.
pub fn default_name(command: &str) -> String {
    std::path::Path::new(command)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .filter(|stem| !stem.is_empty())
        .unwrap_or_else(|| command.to_owned())
}

/// Reads a command line into an agent.
///
/// Both kinds of quote group their contents, `"` allows an escaped `\"`, and a backslash
/// outside quotes takes the next character literally — which is how a file name with a
/// space in it can be typed without quotes.
pub fn parse(line: &str) -> Result<AgentSpec> {
    parse_named("", line, None)
}

/// Reads a command line into an agent with the name and the terminal the person gave.
///
/// The name is not a detail: it is what the menu shows, and two agents started from the
/// same binary with different arguments — `hermes acp --fast` and `hermes acp --deep` —
/// are two entries as far as a person is concerned. Without one, the program's own name
/// is used, which is a guess and only a guess.
///
/// The terminal is the command line the agent's own shell commands are run through: the
/// work may happen in a container or over `ssh`, and an agent that assumes it is on the
/// machine the editor runs on is an agent that will edit the wrong files.
pub fn parse_named(name: &str, line: &str, terminal: Option<&str>) -> Result<AgentSpec> {
    let mut words: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut have_word = false;
    let mut quote: Option<char> = None;
    let mut characters = line.chars();

    while let Some(character) = characters.next() {
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), other) => word.push(other),
            (Some('"'), '\\') => match characters.next() {
                Some(escaped) => word.push(escaped),
                None => bail!("the command ends in a backslash inside quotes"),
            },
            (Some(_), other) => word.push(other),
            (None, '\'') | (None, '"') => {
                quote = Some(character);
                have_word = true;
            }
            (None, '\\') => match characters.next() {
                Some(escaped) => {
                    word.push(escaped);
                    have_word = true;
                }
                None => bail!("the command ends in a backslash"),
            },
            (None, whitespace) if whitespace.is_whitespace() => {
                if have_word {
                    words.push(std::mem::take(&mut word));
                    have_word = false;
                }
            }
            (None, other) => {
                word.push(other);
                have_word = true;
            }
        }
    }
    if quote.is_some() {
        bail!("a quote in the command was never closed");
    }
    if have_word {
        words.push(word);
    }

    let mut words = words.into_iter();
    let Some(command) = words.next() else {
        bail!("there is no command to run");
    };
    let name = if name.trim().is_empty() {
        default_name(&command)
    } else {
        name.trim().to_owned()
    };
    Ok(AgentSpec {
        name,
        command,
        args: words.collect(),
        env: Default::default(),
        terminal: terminal
            .map(str::trim)
            .filter(|terminal| !terminal.is_empty())
            .map(str::to_owned),
    })
}

/// A command line that would start this agent again, quoted so it reads back the same.
pub fn quote(spec: &AgentSpec) -> String {
    let mut parts = vec![quote_word(&spec.command)];
    parts.extend(spec.args.iter().map(|arg| quote_word(arg)));
    parts.join(" ")
}

/// One word, quoted only if it needs to be: a path with a space in it, or an empty
/// argument, which must not disappear.
pub(crate) fn quote_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|character| !character.is_whitespace() && !"\"'\\".contains(character));
    if plain {
        word.to_owned()
    } else {
        format!("\"{}\"", word.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_and_its_arguments_are_separated_by_spaces() {
        let spec = parse("hermes acp --stdio").expect("the command");
        assert_eq!(spec.command, "hermes");
        assert_eq!(spec.args, vec!["acp".to_owned(), "--stdio".to_owned()]);
        assert_eq!(spec.name, "hermes");
    }

    #[test]
    fn quotes_group_a_word_that_has_spaces_in_it() {
        let spec = parse(r#""/opt/my agent/bin/agent" --stdio"#).expect("the command");
        assert_eq!(spec.command, "/opt/my agent/bin/agent");
        assert_eq!(spec.args, vec!["--stdio".to_owned()]);
    }

    #[test]
    fn single_quotes_take_everything_literally() {
        let spec = parse(r#"agent 'a\b "c"' plain"#).expect("the command");
        assert_eq!(spec.args, vec![r#"a\b "c""#.to_owned(), "plain".to_owned()]);
    }

    #[test]
    fn a_backslash_outside_quotes_escapes_the_next_character() {
        let spec = parse(r#"agent my\ file.txt"#).expect("the command");
        assert_eq!(spec.args, vec!["my file.txt".to_owned()]);
    }

    #[test]
    fn a_shell_metacharacter_is_part_of_a_file_name_and_nothing_more() {
        // No shell is involved: `; rm -rf` is three words of an argument, not a second
        // command, and `$HOME` is not expanded.
        let spec = parse("agent 'a; rm -rf $HOME'").expect("the command");
        assert_eq!(spec.args, vec!["a; rm -rf $HOME".to_owned()]);
    }

    #[test]
    fn an_empty_line_is_refused_in_words() {
        let error = parse("   ").expect_err("an empty command was accepted");
        assert!(
            format!("{error}").contains("no command"),
            "the refusal is not in words: {error}"
        );
    }

    #[test]
    fn an_unclosed_quote_is_refused() {
        assert!(parse(r#"agent "unfinished"#).is_err());
        assert!(parse("agent 'unfinished").is_err());
    }

    #[test]
    fn a_command_written_out_and_read_back_is_the_same_command() {
        let spec = AgentSpec {
            name: "odd".to_owned(),
            command: "/opt/my agent/agent".to_owned(),
            args: vec![
                "a b".to_owned(),
                String::new(),
                r#"quote"inside"#.to_owned(),
            ],
            env: Default::default(),
            terminal: Some("ssh dev@build".to_owned()),
        };
        let line = quote(&spec);
        let back = parse_named(&spec.name, &line, spec.terminal.as_deref())
            .expect("the command read back");
        assert_eq!(back.command, spec.command);
        assert_eq!(back.name, "odd", "the name is the one that was given");
        assert_eq!(back.terminal.as_deref(), Some("ssh dev@build"));
        assert_eq!(back.args, spec.args, "through {line:?}");
    }
}
