pub fn to_plain_text(snippet: &str) -> String {
    let chars: Vec<char> = snippet.chars().collect();
    let mut out = String::with_capacity(snippet.len());
    let mut at = 0;
    copy_until(&chars, &mut at, None, &mut out);
    out
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn copy_until(chars: &[char], at: &mut usize, stop: Option<char>, out: &mut String) -> bool {
    while let Some(&c) = chars.get(*at) {
        if Some(c) == stop {
            *at += 1;
            return true;
        }
        match c {
            '\\' if matches!(chars.get(*at + 1), Some('$' | '}' | '\\')) => {
                out.push(chars[*at + 1]);
                *at += 2;
            }
            '$' => expand_dollar(chars, at, out),
            _ => {
                out.push(c);
                *at += 1;
            }
        }
    }
    false
}

fn expand_dollar(chars: &[char], at: &mut usize, out: &mut String) {
    let start = *at;
    *at += 1;
    if chars.get(*at) == Some(&'{') {
        *at += 1;
        let before = out.len();
        if !expand_braces(chars, at, out) {
            out.truncate(before);
            out.push('$');
            *at = start + 1;
        }
        return;
    }
    let name = take_name(chars, at);
    if name.is_empty() {
        out.push('$');
    } else if !name.starts_with(|c: char| c.is_ascii_digit()) {
        out.push_str(&name);
    }
}

fn expand_braces(chars: &[char], at: &mut usize, out: &mut String) -> bool {
    let name = take_name(chars, at);
    if name.is_empty() {
        return false;
    }
    match chars.get(*at) {
        Some('}') => {
            *at += 1;
            if !name.starts_with(|c: char| c.is_ascii_digit()) {
                out.push_str(&name);
            }
            true
        }
        Some(':') => {
            *at += 1;
            copy_until(chars, at, Some('}'), out)
        }
        Some('|') => {
            *at += 1;
            first_choice(chars, at, out)
        }
        Some('/') => skip_past_brace(chars, at),
        _ => false,
    }
}

fn first_choice(chars: &[char], at: &mut usize, out: &mut String) -> bool {
    let mut in_first = true;
    while let Some(&c) = chars.get(*at) {
        *at += 1;
        match c {
            '|' if chars.get(*at) == Some(&'}') => {
                *at += 1;
                return true;
            }
            '\\' => {
                if let Some(&next) = chars.get(*at) {
                    *at += 1;
                    if in_first {
                        out.push(next);
                    }
                }
            }
            ',' => in_first = false,
            _ if in_first => out.push(c),
            _ => {}
        }
    }
    false
}

fn skip_past_brace(chars: &[char], at: &mut usize) -> bool {
    while let Some(&c) = chars.get(*at) {
        *at += if c == '\\' { 2 } else { 1 };
        if c == '}' {
            return true;
        }
    }
    false
}

fn take_name(chars: &[char], at: &mut usize) -> String {
    let start = *at;
    while chars.get(*at).copied().is_some_and(is_name_char) {
        *at += 1;
    }
    chars[start..*at].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::to_plain_text;

    #[test]
    fn placeholders_become_their_text_and_stops_disappear() {
        let cases = [
            ("foo($0)", "foo()"),
            ("foo($1, $2)$0", "foo(, )"),
            ("foo(${1})", "foo()"),
            ("foo(${1:bar}, ${2:baz})", "foo(bar, baz)"),
            ("${1:outer ${2:inner} end}", "outer inner end"),
            ("${1|one,two,three|}", "one"),
            ("${1|a\\,b,c|}", "a,b"),
            ("if ${1:x} {\n\t$0\n}", "if x {\n\t\n}"),
            ("plain", "plain"),
            ("", ""),
        ];
        for (snippet, plain) in cases {
            assert_eq!(to_plain_text(snippet), plain, "{snippet}");
        }
    }

    #[test]
    fn escapes_leave_the_character_php_needs_for_its_variables() {
        assert_eq!(to_plain_text("\\$foo"), "$foo");
        assert_eq!(to_plain_text("a\\}b\\\\c"), "a}b\\c");
        assert_eq!(to_plain_text("\\n"), "\\n", "not an escape in a snippet");
    }

    #[test]
    fn variables_give_their_default_or_their_name() {
        assert_eq!(to_plain_text("$TM_FILENAME"), "TM_FILENAME");
        assert_eq!(to_plain_text("${TM_FILENAME}"), "TM_FILENAME");
        assert_eq!(to_plain_text("${TM_FILENAME:none}"), "none");
        assert_eq!(to_plain_text("a${1/x/y/g}b"), "ab");
    }

    #[test]
    fn what_never_was_snippet_syntax_stays_and_nothing_panics() {
        for text in [
            "$", "${", "${1", "${1:", "${1:a", "${1|", "${1|a", "${!}", "cost: 5$", "}",
        ] {
            let plain = to_plain_text(text);
            assert!(plain.contains('$') || plain == "}", "{text} -> {plain}");
        }
        assert_eq!(to_plain_text("${1:a"), "${1:a");
        assert_eq!(to_plain_text("cost: 5$"), "cost: 5$");
        assert_eq!(to_plain_text("ы${1:ж}𝄞"), "ыж𝄞");
    }
}
