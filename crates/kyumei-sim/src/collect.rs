//! Answering Collect now's queries from a simulated run.

use kyumei_samples::{Point, QueryResult, Samples, Series, Window};

use crate::{
    Run, TEMPDES_COMMIT,
    promql::{self, EvalError, ParseError, Store},
};

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CollectError {
    #[error(transparent)]
    Parse(#[from] ParseError),
    #[error(transparent)]
    Eval(#[from] EvalError),
}

impl Run {
    /// Answer `queries` over `window` as Collect now answers them from
    /// Prometheus, with the run's simulated time zero standing for `origin_ms`
    /// on the wall clock.
    pub fn collect(
        &self,
        queries: &[String],
        window: Window,
        origin_ms: i64,
        collected_at_ms: i64,
    ) -> Result<Samples, CollectError> {
        let store = Store::new(&self.scrapes);
        let mut answers = Vec::new();
        for query in queries {
            let series = promql::evaluate_range(
                &promql::parse(query)?,
                &store,
                window.start_ms - origin_ms,
                window.end_ms - origin_ms,
                window.step_ms as i64,
            )?;
            answers.push(QueryResult {
                query: query.clone(),
                series: series
                    .into_iter()
                    .map(|s| Series {
                        labels: s.labels,
                        points: s
                            .points
                            .into_iter()
                            .map(|(at, value)| Point {
                                at_ms: at + origin_ms,
                                value,
                            })
                            .collect(),
                    })
                    .collect(),
            });
        }
        Ok(Samples {
            collector: format!("tempdes {}", &TEMPDES_COMMIT[..7]),
            source: format!("a simulation of {} (seed {})", self.scenario, self.seed),
            collected_at_ms,
            investigation: None,
            request: None,
            window,
            rounded_to: None,
            redacted: Vec::new(),
            queries: answers,
        })
    }
}
