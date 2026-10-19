//! The PromQL Kyumei's evidence requests use, answered over scrapes.
//!
//! Requests select series by label, read counters through `rate`, read
//! histograms through `histogram_quantile`, and aggregate with `sum` or `max`
//! by labels. That subset, with `increase`, `min`, `avg` and `count`, is parsed
//! here and evaluated as Prometheus evaluates a range query:
//!
//! - a selector takes each series' latest sample from the five minutes up to
//!   the evaluation time;
//! - `rate` and `increase` take the samples in their range, count a drop as a
//!   counter restart, and extrapolate the increase to the range's edges;
//! - `histogram_quantile` interpolates linearly within the bucket the quantile
//!   falls in;
//! - functions and aggregations drop the metric name.
//!
//! Anything outside the subset is refused with the reason, never answered
//! approximately. Times are milliseconds, as in Prometheus.

use std::collections::BTreeMap;

use regex::Regex;

use crate::{Scrape, scrape::number};

/// How far back a selector looks for a series' latest sample.
const LOOKBACK_MS: i64 = 5 * 60 * 1_000;

pub type Labels = BTreeMap<String, String>;

/// A parsed query.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// `metric{label="value", …}`.
    Selector(Selector),
    /// `rate(selector[range])` or `increase(selector[range])`.
    Range {
        function: RangeFunction,
        selector: Selector,
        range_ms: i64,
    },
    /// `sum by (…) (expr)` and the like.
    Aggregate {
        op: Aggregation,
        grouping: Grouping,
        expr: Box<Expr>,
    },
    /// `histogram_quantile(φ, expr)`.
    HistogramQuantile { quantile: f64, expr: Box<Expr> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RangeFunction {
    Rate,
    Increase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Aggregation {
    Sum,
    Max,
    Min,
    Avg,
    Count,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grouping {
    /// No `by` or `without`: one group.
    All,
    By(Vec<String>),
    Without(Vec<String>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    pub metric: String,
    pub matchers: Vec<Matcher>,
}

#[derive(Clone, Debug)]
pub struct Matcher {
    pub label: String,
    pub op: MatchOp,
    pub value: String,
    regex: Option<Regex>,
}

impl PartialEq for Matcher {
    fn eq(&self, other: &Self) -> bool {
        self.label == other.label && self.op == other.op && self.value == other.value
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchOp {
    Equal,
    NotEqual,
    Regex,
    NotRegex,
}

impl Matcher {
    fn new(label: String, op: MatchOp, value: String) -> Result<Self, String> {
        // Prometheus anchors a matcher's regular expression at both ends.
        let regex = match op {
            MatchOp::Regex | MatchOp::NotRegex => Some(
                Regex::new(&format!("^(?:{value})$"))
                    .map_err(|e| format!("`{value}` is not a regular expression: {e}"))?,
            ),
            MatchOp::Equal | MatchOp::NotEqual => None,
        };
        Ok(Self {
            label,
            op,
            value,
            regex,
        })
    }

    /// A label the series doesn't have matches as the empty string.
    fn matches(&self, labels: &Labels) -> bool {
        let v = labels.get(&self.label).map_or("", String::as_str);
        let regex = || self.regex.as_ref().is_some_and(|r| r.is_match(v));
        match self.op {
            MatchOp::Equal => v == self.value,
            MatchOp::NotEqual => v != self.value,
            MatchOp::Regex => regex(),
            MatchOp::NotRegex => !regex(),
        }
    }
}

impl Selector {
    fn matches(&self, metric: &str, labels: &Labels) -> bool {
        metric == self.metric && self.matchers.iter().all(|m| m.matches(labels))
    }
}

#[derive(Debug, PartialEq, thiserror::Error)]
#[error("{message}, at character {at} of `{query}`")]
pub struct ParseError {
    pub query: String,
    pub at: usize,
    pub message: String,
}

/// Parse a query, or say why it is outside the subset.
pub fn parse(query: &str) -> Result<Expr, ParseError> {
    let error = |at: usize, message: String| ParseError {
        query: query.to_owned(),
        at,
        message,
    };
    let tokens = lex(query).map_err(|(at, m)| error(at, m))?;
    let mut parser = Parser { tokens, pos: 0 };
    let expr = parser.expr().map_err(|(at, m)| error(at, m))?;
    match parser.tokens.get(parser.pos) {
        None => Ok(expr),
        Some((at, t)) => Err(error(*at, format!("unexpected {t:?} after the query"))),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Number(f64),
    Range(i64),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Op(MatchOp),
}

type Spanned = (usize, Token);

fn lex(query: &str) -> Result<Vec<Spanned>, (usize, String)> {
    let mut tokens = Vec::new();
    let chars: Vec<(usize, char)> = query.char_indices().collect();
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' | ')' | '{' | '}' | ',' => {
                tokens.push((
                    at,
                    match c {
                        '(' => Token::LParen,
                        ')' => Token::RParen,
                        '{' => Token::LBrace,
                        '}' => Token::RBrace,
                        _ => Token::Comma,
                    },
                ));
                i += 1;
            }
            '=' | '!' => {
                let next = chars.get(i + 1).map(|(_, c)| *c);
                let (op, len) = match (c, next) {
                    ('=', Some('~')) => (MatchOp::Regex, 2),
                    ('=', _) => (MatchOp::Equal, 1),
                    ('!', Some('=')) => (MatchOp::NotEqual, 2),
                    ('!', Some('~')) => (MatchOp::NotRegex, 2),
                    _ => return Err((at, "`!` must be followed by `=` or `~`".to_owned())),
                };
                tokens.push((at, Token::Op(op)));
                i += len;
            }
            '"' | '\'' => {
                let quote = c;
                let mut value = String::new();
                let mut j = i + 1;
                loop {
                    let Some(&(_, d)) = chars.get(j) else {
                        return Err((at, "a string is not closed".to_owned()));
                    };
                    j += 1;
                    match d {
                        '\\' => {
                            let Some(&(_, e)) = chars.get(j) else {
                                return Err((at, "a string is not closed".to_owned()));
                            };
                            j += 1;
                            value.push(match e {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                        d if d == quote => break,
                        d => value.push(d),
                    }
                }
                tokens.push((at, Token::Str(value)));
                i = j;
            }
            '[' => {
                let Some(close) = chars[i..].iter().position(|(_, c)| *c == ']') else {
                    return Err((at, "a range is not closed".to_owned()));
                };
                let text: String = chars[i + 1..i + close].iter().map(|(_, c)| *c).collect();
                let ms = duration_ms(text.trim())
                    .ok_or_else(|| (at, format!("`{text}` is not a duration")))?;
                tokens.push((at, Token::Range(ms)));
                i += close + 1;
            }
            c if c.is_ascii_digit() || c == '.' => {
                let mut j = i;
                while chars.get(j).is_some_and(|(_, d)| {
                    d.is_ascii_alphanumeric() || *d == '.' || *d == '+' || *d == '-'
                }) {
                    // A sign belongs to the number only after an exponent.
                    let (_, d) = chars[j];
                    if (d == '+' || d == '-') && !matches!(chars[j - 1].1, 'e' | 'E') {
                        break;
                    }
                    j += 1;
                }
                let text: String = chars[i..j].iter().map(|(_, c)| *c).collect();
                let n = text
                    .parse()
                    .map_err(|_| (at, format!("`{text}` is not a number")))?;
                tokens.push((at, Token::Number(n)));
                i = j;
            }
            c if c.is_ascii_alphabetic() || c == '_' || c == ':' => {
                let mut j = i;
                while chars
                    .get(j)
                    .is_some_and(|(_, d)| d.is_ascii_alphanumeric() || *d == '_' || *d == ':')
                {
                    j += 1;
                }
                tokens.push((
                    at,
                    Token::Ident(chars[i..j].iter().map(|(_, c)| *c).collect()),
                ));
                i = j;
            }
            other => return Err((at, format!("`{other}` is outside the PromQL Kyumei reads"))),
        }
    }
    Ok(tokens)
}

/// `1m`, `30s`, `1h30m`, `500ms`.
fn duration_ms(text: &str) -> Option<i64> {
    if text.is_empty() {
        return None;
    }
    let mut total = 0i64;
    let mut rest = text;
    while !rest.is_empty() {
        let digits = rest.find(|c: char| !c.is_ascii_digit())?;
        if digits == 0 {
            return None;
        }
        let n: i64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_len = rest
            .find(|c: char| c.is_ascii_digit())
            .unwrap_or(rest.len());
        let unit = match &rest[..unit_len] {
            "ms" => 1,
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            "w" => 604_800_000,
            "y" => 31_536_000_000,
            _ => return None,
        };
        total += n * unit;
        rest = &rest[unit_len..];
    }
    Some(total)
}

struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
}

type ParseResult<T> = Result<T, (usize, String)>;

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos).map(|(_, t)| t)
    }

    fn peek_at(&self, offset: usize) -> Option<&Token> {
        self.tokens.get(self.pos + offset).map(|(_, t)| t)
    }

    fn here(&self) -> usize {
        self.tokens
            .get(self.pos)
            .or(self.tokens.last())
            .map_or(0, |(at, _)| *at)
    }

    fn next(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).map(|(_, t)| t.clone());
        self.pos += 1;
        t
    }

    fn expect(&mut self, want: &Token, what: &str) -> ParseResult<()> {
        let at = self.here();
        match self.next() {
            Some(t) if &t == want => Ok(()),
            Some(t) => Err((at, format!("expected {what}, found {t:?}"))),
            None => Err((at, format!("expected {what} at the end"))),
        }
    }

    fn ident(&mut self, what: &str) -> ParseResult<String> {
        let at = self.here();
        match self.next() {
            Some(Token::Ident(name)) => Ok(name),
            Some(t) => Err((at, format!("expected {what}, found {t:?}"))),
            None => Err((at, format!("expected {what} at the end"))),
        }
    }

    fn expr(&mut self) -> ParseResult<Expr> {
        let at = self.here();
        match self.peek().cloned() {
            Some(Token::LParen) => {
                self.next();
                let inner = self.expr()?;
                self.expect(&Token::RParen, "`)`")?;
                Ok(inner)
            }
            Some(Token::Ident(name)) => {
                let opens = matches!(self.peek_at(1), Some(Token::LParen));
                let grouped =
                    matches!(self.peek_at(1), Some(Token::Ident(g)) if g == "by" || g == "without");
                match name.as_str() {
                    "sum" | "max" | "min" | "avg" | "count" if opens || grouped => self.aggregate(),
                    "rate" | "increase" if opens => self.range_function(),
                    "histogram_quantile" if opens => self.histogram_quantile(),
                    _ if opens => Err((at, format!("`{name}` is outside the PromQL Kyumei reads"))),
                    _ => Ok(Expr::Selector(self.selector()?)),
                }
            }
            Some(t) => Err((at, format!("expected a query, found {t:?}"))),
            None => Err((at, "the query is empty".to_owned())),
        }
    }

    fn aggregate(&mut self) -> ParseResult<Expr> {
        let op = match self.ident("an aggregation")?.as_str() {
            "sum" => Aggregation::Sum,
            "max" => Aggregation::Max,
            "min" => Aggregation::Min,
            "avg" => Aggregation::Avg,
            _ => Aggregation::Count,
        };
        let mut grouping = self.grouping()?;
        self.expect(&Token::LParen, "`(`")?;
        let expr = self.expr()?;
        self.expect(&Token::RParen, "`)`")?;
        if grouping == Grouping::All {
            grouping = self.grouping()?;
        }
        Ok(Expr::Aggregate {
            op,
            grouping,
            expr: Box::new(expr),
        })
    }

    fn grouping(&mut self) -> ParseResult<Grouping> {
        let by = match self.peek() {
            Some(Token::Ident(g)) if g == "by" => true,
            Some(Token::Ident(g)) if g == "without" => false,
            _ => return Ok(Grouping::All),
        };
        self.next();
        self.expect(&Token::LParen, "`(` after `by` or `without`")?;
        let mut labels = Vec::new();
        while !matches!(self.peek(), Some(Token::RParen)) {
            labels.push(self.ident("a label name")?);
            if matches!(self.peek(), Some(Token::Comma)) {
                self.next();
            }
        }
        self.next();
        Ok(if by {
            Grouping::By(labels)
        } else {
            Grouping::Without(labels)
        })
    }

    fn range_function(&mut self) -> ParseResult<Expr> {
        let function = match self.ident("a function")?.as_str() {
            "rate" => RangeFunction::Rate,
            _ => RangeFunction::Increase,
        };
        self.expect(&Token::LParen, "`(`")?;
        let selector = self.selector()?;
        let at = self.here();
        let range_ms = match self.next() {
            Some(Token::Range(ms)) if ms > 0 => ms,
            _ => return Err((at, "expected a range such as `[1m]`".to_owned())),
        };
        self.expect(&Token::RParen, "`)`")?;
        Ok(Expr::Range {
            function,
            selector,
            range_ms,
        })
    }

    fn histogram_quantile(&mut self) -> ParseResult<Expr> {
        self.next();
        self.expect(&Token::LParen, "`(`")?;
        let at = self.here();
        let quantile = match self.next() {
            Some(Token::Number(q)) => q,
            _ => return Err((at, "expected the quantile, such as 0.99".to_owned())),
        };
        self.expect(&Token::Comma, "`,`")?;
        let expr = self.expr()?;
        self.expect(&Token::RParen, "`)`")?;
        Ok(Expr::HistogramQuantile {
            quantile,
            expr: Box::new(expr),
        })
    }

    fn selector(&mut self) -> ParseResult<Selector> {
        let metric = self.ident("a metric name")?;
        let mut matchers = Vec::new();
        if matches!(self.peek(), Some(Token::LBrace)) {
            self.next();
            while !matches!(self.peek(), Some(Token::RBrace)) {
                let label = self.ident("a label name")?;
                let at = self.here();
                let op = match self.next() {
                    Some(Token::Op(op)) => op,
                    _ => return Err((at, "expected `=`, `!=`, `=~` or `!~`".to_owned())),
                };
                let at = self.here();
                let value = match self.next() {
                    Some(Token::Str(v)) => v,
                    _ => return Err((at, "expected a quoted label value".to_owned())),
                };
                matchers.push(Matcher::new(label, op, value).map_err(|m| (at, m))?);
                if matches!(self.peek(), Some(Token::Comma)) {
                    self.next();
                }
            }
            self.next();
        }
        Ok(Selector { metric, matchers })
    }
}

/// Every scraped series, with its samples in time order.
#[derive(Clone, Debug, Default)]
pub struct Store {
    series: Vec<Stored>,
}

#[derive(Clone, Debug)]
struct Stored {
    metric: String,
    /// The series' labels with `__name__`, as a selector returns them.
    labels: Labels,
    points: Vec<(i64, f64)>,
}

impl Store {
    pub fn new(scrapes: &[Scrape]) -> Self {
        let mut by_series: BTreeMap<(String, Labels), Vec<(i64, f64)>> = BTreeMap::new();
        for scrape in scrapes {
            for s in &scrape.samples {
                by_series
                    .entry((s.metric.clone(), s.labels.clone()))
                    .or_default()
                    .push((scrape.at_ms, s.value));
            }
        }
        let series = by_series
            .into_iter()
            .map(|((metric, mut labels), mut points)| {
                points.sort_by_key(|(at, _)| *at);
                labels.insert("__name__".to_owned(), metric.clone());
                Stored {
                    metric,
                    labels,
                    points,
                }
            })
            .collect();
        Self { series }
    }

    fn matching<'a>(&'a self, selector: &'a Selector) -> impl Iterator<Item = &'a Stored> + 'a {
        self.series
            .iter()
            .filter(|s| selector.matches(&s.metric, &s.labels))
    }
}

/// One series of a range query's answer: its labels and `(time, value)` points.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub labels: Labels,
    pub points: Vec<(i64, f64)>,
}

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum EvalError {
    #[error("a range query needs a positive step")]
    Step,
    #[error("a range query's end comes before its start")]
    Window,
}

/// Evaluate `expr` at `start_ms`, then every `step_ms` up to `end_ms`, as a
/// Prometheus range query does.
pub fn evaluate_range(
    expr: &Expr,
    store: &Store,
    start_ms: i64,
    end_ms: i64,
    step_ms: i64,
) -> Result<Vec<Series>, EvalError> {
    if step_ms <= 0 {
        return Err(EvalError::Step);
    }
    if end_ms < start_ms {
        return Err(EvalError::Window);
    }
    let mut answer: BTreeMap<Labels, Vec<(i64, f64)>> = BTreeMap::new();
    let mut at = start_ms;
    while at <= end_ms {
        for (labels, value) in evaluate_at(expr, store, at) {
            answer.entry(labels).or_default().push((at, value));
        }
        at += step_ms;
    }
    Ok(answer
        .into_iter()
        .map(|(labels, points)| Series { labels, points })
        .collect())
}

fn evaluate_at(expr: &Expr, store: &Store, at: i64) -> Vec<(Labels, f64)> {
    match expr {
        Expr::Selector(selector) => store
            .matching(selector)
            .filter_map(|s| {
                s.points
                    .iter()
                    .rev()
                    .find(|(t, _)| *t <= at)
                    .filter(|(t, _)| *t > at - LOOKBACK_MS)
                    .map(|(_, v)| (s.labels.clone(), *v))
            })
            .collect(),
        Expr::Range {
            function,
            selector,
            range_ms,
        } => store
            .matching(selector)
            .filter_map(|s| {
                let start = at - range_ms;
                let window: Vec<(i64, f64)> = s
                    .points
                    .iter()
                    .copied()
                    .filter(|(t, _)| *t > start && *t <= at)
                    .collect();
                extrapolated(
                    &window,
                    start,
                    at,
                    *range_ms,
                    *function == RangeFunction::Rate,
                )
                .map(|v| (without_name(&s.labels), v))
            })
            .collect(),
        Expr::Aggregate { op, grouping, expr } => {
            let mut groups: BTreeMap<Labels, Vec<f64>> = BTreeMap::new();
            for (labels, value) in evaluate_at(expr, store, at) {
                let key = match grouping {
                    Grouping::All => Labels::new(),
                    Grouping::By(keep) => labels
                        .into_iter()
                        .filter(|(k, _)| keep.contains(k))
                        .collect(),
                    Grouping::Without(drop) => labels
                        .into_iter()
                        .filter(|(k, _)| k != "__name__" && !drop.contains(k))
                        .collect(),
                };
                groups.entry(key).or_default().push(value);
            }
            groups
                .into_iter()
                .map(|(labels, values)| (labels, reduce(*op, &values)))
                .collect()
        }
        Expr::HistogramQuantile { quantile, expr } => {
            let mut groups: BTreeMap<Labels, Vec<(f64, f64)>> = BTreeMap::new();
            for (mut labels, count) in evaluate_at(expr, store, at) {
                let Some(bound) = labels.remove("le").and_then(|le| number(&le)) else {
                    continue;
                };
                labels.remove("__name__");
                groups.entry(labels).or_default().push((bound, count));
            }
            groups
                .into_iter()
                .map(|(labels, buckets)| (labels, bucket_quantile(*quantile, buckets)))
                .collect()
        }
    }
}

fn without_name(labels: &Labels) -> Labels {
    let mut labels = labels.clone();
    labels.remove("__name__");
    labels
}

fn reduce(op: Aggregation, values: &[f64]) -> f64 {
    match op {
        Aggregation::Sum => values.iter().sum(),
        Aggregation::Avg => values.iter().sum::<f64>() / values.len() as f64,
        Aggregation::Count => values.len() as f64,
        // NaN loses to any number, as in Prometheus.
        Aggregation::Max => values
            .iter()
            .copied()
            .fold(f64::NAN, |m, v| if m.is_nan() || v > m { v } else { m }),
        Aggregation::Min => values
            .iter()
            .copied()
            .fold(f64::NAN, |m, v| if m.is_nan() || v < m { v } else { m }),
    }
}

/// Prometheus's `extrapolatedRate` for a counter: the increase over the
/// samples, with drops counted as restarts, extrapolated towards the range's
/// edges, by the full distance when the samples reach within 110% of the
/// average interval of an edge, and by half an interval otherwise, and never
/// back past the counter's zero.
fn extrapolated(
    points: &[(i64, f64)],
    start: i64,
    end: i64,
    range_ms: i64,
    rate: bool,
) -> Option<f64> {
    if points.len() < 2 {
        return None;
    }
    let (first_t, first_v) = points[0];
    let (last_t, last_v) = points[points.len() - 1];
    let mut increase = last_v - first_v;
    let mut previous = first_v;
    for &(_, v) in &points[1..] {
        if v < previous {
            increase += previous;
        }
        previous = v;
    }
    let sampled = (last_t - first_t) as f64 / 1_000.0;
    let average = sampled / (points.len() - 1) as f64;
    let threshold = average * 1.1;
    let mut to_start = (first_t - start) as f64 / 1_000.0;
    if to_start >= threshold {
        to_start = average / 2.0;
    }
    if increase > 0.0 && first_v >= 0.0 {
        let to_zero = sampled * (first_v / increase);
        if to_zero < to_start {
            to_start = to_zero;
        }
    }
    let mut to_end = (end - last_t) as f64 / 1_000.0;
    if to_end >= threshold {
        to_end = average / 2.0;
    }
    let mut factor = (sampled + to_start + to_end) / sampled;
    if rate {
        factor /= range_ms as f64 / 1_000.0;
    }
    Some(increase * factor)
}

/// Prometheus's `bucketQuantile` for cumulative `(upper bound, count)` buckets.
fn bucket_quantile(quantile: f64, mut buckets: Vec<(f64, f64)>) -> f64 {
    if quantile.is_nan() {
        return f64::NAN;
    }
    if quantile < 0.0 {
        return f64::NEG_INFINITY;
    }
    if quantile > 1.0 {
        return f64::INFINITY;
    }
    buckets.sort_by(|a, b| a.0.total_cmp(&b.0));
    if buckets.last().is_none_or(|b| b.0 != f64::INFINITY) {
        return f64::NAN;
    }
    // Buckets with the same bound are added together, and counts that fall
    // (from scrapes taken mid-update) are held at the highest so far.
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (bound, count) in buckets {
        match merged.last_mut() {
            Some(last) if last.0 == bound => last.1 += count,
            _ => merged.push((bound, count)),
        }
    }
    let mut highest = f64::NEG_INFINITY;
    for bucket in &mut merged {
        if bucket.1 < highest {
            bucket.1 = highest;
        } else {
            highest = bucket.1;
        }
    }
    let n = merged.len();
    if n < 2 {
        return f64::NAN;
    }
    let observations = merged[n - 1].1;
    if observations == 0.0 || observations.is_nan() {
        return f64::NAN;
    }
    let mut rank = quantile * observations;
    let b = merged[..n - 1]
        .iter()
        .position(|(_, count)| *count >= rank)
        .unwrap_or(n - 1);
    if b == n - 1 {
        return merged[n - 2].0;
    }
    if b == 0 && merged[0].0 <= 0.0 {
        return merged[0].0;
    }
    let (mut start, end, mut count) = (0.0, merged[b].0, merged[b].1);
    if b > 0 {
        start = merged[b - 1].0;
        count -= merged[b - 1].1;
        rank -= merged[b - 1].1;
    }
    start + (end - start) * (rank / count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sample;

    /// A sample written out: metric, labels, value.
    type Raw<'a> = (&'a str, &'a [(&'a str, &'a str)], f64);

    fn scrape(at_ms: i64, samples: &[Raw<'_>]) -> Scrape {
        Scrape {
            at_ms,
            samples: samples
                .iter()
                .map(|(metric, labels, value)| Sample {
                    metric: (*metric).to_owned(),
                    labels: labels
                        .iter()
                        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                        .collect(),
                    value: *value,
                })
                .collect(),
        }
    }

    fn one(expr: &str, scrapes: &[Scrape], at: i64) -> Vec<(Labels, f64)> {
        evaluate_at(&parse(expr).expect("parse"), &Store::new(scrapes), at)
    }

    #[test]
    fn requests_queries_parse() {
        let e = parse(
            r#"histogram_quantile(0.95, sum by (le) (rate(temporal_workflow_task_schedule_to_start_latency_seconds_bucket{namespace="orders", task_queue=~"fulfil(:sticky)?"}[1m])))"#,
        )
        .expect("parse");
        let Expr::HistogramQuantile { quantile, expr } = e else {
            panic!("a quantile")
        };
        assert_eq!(quantile, 0.95);
        let Expr::Aggregate { op, grouping, expr } = *expr else {
            panic!("a sum")
        };
        assert_eq!(op, Aggregation::Sum);
        assert_eq!(grouping, Grouping::By(vec!["le".to_owned()]));
        let Expr::Range {
            function,
            selector,
            range_ms,
        } = *expr
        else {
            panic!("a rate")
        };
        assert_eq!(function, RangeFunction::Rate);
        assert_eq!(range_ms, 60_000);
        assert_eq!(selector.matchers[1].op, MatchOp::Regex);
        // `by` may follow the aggregated expression too.
        assert_eq!(
            parse("sum(x) by (a)").expect("parse"),
            parse("sum by (a) (x)").expect("parse")
        );
    }

    #[test]
    fn what_kyumei_does_not_read_is_refused() {
        for query in [
            "topk(3, x)",
            "x + y",
            "rate(x)",
            "x offset 5m",
            r#"x{a="b""#,
            "",
        ] {
            assert!(parse(query).is_err(), "{query}");
        }
    }

    #[test]
    fn a_regex_matcher_is_anchored() {
        let scrapes = [scrape(
            0,
            &[
                ("q", &[("task_queue", "fulfil")], 1.0),
                ("q", &[("task_queue", "fulfil:sticky")], 2.0),
                ("q", &[("task_queue", "fulfil-other")], 4.0),
            ],
        )];
        let got = one(r#"sum(q{task_queue=~"fulfil(:sticky)?"})"#, &scrapes, 0);
        assert_eq!(got, vec![(Labels::new(), 3.0)]);
    }

    #[test]
    fn rate_extrapolates_as_prometheus_does() {
        // A counter rising 10 a second, scraped every 15 s: over a minute, the
        // samples at 15, 30, 45 and 60 s cover 45 s of it and are extrapolated
        // to the full minute.
        let scrapes: Vec<Scrape> = (1..=4)
            .map(|i| scrape(i * 15_000, &[("c", &[], 100.0 + 150.0 * i as f64)]))
            .collect();
        let got = one("rate(c[1m])", &scrapes, 60_000);
        assert_eq!(got.len(), 1);
        assert!((got[0].1 - 10.0).abs() < 1e-9, "{}", got[0].1);
        let got = one("increase(c[1m])", &scrapes, 60_000);
        assert!((got[0].1 - 600.0).abs() < 1e-9, "{}", got[0].1);
        // A restart counts what came before it.
        let restarted = [
            scrape(15_000, &[("c", &[], 300.0)]),
            scrape(30_000, &[("c", &[], 450.0)]),
            scrape(45_000, &[("c", &[], 50.0)]),
            scrape(60_000, &[("c", &[], 200.0)]),
        ];
        let got = one("increase(c[1m])", &restarted, 60_000);
        // 150 + 50 + 150 = 350 over 45 s, extrapolated to 60 s.
        assert!(
            (got[0].1 - 350.0 * 60.0 / 45.0).abs() < 1e-9,
            "{}",
            got[0].1
        );
        // One sample is not a rate.
        assert!(one("rate(c[1m])", &scrapes[..1], 15_000).is_empty());
    }

    #[test]
    fn histogram_quantile_interpolates_within_its_bucket() {
        let buckets = |a: f64, b: f64, c: f64| -> Vec<Raw<'static>> {
            vec![
                ("h_bucket", &[("le", "0.1")], a),
                ("h_bucket", &[("le", "1")], b),
                ("h_bucket", &[("le", "+Inf")], c),
            ]
        };
        // 100 observations: 50 under 0.1 s, 40 more under 1 s, 10 above.
        let got = one(
            "histogram_quantile(0.75, h_bucket)",
            &[scrape(0, &buckets(50.0, 90.0, 100.0))],
            0,
        );
        // Rank 75 falls 25 of 40 into the 0.1–1 bucket.
        assert!((got[0].1 - (0.1 + 0.9 * 25.0 / 40.0)).abs() < 1e-12);
        // A quantile in the +Inf bucket is the highest finite bound.
        let got = one(
            "histogram_quantile(0.99, h_bucket)",
            &[scrape(0, &buckets(50.0, 90.0, 100.0))],
            0,
        );
        assert_eq!(got[0].1, 1.0);
        // No observations, no quantile.
        let got = one(
            "histogram_quantile(0.5, h_bucket)",
            &[scrape(0, &buckets(0.0, 0.0, 0.0))],
            0,
        );
        assert!(got[0].1.is_nan());
    }

    #[test]
    fn aggregations_group_and_drop_the_name() {
        let scrapes = [scrape(
            0,
            &[
                ("g", &[("pod", "a"), ("op", "x")], 1.0),
                ("g", &[("pod", "b"), ("op", "x")], 5.0),
                ("g", &[("pod", "b"), ("op", "y")], 2.0),
            ],
        )];
        let by_op = one("max by (op) (g)", &scrapes, 0);
        assert_eq!(
            by_op,
            vec![
                (Labels::from([("op".to_owned(), "x".to_owned())]), 5.0),
                (Labels::from([("op".to_owned(), "y".to_owned())]), 2.0),
            ]
        );
        let without_pod = one("sum without (pod) (g)", &scrapes, 0);
        assert_eq!(
            without_pod[0].0,
            Labels::from([("op".to_owned(), "x".to_owned())])
        );
        assert_eq!(without_pod[0].1, 6.0);
        assert_eq!(one("count(g)", &scrapes, 0)[0].1, 3.0);
        // A selector keeps the name; a stale sample is not returned.
        assert_eq!(one("g", &scrapes, 0)[0].0["__name__"], "g");
        assert!(one("g", &scrapes, LOOKBACK_MS).is_empty());
    }

    #[test]
    fn a_range_query_steps_through_its_window() {
        let scrapes: Vec<Scrape> = (0..=8)
            .map(|i| scrape(i * 15_000, &[("c", &[], 20.0 * 15.0 * i as f64)]))
            .collect();
        let answer = evaluate_range(
            &parse("rate(c[1m])").expect("parse"),
            &Store::new(&scrapes),
            60_000,
            120_000,
            30_000,
        )
        .expect("evaluate");
        assert_eq!(answer.len(), 1);
        let times: Vec<i64> = answer[0].points.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, vec![60_000, 90_000, 120_000]);
        assert!(
            answer[0]
                .points
                .iter()
                .all(|(_, v)| (v - 20.0).abs() < 1e-9)
        );
    }
}
