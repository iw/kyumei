//! Time series charts, drawn as a dashboard panel draws its series: lines over
//! time, the panel's thresholds, and a legend with its statistics.
//!
//! The chart is data the caller prepares: values already in the panel's unit,
//! and every label already written as the panel writes it. This module only
//! lays it out, as accessible SVG with a text version. Like the diagrams, it
//! knows nothing about Temporal or Grafana.

use std::fmt::Write as _;

/// A chart ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Chart {
    /// Unique within a page; used for element ids.
    pub id: String,
    pub title: String,
    /// What the chart shows, for screen readers.
    pub description: String,
    /// Unix milliseconds, the window drawn.
    pub x_range: (i64, i64),
    pub y_range: (f64, f64),
    /// Positions and labels along each axis.
    pub x_ticks: Vec<(i64, String)>,
    pub y_ticks: Vec<(f64, String)>,
    pub series: Vec<ChartSeries>,
    pub thresholds: Vec<ChartThreshold>,
    /// Shade the bands between thresholds in their colours.
    pub threshold_areas: bool,
    /// The legend's statistic columns, such as Min, Max, Mean and Last *.
    pub legend_columns: Vec<String>,
    pub legend: LegendStyle,
    /// Lines under the chart: what it is drawn from, and anything left out of it.
    pub caption: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartSeries {
    pub name: String,
    /// A CSS colour.
    pub color: String,
    /// Unix milliseconds and values; NaN breaks the line.
    pub points: Vec<(i64, f64)>,
    /// The legend statistics, written, in the order of the columns.
    pub statistics: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChartThreshold {
    pub value: f64,
    pub label: String,
    pub color: String,
    pub dashed: bool,
    /// Drawn as a line; a threshold outside the drawn range is only listed.
    pub line: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LegendStyle {
    #[default]
    Table,
    List,
    Hidden,
}

const WIDTH: f64 = 720.0;
const PLOT_H: f64 = 220.0;
const LEFT: f64 = 76.0;
const RIGHT: f64 = 14.0;
const TOP: f64 = 30.0;
const AXIS: f64 = 22.0;
const ROW: f64 = 18.0;
const CHAR_W: f64 = 6.4;

/// Values `n` steps apart from `min` to `max`, at a round interval.
pub fn nice_ticks(min: f64, max: f64, target: usize) -> Vec<f64> {
    if !(min.is_finite() && max.is_finite()) || max <= min || target == 0 {
        return vec![min];
    }
    let raw = (max - min) / target as f64;
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 2.5, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * magnitude);
    let first = (min / step).ceil() * step;
    let mut ticks = Vec::new();
    let mut t = first;
    while t <= max + step * 1e-9 {
        // Round away floating point dust such as 0.30000000000000004.
        ticks.push((t / step).round() * step);
        t += step;
    }
    ticks
}

/// Times at a round interval (a minute to a day) across the window.
pub fn time_ticks(start_ms: i64, end_ms: i64, target: usize) -> Vec<i64> {
    const MINUTE: i64 = 60_000;
    let span = (end_ms - start_ms).max(1);
    let step = [1, 2, 5, 10, 15, 30, 60, 120, 180, 360, 720, 1440]
        .iter()
        .map(|m| m * MINUTE)
        .find(|s| span / s <= target as i64)
        .unwrap_or(1440 * MINUTE);
    let first = start_ms.div_euclid(step) * step
        + if start_ms.rem_euclid(step) == 0 {
            0
        } else {
            step
        };
    (0..)
        .map(|i| first + i * step)
        .take_while(|t| *t <= end_ms)
        .collect()
}

/// The chart as SVG.
pub fn render_chart(c: &Chart) -> String {
    let id = sanitize(&c.id);
    let plot_w = WIDTH - LEFT - RIGHT;
    let (x0, x1) = (
        c.x_range.0 as f64,
        (c.x_range.1.max(c.x_range.0 + 1)) as f64,
    );
    let (y0, y1) = if c.y_range.1 > c.y_range.0 {
        c.y_range
    } else {
        (c.y_range.0 - 1.0, c.y_range.0 + 1.0)
    };
    let px = |t: f64| LEFT + (t - x0) / (x1 - x0) * plot_w;
    let py = |v: f64| TOP + PLOT_H - (v - y0) / (y1 - y0) * PLOT_H;
    let legend_rows = match c.legend {
        LegendStyle::Hidden => 0,
        _ => c.series.len() + 1,
    };
    let height = TOP + PLOT_H + AXIS + 8.0 + legend_rows as f64 * ROW + 6.0;
    let mut b = String::new();

    let _ = write!(
        b,
        r#"<text x="{LEFT}" y="18" class="kc-title">{}</text>"#,
        esc(&c.title)
    );
    let _ = write!(
        b,
        r#"<clipPath id="{id}-plot"><rect x="{LEFT}" y="{TOP}" width="{plot_w:.1}" height="{PLOT_H}"/></clipPath>"#
    );

    // Threshold bands: each from its value up to the next threshold.
    if c.threshold_areas {
        let mut levels: Vec<&ChartThreshold> = c.thresholds.iter().collect();
        levels.sort_by(|a, b| a.value.total_cmp(&b.value));
        for (i, t) in levels.iter().enumerate() {
            let from = t.value.max(y0);
            let to = levels.get(i + 1).map_or(y1, |n| n.value.min(y1));
            if to > from {
                let _ = write!(
                    b,
                    r#"<rect x="{LEFT}" y="{:.1}" width="{plot_w:.1}" height="{:.1}" fill="{}" class="kc-band"/>"#,
                    py(to),
                    py(from) - py(to),
                    esc(&t.color)
                );
            }
        }
    }

    // Grid and axes.
    for (v, label) in &c.y_ticks {
        if *v < y0 || *v > y1 {
            continue;
        }
        let y = py(*v);
        let _ = write!(
            b,
            r#"<line x1="{LEFT}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" class="kc-grid"/><text x="{:.1}" y="{:.1}" text-anchor="end" class="kc-tick">{}</text>"#,
            LEFT + plot_w,
            LEFT - 6.0,
            y + 4.0,
            esc(label)
        );
    }
    for (t, label) in &c.x_ticks {
        let x = px(*t as f64);
        if x < LEFT - 0.5 || x > LEFT + plot_w + 0.5 {
            continue;
        }
        let _ = write!(
            b,
            r#"<line x1="{x:.1}" y1="{TOP}" x2="{x:.1}" y2="{:.1}" class="kc-grid"/><text x="{x:.1}" y="{:.1}" text-anchor="middle" class="kc-tick">{}</text>"#,
            TOP + PLOT_H,
            TOP + PLOT_H + 15.0,
            esc(label)
        );
    }
    let _ = write!(
        b,
        r#"<rect x="{LEFT}" y="{TOP}" width="{plot_w:.1}" height="{PLOT_H}" class="kc-frame"/>"#
    );

    // Threshold lines.
    for t in c
        .thresholds
        .iter()
        .filter(|t| t.line && t.value >= y0 && t.value <= y1)
    {
        let y = py(t.value);
        let _ = write!(
            b,
            r#"<line x1="{LEFT}" y1="{y:.1}" x2="{:.1}" y2="{y:.1}" stroke="{}" class="kc-threshold"{}><title>{}</title></line>"#,
            LEFT + plot_w,
            esc(&t.color),
            if t.dashed {
                r#" stroke-dasharray="6 4""#
            } else {
                ""
            },
            esc(&t.label)
        );
    }

    // Series: a path per run of real values; NaN breaks the line.
    let _ = write!(b, r#"<g clip-path="url(#{id}-plot)">"#);
    for s in &c.series {
        let mut d = String::new();
        let mut pen_down = false;
        let mut singles = Vec::new();
        for (i, (t, v)) in s.points.iter().enumerate() {
            if !v.is_finite() {
                pen_down = false;
                continue;
            }
            let (x, y) = (px(*t as f64), py(v.clamp(y0 - (y1 - y0), y1 + (y1 - y0))));
            let _ = write!(d, "{}{x:.1} {y:.1} ", if pen_down { "L" } else { "M" });
            let next_real = s.points.get(i + 1).is_some_and(|(_, n)| n.is_finite());
            if !pen_down && !next_real {
                singles.push((x, y));
            }
            pen_down = true;
        }
        let _ = write!(
            b,
            r#"<path d="{}" stroke="{}" class="kc-line"><title>{}</title></path>"#,
            d.trim_end(),
            esc(&s.color),
            esc(&s.name)
        );
        for (x, y) in singles {
            let _ = write!(
                b,
                r#"<circle cx="{x:.1}" cy="{y:.1}" r="2" fill="{}"/>"#,
                esc(&s.color)
            );
        }
    }
    b.push_str("</g>");

    // Legend: a table of the series and their statistics.
    if c.legend != LegendStyle::Hidden {
        let top = TOP + PLOT_H + AXIS + 8.0;
        let stat_w = 82.0;
        let name_w = WIDTH - LEFT - RIGHT - stat_w * c.legend_columns.len() as f64 - 16.0;
        let max_chars = (name_w / CHAR_W).floor().max(8.0) as usize;
        for (i, column) in c.legend_columns.iter().enumerate() {
            let x = LEFT + 16.0 + name_w + stat_w * (i as f64 + 1.0) - 4.0;
            let _ = write!(
                b,
                r#"<text x="{x:.1}" y="{:.1}" text-anchor="end" class="kc-legend-head">{}</text>"#,
                top + 12.0,
                esc(column)
            );
        }
        for (r, s) in c.series.iter().enumerate() {
            let y = top + ROW * (r as f64 + 1.0) + 12.0;
            let _ = write!(
                b,
                r#"<g class="kc-legend-row"><title>{}</title><rect x="{LEFT}" y="{:.1}" width="10" height="3" fill="{}"/><text x="{:.1}" y="{y:.1}" class="kc-legend">{}</text>"#,
                esc(&s.name),
                y - 5.0,
                esc(&s.color),
                LEFT + 16.0,
                esc(&shorten(&s.name, max_chars))
            );
            for (i, value) in s.statistics.iter().enumerate() {
                let x = LEFT + 16.0 + name_w + stat_w * (i as f64 + 1.0) - 4.0;
                let _ = write!(
                    b,
                    r#"<text x="{x:.1}" y="{y:.1}" text-anchor="end" class="kc-legend kc-num">{}</text>"#,
                    esc(value)
                );
            }
            b.push_str("</g>");
        }
    }

    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" class="kc" viewBox="0 0 {WIDTH:.0} {height:.0}" width="{WIDTH:.0}" height="{height:.0}" role="img" aria-labelledby="{id}-title {id}-desc"><title id="{id}-title">{}</title><desc id="{id}-desc">{}</desc>{STYLE}{b}</svg>"#,
        esc(&c.title),
        esc(&c.description)
    )
}

/// The chart as text: the legend as a table, then the thresholds and the caption.
pub fn render_chart_text(c: &Chart) -> String {
    let mut rows: Vec<Vec<String>> = vec![
        std::iter::once("Series".to_owned())
            .chain(c.legend_columns.iter().cloned())
            .collect(),
    ];
    for s in &c.series {
        rows.push(
            std::iter::once(s.name.clone())
                .chain(s.statistics.iter().cloned())
                .collect(),
        );
    }
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map_or(0, |c| c.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = format!("{}\n", c.title);
    for row in &rows {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let pad = widths[i].saturating_sub(cell.chars().count());
                if i == 0 {
                    format!("{cell}{}", " ".repeat(pad))
                } else {
                    format!("{}{cell}", " ".repeat(pad))
                }
            })
            .collect();
        out.push_str("  ");
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    if !c.thresholds.is_empty() {
        let t: Vec<&str> = c.thresholds.iter().map(|t| t.label.as_str()).collect();
        let _ = writeln!(out, "Thresholds: {}", t.join(", "));
    }
    for line in &c.caption {
        let _ = writeln!(out, "{line}");
    }
    out
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn sanitize(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("kc-{cleaned}")
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Defaults so a chart is legible on its own; the web interface overrides them
/// through CSS custom properties.
const STYLE: &str = r#"<style>
.kc{font-family:var(--kd-font,ui-sans-serif,system-ui,-apple-system,"Segoe UI",sans-serif);font-size:12px}
.kc .kc-title{fill:var(--kd-ink,#1b1e23);font-size:13px;font-weight:600}
.kc .kc-grid{stroke:var(--kc-grid,#e3e1db);stroke-width:1}
.kc .kc-frame{fill:none;stroke:var(--kd-line,#3a3f47);stroke-width:1}
.kc .kc-tick{fill:var(--kd-muted,#5b616b);font-size:11px;font-variant-numeric:tabular-nums}
.kc .kc-line{fill:none;stroke-width:1.6;stroke-linejoin:round;stroke-linecap:round}
.kc .kc-threshold{stroke-width:1.4}
.kc .kc-band{opacity:.12}
.kc .kc-legend{fill:var(--kd-ink,#1b1e23);font-size:12px}
.kc .kc-legend-head{fill:var(--kd-muted,#5b616b);font-size:11px}
.kc .kc-num{font-variant-numeric:tabular-nums}
</style>"#;

#[cfg(test)]
mod tests {
    use super::*;

    fn chart() -> Chart {
        Chart {
            id: "E1.chart-1".to_owned(),
            title: "Queue <latency>".to_owned(),
            description: "Two series over ten minutes.".to_owned(),
            x_range: (0, 600_000),
            y_range: (0.0, 6.0),
            x_ticks: vec![(0, "00:00".to_owned()), (300_000, "00:05".to_owned())],
            y_ticks: vec![(0.0, "0 s".to_owned()), (5.0, "5 s".to_owned())],
            series: vec![
                ChartSeries {
                    name: "start".to_owned(),
                    color: "#7eb26d".to_owned(),
                    points: vec![(0, 1.0), (60_000, f64::NAN), (120_000, 2.0), (180_000, 4.0)],
                    statistics: vec!["1 s".to_owned(), "4 s".to_owned()],
                },
                ChartSeries {
                    name: "a very long series name that will not fit in the legend at all, not even close to it".to_owned(),
                    color: "#eab839".to_owned(),
                    points: vec![(0, 0.5)],
                    statistics: vec!["500 ms".to_owned(), "500 ms".to_owned()],
                },
            ],
            thresholds: vec![
                ChartThreshold { value: 3.0, label: "3 s (orange)".to_owned(), color: "#ff9830".to_owned(), dashed: true, line: true },
                ChartThreshold { value: 50.0, label: "50 s (red), above the data".to_owned(), color: "#f2495c".to_owned(), dashed: true, line: false },
            ],
            threshold_areas: false,
            legend_columns: vec!["Min".to_owned(), "Max".to_owned()],
            legend: LegendStyle::Table,
            caption: vec!["Drawn as panel 3 of Test Dashboard v1.0.".to_owned()],
        }
    }

    #[test]
    fn draws_lines_broken_by_nan_and_thresholds_in_range() {
        let svg = render_chart(&chart());
        assert!(svg.starts_with("<svg") && svg.ends_with("</svg>"));
        assert!(svg.contains("<title id=\"kc-E1-chart-1-title\">Queue &lt;latency&gt;</title>"));
        // NaN breaks the first series into two runs.
        let first = svg.split("class=\"kc-line\"").next().unwrap_or_default();
        assert_eq!(
            first
                .rsplit("d=\"")
                .next()
                .unwrap_or_default()
                .matches('M')
                .count(),
            2
        );
        // The lone point of the second series is drawn as a dot.
        assert!(svg.contains("<circle"));
        assert!(svg.contains("stroke-dasharray=\"6 4\""));
        assert_eq!(
            svg.matches("class=\"kc-threshold\"").count(),
            1,
            "a threshold above the range is not drawn"
        );
        assert!(svg.contains("a very long series name that will not fit in the legend at all, not even close to it</title>"));
        assert!(svg.contains('…'));
    }

    #[test]
    fn the_text_version_lists_the_legend_thresholds_and_caption() {
        let text = render_chart_text(&chart());
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Queue <latency>");
        assert!(
            lines[1].starts_with("  Series") && lines[1].ends_with("Max"),
            "{text}"
        );
        assert!(
            lines[2].starts_with("  start") && lines[2].ends_with("4 s"),
            "{text}"
        );
        assert!(text.contains("Thresholds: 3 s (orange), 50 s (red), above the data"));
        assert!(text.ends_with("Drawn as panel 3 of Test Dashboard v1.0.\n"));
    }

    #[test]
    fn ticks_fall_on_round_values_and_times() {
        assert_eq!(nice_ticks(0.0, 6.0, 4), vec![0.0, 2.0, 4.0, 6.0]);
        let written: Vec<String> = nice_ticks(0.004, 0.1, 4)
            .iter()
            .map(|v| format!("{v:.3}"))
            .collect();
        assert_eq!(written, vec!["0.025", "0.050", "0.075", "0.100"]);
        assert_eq!(
            time_ticks(90_000, 1_000_000, 6),
            vec![300_000, 600_000, 900_000]
        );
        assert_eq!(
            time_ticks(0, 3_600_000, 6),
            vec![
                0, 600_000, 1_200_000, 1_800_000, 2_400_000, 3_000_000, 3_600_000
            ]
        );
    }
}
