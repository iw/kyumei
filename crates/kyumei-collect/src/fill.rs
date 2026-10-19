//! Filling a request code's placeholders with values given on the command line.
//!
//! A value with characters other than letters, digits and `_` is matched in
//! both spellings, as Kyumei's own queries do: as written, and as tally (the
//! server's default metrics library, and Go Workers using it) writes it, with
//! every other character turned into `_`.

use std::collections::BTreeMap;

use kyumei_samples::placeholders;

/// How tally writes a label value. Matches `kyumei_domain::tally_spelling`.
pub fn tally_spelling(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Fill every `<name>` in `query`. `Err` lists the names with no value.
pub fn fill(query: &str, values: &BTreeMap<String, String>) -> Result<String, Vec<String>> {
    let missing: Vec<String> = placeholders(query)
        .into_iter()
        .filter(|n| !values.contains_key(n))
        .collect();
    if !missing.is_empty() {
        return Err(missing);
    }
    let mut q = query.to_owned();
    for (name, value) in values {
        let placeholder = format!("<{name}>");
        if !q.contains(&placeholder) {
            continue;
        }
        let tally = tally_spelling(value);
        if tally != *value {
            let either = format!("{}|{tally}", string_escaped(&regex_escaped(value)));
            q = q
                .replace(&format!("!=\"{placeholder}\""), &format!("!~\"{either}\""))
                .replace(&format!("=\"{placeholder}\""), &format!("=~\"{either}\""))
                .replace(&format!("=~\"{placeholder}"), &format!("=~\"(?:{either})"))
                .replace(&format!("!~\"{placeholder}"), &format!("!~\"(?:{either})"));
        }
        q = fill_in_place(&q, &placeholder, value);
    }
    Ok(q)
}

/// Put `value` where `placeholder` stands, escaped for its place: inside a
/// regex matcher's string as a regex literal, inside any other string as a
/// string literal, and elsewhere as it is. Matches
/// `kyumei_diagnostics::select`'s filling of Kyumei's own placeholders.
fn fill_in_place(query: &str, placeholder: &str, value: &str) -> String {
    let mut out = String::with_capacity(query.len());
    let mut rest = query;
    // Inside a double-quoted string, and whether it is a regex matcher's.
    let mut string: Option<bool> = None;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with(placeholder) {
            out.push_str(&match string {
                Some(true) => string_escaped(&regex_escaped(value)),
                Some(false) => string_escaped(value),
                None => value.to_owned(),
            });
            rest = &rest[placeholder.len()..];
            continue;
        }
        let mut len = c.len_utf8();
        match (string, c) {
            // An escape sequence: keep it whole.
            (Some(_), '\\') => len += rest[len..].chars().next().map_or(0, char::len_utf8),
            (Some(_), '"') => string = None,
            (None, '"') => {
                let before = out.trim_end();
                string = Some(before.ends_with("=~") || before.ends_with("!~"));
            }
            _ => {}
        }
        out.push_str(&rest[..len]);
        rest = &rest[len..];
    }
    out
}

/// A value as a regex literal: RE2's metacharacters escaped, and nothing else,
/// since RE2 refuses an escaped letter it has no meaning for.
fn regex_escaped(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            "\\.+*?()|[]{}^$"
                .contains(c)
                .then_some('\\')
                .into_iter()
                .chain([c])
        })
        .collect()
}

/// A value inside a PromQL string literal.
fn string_escaped(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `NAME=VALUE` pairs from `--set`.
pub fn parse_settings(pairs: &[String]) -> Result<BTreeMap<String, String>, String> {
    pairs
        .iter()
        .map(|p| match p.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() && !v.is_empty() => {
                Ok((k.trim().to_owned(), v.to_owned()))
            }
            Some((k, _)) if !k.trim().is_empty() => Err(format!(
                "`--set {p}` has no value: put the {} after the `=`",
                k.trim()
            )),
            _ => Err(format!("`{p}` is not NAME=VALUE")),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn plain_values_are_filled_as_they_are() {
        let q = r#"sum(rate(x{namespace="<namespace>", task_queue="<task_queue>"}[1m]))"#;
        assert_eq!(
            fill(
                q,
                &values(&[("namespace", "orders"), ("task_queue", "fulfil_1")])
            ),
            Ok(r#"sum(rate(x{namespace="orders", task_queue="fulfil_1"}[1m]))"#.to_owned())
        );
    }

    #[test]
    fn values_that_tally_rewrites_match_both_spellings() {
        let q = r#"x{namespace="<namespace>", task_queue!="<tq>"}"#;
        assert_eq!(
            fill(q, &values(&[("namespace", "commerce-prod"), ("tq", "a.b")])),
            Ok(
                r#"x{namespace=~"commerce-prod|commerce_prod", task_queue!~"a\\.b|a_b"}"#
                    .to_owned()
            )
        );
    }

    #[test]
    fn values_never_end_the_string_they_stand_in() {
        assert_eq!(
            fill(r#"x{task_queue="<tq>"}"#, &values(&[("tq", r#"a"b\c"#)])),
            Ok(r#"x{task_queue=~"a\"b\\\\c|a_b_c"}"#.to_owned())
        );
        assert_eq!(
            fill(
                r#"x{a="pre-<tq>", b=~"<tq>.*"}[5m]"#,
                &values(&[("tq", "q.1")])
            ),
            Ok(r#"x{a="pre-q.1", b=~"(?:q\\.1|q_1).*"}[5m]"#.to_owned())
        );
        assert_eq!(
            fill(r#"x{namespace="<ns>"}"#, &values(&[("ns", "café-q")])),
            Ok(r#"x{namespace=~"café-q|caf__q"}"#.to_owned())
        );
    }

    #[test]
    fn missing_values_are_listed() {
        let q = r#"x{namespace="<namespace>", task_queue="<task_queue>"}"#;
        assert_eq!(
            fill(q, &values(&[("namespace", "orders")])),
            Err(vec!["task_queue".to_owned()])
        );
    }

    #[test]
    fn settings_parse() {
        assert_eq!(
            parse_settings(&["namespace=orders".to_owned(), "a=b=c".to_owned()]),
            Ok(values(&[("namespace", "orders"), ("a", "b=c")]))
        );
        assert!(parse_settings(&["novalue".to_owned()]).is_err());
        let empty = parse_settings(&["namespace=".to_owned()]).expect_err("empty");
        assert!(empty.contains("put the namespace after"), "{empty}");
    }
}
