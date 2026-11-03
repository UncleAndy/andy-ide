//! When the caret stands inside a call, and which parameter is being written.
//!
//! The rule is pure: the text and the caret's byte go in, and either nothing or the call comes
//! back. The server is asked only when there is a call; which parameter is *active* is the
//! server's own answer, and this count is only what decides that the question is worth asking.
//!
//! The count is of separators of the call's own level: a comma inside an array literal or a
//! string is not one, which is why both are tracked. A comma inside a comment would be counted
//! — the same level of care as the rest of the editor, no more.

/// The call the caret stands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inside {
    /// The byte of the `(` that opened the call.
    pub open: usize,
    /// The parameter being written: 0 for the first, 1 after the first comma, and so on.
    pub argument: usize,
}

/// Where the caret stands, if it stands inside the parentheses of a call.
///
/// `None` covers the three ways there can be no call: the brackets of the nearest unclosed
/// pair are not `(` (an array, a block, a plain group), or there is no unclosed bracket at all.
pub fn inside_a_call(text: &str, caret: usize) -> Option<Inside> {
    let caret = caret.min(text.len());
    // Every unclosed bracket with the byte it opened at and the commas seen at its level.
    let mut open: Vec<(char, usize, usize)> = Vec::new();
    let mut literal: Option<char> = None;
    let mut escaped = false;

    for (byte, character) in text[..caret].char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if let Some(opening) = literal {
            match character {
                '\\' => escaped = true,
                one if one == opening => literal = None,
                _ => {}
            }
            continue;
        }
        match character {
            '"' | '\'' => literal = Some(character),
            '(' | '[' | '{' => open.push((character, byte, 0)),
            ')' | ']' | '}' => {
                open.pop();
            }
            ',' => {
                // Only a comma of the call's own level separates its parameters.
                if let Some(last) = open.last_mut()
                    && last.0 == '('
                {
                    last.2 += 1;
                }
            }
            _ => {}
        }
    }

    match open.last() {
        Some(&('(', open_byte, argument)) => Some(Inside {
            open: open_byte,
            argument,
        }),
        _ => None,
    }
}

/// What the editor draws for one answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    /// The whole signature of the call, as the server wrote it.
    pub label: String,
    /// Each parameter's own text, and whether it is the one being written.
    pub parameters: Vec<(String, bool)>,
}

/// Turns an answer into what the editor draws.
///
/// Only the active signature is drawn: a person writing one call needs the one being matched, and
/// choosing among overloads is a different thing the editor does not offer yet.
pub fn view_of(help: &lsp_types::SignatureHelp) -> Option<View> {
    let active = help.active_signature.unwrap_or(0) as usize;
    let signature = help
        .signatures
        .get(active)
        .or_else(|| help.signatures.first())?;
    let active_parameter = signature
        .active_parameter
        .or(help.active_parameter)
        .map(|one| one as usize);
    let parameters = signature
        .parameters
        .as_ref()
        .map(|list| {
            list.iter()
                .enumerate()
                .map(|(index, one)| {
                    (
                        label_of(one, &signature.label),
                        Some(index) == active_parameter,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Some(View {
        label: signature.label.clone(),
        parameters,
    })
}

/// The text of a parameter's label.
///
/// The protocol allows it to be either the text itself or a range *into* the signature's label —
/// and that range counts UTF-16 units, which is not what Rust counts, so the label is walked
/// rather than sliced. rust-analyzer sends the range form, so without this the parameters would
/// come out empty.
fn label_of(parameter: &lsp_types::ParameterInformation, signature: &str) -> String {
    match &parameter.label {
        lsp_types::ParameterLabel::Simple(text) => text.clone(),
        lsp_types::ParameterLabel::LabelOffsets([start, end]) => {
            let mut units = 0u32;
            let mut from = None;
            let mut to = signature.len();
            for (byte, letter) in signature.char_indices() {
                if units == *start && from.is_none() {
                    from = Some(byte);
                }
                if units == *end {
                    to = byte;
                    break;
                }
                units += letter.len_utf16() as u32;
            }
            from.map(|from| signature[from..to.min(signature.len())].to_owned())
                .unwrap_or_default()
        }
    }
}

/// The call a question is about: which file and line, which bracket, which parameter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub path: std::path::PathBuf,
    pub line: u32,
    pub open: usize,
    pub argument: usize,
}

/// Everything the window knows about the parameters of the call being written.
///
/// Two rules live here, and they are the two that can go visibly wrong:
///
/// - the question is worth asking again only when the call is *another* one. Typing inside the
///   same parameter of the same call leaves the key unchanged, so the server is not asked on
///   every keystroke;
/// - an answer is shown only for the file and line it was asked about, and not for a call the
///   writer has already waved away with Esc. A late answer about another place is thrown away —
///   the same rule the completion list and the highlight hint keep.
#[derive(Default)]
pub struct Help {
    /// The question that went to the server last.
    asked: Option<Asked>,
    /// The answer that came back, with the file and line it was asked about.
    answer: Option<(std::path::PathBuf, u32, lsp_types::SignatureHelp)>,
    /// The call whose popup Esc closed: it stays closed until the call changes.
    waved_away: Option<Asked>,
}

impl Help {
    pub fn new() -> Self {
        Self::default()
    }

    /// The call the caret stands in now, if it stands in one.
    pub fn now(
        &self,
        path: &std::path::Path,
        line: usize,
        text: &str,
        caret: usize,
    ) -> Option<Asked> {
        let inside = inside_a_call(text, caret)?;
        Some(Asked {
            path: path.to_path_buf(),
            line: line as u32,
            open: inside.open,
            argument: inside.argument,
        })
    }

    /// Whether this call is a question not yet sent.
    pub fn due(&self, now: &Asked) -> bool {
        self.asked.as_ref() != Some(now)
    }

    /// Remembers that the question went out, so it is not repeated while the call is the same.
    pub fn asked(&mut self, now: Asked) {
        self.asked = Some(now);
    }

    /// Takes an answer. `None` means the server knows of no call there, which also clears whatever
    /// was shown before.
    ///
    /// An empty answer also takes back the "asked" mark: a server that has just been told about a
    /// newly written call may not know it yet, and a question asked once and never again would leave
    /// the popup missing for good. How often the question may be repeated is the window's business —
    /// it keeps a pause between them.
    pub fn answer(
        &mut self,
        path: std::path::PathBuf,
        line: u32,
        help: Option<lsp_types::SignatureHelp>,
    ) {
        if help.is_none() {
            self.asked = None;
        }
        self.answer = help.map(|help| (path, line, help));
    }

    /// One line for the `ANDY_IDE_SIGNATURE_PROBE` run: what the rule sees, and what is known about
    /// the answer. The same shape as the completion probe, and for the same reason — a popup that
    /// never appears leaves nothing to look at.
    pub fn probe(&self, path: &std::path::Path, line: usize, text: &str, caret: usize) -> String {
        let now = self.now(path, line, text, caret);
        let what = match &now {
            Some(call) => format!("вызов: скобка {} параметр {}", call.open, call.argument),
            None => "вызова нет".to_owned(),
        };
        format!(
            "строка {} каретка {} | {} | ответ {} | спрошено {}",
            line,
            caret,
            what,
            self.answer.is_some(),
            self.asked.is_some()
        )
    }

    /// The answer to draw for the call the caret stands in now, if there is one.
    pub fn showing(&self, now: &Option<Asked>) -> Option<&lsp_types::SignatureHelp> {
        let (path, line, help) = self.answer.as_ref()?;
        let now = now.as_ref()?;
        if &now.path != path || now.line != *line {
            return None;
        }
        if self.waved_away.as_ref() == Some(now) {
            return None;
        }
        Some(help)
    }

    /// Esc: the popup goes and stays gone while the caret stays in this call.
    pub fn wave_away(&mut self, now: Option<Asked>) {
        if now.is_some() {
            self.waved_away = now;
        }
    }

    /// Nothing to ask and nothing to show: no file, or no server.
    pub fn forget(&mut self) {
        self.asked = None;
        self.answer = None;
        self.waved_away = None;
    }
}

#[cfg(test)]
mod signature_tests {
    use super::*;

    /// The caret at the end of `text`.
    fn at_end(text: &str) -> Option<Inside> {
        inside_a_call(text, text.len())
    }

    #[test]
    fn a_caret_inside_a_call_names_the_call_and_the_parameter() {
        let inside = at_end("project.unwrap(").expect("the caret is inside a call");
        assert_eq!(
            inside.open,
            "project.unwrap".len(),
            "the call opens at its bracket"
        );
        assert_eq!(
            inside.argument, 0,
            "nothing is written yet, so it is the first parameter"
        );

        let inside = at_end("push(first, second").expect("still inside the call");
        assert_eq!(
            inside.argument, 1,
            "one comma is behind, so it is the second parameter"
        );

        let inside = at_end("push(first,").expect("still inside the call");
        assert_eq!(
            inside.argument, 1,
            "a comma just typed moves to the next parameter"
        );

        let inside = at_end("push(").expect("inside");
        assert_eq!(inside.argument, 0, "an empty call is the first parameter");
    }

    #[test]
    fn the_nested_brackets_of_the_arguments_do_not_count_as_separators() {
        // Об этом и весь модуль: запятая внутри литерала массива или строки — не разделитель.
        let inside = at_end("push([1, 2], треть").expect("inside");
        assert_eq!(
            inside.argument, 1,
            "the comma inside the array is not a separator"
        );

        let inside = at_end("push(\"a, b\"").expect("inside");
        assert_eq!(
            inside.argument, 0,
            "the comma inside the string is not a separator"
        );
    }

    #[test]
    fn a_caret_outside_any_call_asks_nothing() {
        assert_eq!(
            at_end("let a = project.unwrap()"),
            None,
            "the call is closed"
        );
        assert_eq!(
            at_end("array[0"),
            None,
            "the nearest unclosed bracket is an array"
        );
        assert_eq!(at_end("if flag { a } else { "), None, "…and here a block");
        assert_eq!(at_end("let a = (1 + 2)"), None, "a plain group is closed");
    }

    #[test]
    fn cyrillic_is_counted_by_characters_not_bytes() {
        let inside = at_end("вызов(первый, втор").expect("inside");
        assert_eq!(
            inside.argument, 1,
            "the comma is behind, the cyrillic is not bytes"
        );
        assert_eq!(
            inside.open,
            "вызов".len(),
            "the bracket is at its byte, not its character"
        );
    }

    fn a_call(path: &str, line: usize, text: &str, caret: usize) -> Option<Asked> {
        Help::default().now(std::path::Path::new(path), line, text, caret)
    }

    #[test]
    fn the_same_call_is_not_asked_twice_but_typing_a_comma_makes_a_new_question() {
        let mut help = Help::default();
        let first = a_call("a.rs", 3, "push(val", 8).expect("inside");
        assert!(
            help.due(&first),
            "a call never asked about is due a question"
        );
        help.asked(first.clone());
        assert!(
            !help.due(&first),
            "typing inside the same parameter is not a new question"
        );

        let after_comma = a_call("a.rs", 3, "push(val,", 9).expect("inside");
        assert!(
            help.due(&after_comma),
            "the next parameter is a new question"
        );
    }

    #[test]
    fn an_answer_is_shown_for_its_own_file_and_line_only() {
        let mut help = Help::default();
        help.answer(std::path::PathBuf::from("a.rs"), 3, Some(some_answer()));
        let here = a_call("a.rs", 3, "push(val", 8);
        assert!(
            help.showing(&here).is_some(),
            "the answer belongs to this line"
        );

        let elsewhere = a_call("a.rs", 9, "push(val", 8);
        assert!(
            help.showing(&elsewhere).is_none(),
            "…and not to another line"
        );

        let another_file = a_call("b.rs", 3, "push(val", 8);
        assert!(
            help.showing(&another_file).is_none(),
            "…nor to another file"
        );
        assert!(
            help.showing(&None).is_none(),
            "…and not when there is no call at all"
        );
    }

    #[test]
    fn esc_closes_the_popup_until_the_call_changes() {
        let mut help = Help::default();
        let call = a_call("a.rs", 3, "push(val", 8).expect("inside");
        help.answer(std::path::PathBuf::from("a.rs"), 3, Some(some_answer()));
        assert!(
            help.showing(&Some(call.clone())).is_some(),
            "shown before Esc"
        );

        help.wave_away(Some(call.clone()));
        assert!(help.showing(&Some(call)).is_none(), "closed by Esc");

        let next = a_call("a.rs", 3, "push(val,", 9).expect("inside");
        assert!(
            help.showing(&Some(next)).is_some(),
            "the next parameter brings it back"
        );
    }

    /// An answer with nothing in it: for the checks that only ask whether it is drawn at all.
    fn some_answer() -> lsp_types::SignatureHelp {
        lsp_types::SignatureHelp {
            signatures: Vec::new(),
            active_signature: None,
            active_parameter: None,
        }
    }

    fn an_answer(label: &str, ranges: Option<[u32; 2]>) -> lsp_types::SignatureHelp {
        lsp_types::SignatureHelp {
            signatures: vec![lsp_types::SignatureInformation {
                label: label.to_owned(),
                documentation: None,
                parameters: Some(vec![lsp_types::ParameterInformation {
                    label: match ranges {
                        Some(offsets) => lsp_types::ParameterLabel::LabelOffsets(offsets),
                        None => lsp_types::ParameterLabel::Simple("текст".to_owned()),
                    },
                    documentation: None,
                }]),
                active_parameter: None,
            }],
            active_signature: Some(0),
            active_parameter: Some(0),
        }
    }

    #[test]
    fn a_parameter_given_as_a_range_is_cut_out_of_the_signature() {
        // rust-analyzer sends the range form, and the range counts UTF-16 units: for the cyrillic
        // in this label that is still one unit per letter, but the start is counted from the whole
        // prefix, so a wrong count would cut the text in the middle.
        let help = an_answer("fn f(имя: u32)", Some([5, 13]));
        let view = view_of(&help).expect("a view comes out");
        assert_eq!(view.label, "fn f(имя: u32)");
        assert_eq!(view.parameters.len(), 1, "one parameter is drawn");
        assert_eq!(
            view.parameters[0].0, "имя: u32",
            "cut out of the signature by the range"
        );
        assert!(view.parameters[0].1, "it is the one being written");
    }

    #[test]
    fn a_parameter_given_as_text_is_taken_as_it_is() {
        let help = an_answer("fn f(a: u32)", None);
        let view = view_of(&help).expect("a view comes out");
        assert_eq!(
            view.parameters[0].0, "текст",
            "the text form goes through untouched"
        );
    }

    #[test]
    fn an_answer_with_no_signature_draws_nothing() {
        let mut help = an_answer("fn f(a: u32)", None);
        help.signatures.clear();
        assert_eq!(view_of(&help), None, "nothing to draw");
    }

    #[test]
    fn an_empty_answer_leaves_the_call_worth_asking_about_again() {
        let mut help = Help::default();
        let call = a_call("a.rs", 3, "push(val", 8).expect("inside");
        help.asked(call.clone());
        assert!(!help.due(&call), "asked once, so not due again");

        help.answer(std::path::PathBuf::from("a.rs"), 3, None);
        assert!(
            help.due(&call),
            "an empty answer takes the mark back: the server may not know the call yet"
        );
        assert!(
            help.showing(&Some(call)).is_none(),
            "and there is nothing to show"
        );
    }

    #[test]
    fn an_escaped_quote_does_not_end_the_string() {
        let inside = at_end("push(\"a \\\", b\"").expect("inside");
        assert_eq!(
            inside.argument, 0,
            "the escaped quote is part of the string"
        );
    }
}
