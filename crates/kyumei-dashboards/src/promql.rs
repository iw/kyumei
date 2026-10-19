//! Enough of PromQL to say what a panel's query reads: the series it selects,
//! with their label matchers, the labels it groups by, and the functions it
//! applies. Template variables (`$namespace`, `[$__rate_interval]`) are left
//! as they are.

/// One series selector: `metric{label="value", …}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selector {
    /// The metric name, written before the braces or as `__name__="…"`.
    pub metric: Option<String>,
    pub matchers: Vec<Matcher>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Matcher {
    pub label: String,
    pub op: MatchOp,
    pub value: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOp {
    Equal,
    NotEqual,
    Regex,
    NotRegex,
}

/// What a query reads and how it shapes it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub selectors: Vec<Selector>,
    /// Labels in `by (…)` clauses, which the result keeps.
    pub grouped_by: Vec<String>,
    /// Labels in `without (…)` clauses, which the result drops.
    pub grouped_without: Vec<String>,
    /// Functions and aggregations, in the order they appear.
    pub functions: Vec<String>,
}

impl Query {
    /// The metric names the query selects.
    pub fn metrics(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .selectors
            .iter()
            .filter_map(|s| s.metric.as_deref())
            .collect();
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Every label the query matches on or groups by.
    pub fn labels(&self) -> Vec<&str> {
        let mut labels: Vec<&str> = self
            .selectors
            .iter()
            .flat_map(|s| s.matchers.iter().map(|m| m.label.as_str()))
            .chain(self.grouped_by.iter().map(String::as_str))
            .chain(self.grouped_without.iter().map(String::as_str))
            .filter(|l| *l != "__name__")
            .collect();
        labels.sort_unstable();
        labels.dedup();
        labels
    }

    pub fn uses(&self, function: &str) -> bool {
        self.functions.iter().any(|f| f == function)
    }
}

/// Aggregations, which may put `by (…)` or `without (…)` before their argument.
const AGGREGATIONS: &[&str] = &[
    "sum",
    "min",
    "max",
    "avg",
    "group",
    "stddev",
    "stdvar",
    "count",
    "count_values",
    "bottomk",
    "topk",
    "quantile",
    "limitk",
    "limit_ratio",
];

const KEYWORDS: &[&str] = &[
    "by",
    "without",
    "on",
    "ignoring",
    "group_left",
    "group_right",
    "bool",
    "offset",
    "and",
    "or",
    "unless",
    "atan2",
    "inf",
    "nan",
    "start",
    "end",
];

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Open(char),
    Close(char),
    Comma,
    Op(String),
    Other,
}

fn lex(expr: &str) -> Vec<Token> {
    let chars: Vec<char> = expr.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '#' => {
                // A comment, to the end of the line.
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '"' | '\'' | '`' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && quote != '`' && i + 1 < chars.len() {
                        i += 1;
                        s.push(match chars[i] {
                            'n' => '\n',
                            't' => '\t',
                            other => other,
                        });
                    } else {
                        s.push(chars[i]);
                    }
                    i += 1;
                }
                i += 1;
                out.push(Token::Str(s));
            }
            '(' | '{' | '[' => {
                out.push(Token::Open(c));
                i += 1;
            }
            ')' | '}' | ']' => {
                out.push(Token::Close(c));
                i += 1;
            }
            ',' => {
                out.push(Token::Comma);
                i += 1;
            }
            '=' | '!' | '<' | '>' => {
                let mut op = c.to_string();
                if i + 1 < chars.len() && matches!(chars[i + 1], '=' | '~') {
                    op.push(chars[i + 1]);
                    i += 1;
                }
                out.push(Token::Op(op));
                i += 1;
            }
            '$' => {
                // A template variable: `$name` or `${name…}`, never a metric.
                i += 1;
                if i < chars.len() && chars[i] == '{' {
                    while i < chars.len() && chars[i] != '}' {
                        i += 1;
                    }
                    i += 1;
                } else {
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        i += 1;
                    }
                }
                out.push(Token::Other);
            }
            c if c.is_ascii_alphabetic() || c == '_' || c == ':' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || chars[i] == '_' || chars[i] == ':')
                {
                    i += 1;
                }
                out.push(Token::Ident(chars[start..i].iter().collect()));
            }
            c if c.is_ascii_digit() || c == '.' => {
                // A number or a duration such as 1m30s.
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                out.push(Token::Other);
            }
            _ => {
                out.push(Token::Other);
                i += 1;
            }
        }
    }
    out
}

/// What `expr` reads.
pub fn parse(expr: &str) -> Query {
    let tokens = lex(expr);
    let mut q = Query::default();
    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Ident(name) if name == "by" || name == "without" => {
                if let Some((labels, next)) = label_list(&tokens, i + 1) {
                    if name == "by" {
                        q.grouped_by.extend(labels);
                    } else {
                        q.grouped_without.extend(labels);
                    }
                    i = next;
                    continue;
                }
                i += 1;
            }
            Token::Ident(name)
                if matches!(
                    name.as_str(),
                    "on" | "ignoring" | "group_left" | "group_right"
                ) =>
            {
                // Vector matching labels are not selectors.
                i = label_list(&tokens, i + 1).map_or(i + 1, |(_, next)| next);
            }
            Token::Ident(name) if KEYWORDS.contains(&name.as_str()) => i += 1,
            Token::Ident(name) => match tokens.get(i + 1) {
                Some(Token::Ident(next))
                    if AGGREGATIONS.contains(&name.as_str())
                        && (next == "by" || next == "without") =>
                {
                    q.functions.push(name.clone());
                    i += 1;
                }
                Some(Token::Open('(')) => {
                    q.functions.push(name.clone());
                    i += 1;
                }
                Some(Token::Open('{')) => {
                    let (matchers, next) = matchers(&tokens, i + 2);
                    q.selectors.push(Selector {
                        metric: Some(name.clone()),
                        matchers,
                    });
                    i = next;
                }
                _ => {
                    q.selectors.push(Selector {
                        metric: Some(name.clone()),
                        matchers: Vec::new(),
                    });
                    i += 1;
                }
            },
            Token::Open('{') => {
                let (matchers, next) = matchers(&tokens, i + 1);
                let metric = matchers
                    .iter()
                    .find(|m| m.label == "__name__" && m.op == MatchOp::Equal)
                    .map(|m| m.value.clone());
                q.selectors.push(Selector { metric, matchers });
                i = next;
            }
            Token::Open('[') => {
                // A range or subquery: durations and template variables only.
                while i < tokens.len() && tokens[i] != Token::Close(']') {
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    q
}

/// `(a, b)` starting at `start`; the labels and the index after the list.
fn label_list(tokens: &[Token], start: usize) -> Option<(Vec<String>, usize)> {
    if tokens.get(start) != Some(&Token::Open('(')) {
        return None;
    }
    let mut labels = Vec::new();
    let mut i = start + 1;
    while i < tokens.len() {
        match &tokens[i] {
            Token::Close(')') => return Some((labels, i + 1)),
            Token::Ident(l) => labels.push(l.clone()),
            _ => {}
        }
        i += 1;
    }
    Some((labels, i))
}

/// Matchers from just inside `{` to its `}`.
fn matchers(tokens: &[Token], start: usize) -> (Vec<Matcher>, usize) {
    let mut out = Vec::new();
    let mut i = start;
    while i < tokens.len() {
        match (&tokens[i], tokens.get(i + 1), tokens.get(i + 2)) {
            (Token::Close('}'), _, _) => return (out, i + 1),
            (Token::Ident(label), Some(Token::Op(op)), Some(Token::Str(value))) => {
                let op = match op.as_str() {
                    "=" => MatchOp::Equal,
                    "!=" => MatchOp::NotEqual,
                    "=~" => MatchOp::Regex,
                    "!~" => MatchOp::NotRegex,
                    _ => {
                        i += 1;
                        continue;
                    }
                };
                out.push(Matcher {
                    label: label.clone(),
                    op,
                    value: value.clone(),
                });
                i += 3;
            }
            _ => i += 1,
        }
    }
    (out, i)
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn a_quantile_query_reads_one_histogram() {
        let q = parse(
            r#"histogram_quantile($p, sum(rate(task_latency_queue_bucket{operation=~"VisibilityTask.*",service_name="history",namespace="$namespace"}[$__rate_interval])) by (operation, le))"#,
        );
        assert_eq!(q.metrics(), vec!["task_latency_queue_bucket"]);
        assert_eq!(q.functions, vec!["histogram_quantile", "sum", "rate"]);
        assert_eq!(q.grouped_by, vec!["operation", "le"]);
        assert_eq!(
            q.selectors[0].matchers,
            vec![
                Matcher {
                    label: "operation".into(),
                    op: MatchOp::Regex,
                    value: "VisibilityTask.*".into()
                },
                Matcher {
                    label: "service_name".into(),
                    op: MatchOp::Equal,
                    value: "history".into()
                },
                Matcher {
                    label: "namespace".into(),
                    op: MatchOp::Equal,
                    value: "$namespace".into()
                },
            ]
        );
        assert_eq!(
            q.labels(),
            vec!["le", "namespace", "operation", "service_name"]
        );
        assert!(q.uses("histogram_quantile"));
    }

    #[test]
    fn ratios_bare_names_and_vector_matching() {
        let q = parse(
            "sum by (instance) (rate(workflow_context_cleared[5m])) / on (instance) group_left sum by (instance) (rate(cache_miss{cache_type=\"mutablestate\"}[5m])) > bool 2",
        );
        assert_eq!(q.metrics(), vec!["cache_miss", "workflow_context_cleared"]);
        assert_eq!(q.grouped_by, vec!["instance", "instance"]);
        assert_eq!(
            parse("up == 0 or absent(up{job=\"temporal\"})").metrics(),
            vec!["up"]
        );
        let named = parse(r#"{__name__="persistence_requests", operation!="X"} offset 5m"#);
        assert_eq!(named.metrics(), vec!["persistence_requests"]);
        assert_eq!(named.labels(), vec!["operation"]);
        assert_eq!(
            parse("max without (pod) (numshards_gauge)").grouped_without,
            vec!["pod"]
        );
        // Comments and strings are not selectors.
        assert_eq!(
            parse("# rate of x\nrate(x_total{a=\"by (b)\"}[1m])").metrics(),
            vec!["x_total"]
        );
    }
}
