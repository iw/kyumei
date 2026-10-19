//! Headings of Markdown documents, and the anchors GitHub gives them, so that a
//! reference to a section can be linked and checked.

use std::{collections::BTreeMap, sync::LazyLock};

use regex::Regex;

/// `[text](url)`, whose rendered text is `text`.
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap_or_else(|e| panic!("{e}")));

/// One heading of a document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub level: usize,
    /// As written, without the `#` marks.
    pub text: String,
    /// The anchor GitHub gives it, numbered when an earlier heading has the same one.
    pub anchor: String,
}

/// The anchor GitHub makes from a heading: lower case, with everything but letters,
/// digits, spaces, `-` and `_` removed, and spaces turned into `-`.
pub fn heading_anchor(text: &str) -> String {
    plain(text)
        .to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// A heading's text as it reads: links reduced to their text, code and emphasis marks
/// removed.
pub fn plain(text: &str) -> String {
    LINK.replace_all(text, "$1")
        .chars()
        .filter(|c| !matches!(c, '`' | '*'))
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Every ATX heading (`#` to `######`) outside fenced code blocks, in order.
pub fn headings(document: &str) -> Vec<Heading> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for line in document.lines() {
        let trimmed = line.trim_start();
        let indent = line.len() - trimmed.len();
        if indent <= 3 {
            for marker in ["```", "~~~"] {
                if trimmed.starts_with(marker) {
                    fence = match fence {
                        Some(open) if open == marker => None,
                        None => Some(marker),
                        other => other,
                    };
                }
            }
        }
        if fence.is_some() || indent > 3 {
            continue;
        }
        let level = trimmed.chars().take_while(|c| *c == '#').count();
        if !(1..=6).contains(&level) {
            continue;
        }
        let rest = &trimmed[level..];
        if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
            continue;
        }
        let text = rest.trim().trim_end_matches('#').trim().to_owned();
        let base = heading_anchor(&text);
        let n = seen.entry(base.clone()).or_insert(0);
        let anchor = if *n == 0 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        *n += 1;
        out.push(Heading {
            level,
            text,
            anchor,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_made_as_github_makes_them() {
        assert_eq!(heading_anchor("Why it matters"), "why-it-matters");
        assert_eq!(
            heading_anchor("Detecting and Fixing an Existing `Write > Read` Misconfiguration"),
            "detecting-and-fixing-an-existing-write--read-misconfiguration"
        );
        assert_eq!(
            heading_anchor("Step 1 — Confirm a hot shard exists"),
            "step-1--confirm-a-hot-shard-exists"
        );
        assert_eq!(
            heading_anchor("4.2 How to detect it"),
            "42-how-to-detect-it"
        );
        assert_eq!(
            heading_anchor("[Alert 14](./x.md) — slots"),
            "alert-14--slots"
        );
        assert_eq!(
            heading_anchor("Raising `history.shardIOConcurrency`"),
            "raising-historyshardioconcurrency"
        );
    }

    #[test]
    fn headings_skip_code_and_number_repeats() {
        let doc = "# Title\n\n## Triage\n\n```sh\n# not a heading\n```\n\n## Triage\n#nospace\n    # indented code\n### Notes ##\n";
        let found: Vec<(usize, String, String)> = headings(doc)
            .into_iter()
            .map(|h| (h.level, h.text, h.anchor))
            .collect();
        let expected: Vec<(usize, String, String)> = [
            (1, "Title", "title"),
            (2, "Triage", "triage"),
            (2, "Triage", "triage-1"),
            (3, "Notes", "notes"),
        ]
        .iter()
        .map(|(l, t, a)| (*l, (*t).to_owned(), (*a).to_owned()))
        .collect();
        assert_eq!(found, expected);
    }
}
