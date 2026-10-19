//! A tempdes scenario, run and scraped.

use std::time::{Duration, Instant};

use tempdes::{
    config::scenario::Scenario,
    model::build,
    report::prom::{self, Reading},
};

use crate::scrape::{Sample, ScrapeError, parse_exposition};

/// One scrape of the simulated cluster's metrics.
#[derive(Clone, Debug, PartialEq)]
pub struct Scrape {
    /// Simulated time since the run started, in milliseconds.
    pub at_ms: i64,
    pub samples: Vec<Sample>,
}

/// A finished run: what was simulated, and every scrape of it.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub scenario: String,
    pub seed: u64,
    /// Simulated time before measurement starts. tempdes resets its metrics
    /// when the warm-up ends, which a query reads as a restart, so evidence
    /// windows start after it.
    pub warmup_ms: i64,
    /// Simulated time measured after the warm-up.
    pub duration_ms: i64,
    pub scrapes: Vec<Scrape>,
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("the scenario can't be simulated: {0}")]
    Scenario(String),
    #[error("the simulated metrics could not be read: {0}")]
    Scrape(#[from] ScrapeError),
    #[error("the simulation stopped: {0}")]
    Stopped(String),
    #[error(
        "the simulation was stopped after {wall:?} of its budget, {simulated_ms} ms into the run"
    )]
    Budget { wall: Duration, simulated_ms: i64 },
}

/// A scenario to simulate, in tempdes's YAML format.
#[derive(Clone, Debug)]
pub struct Simulation {
    scenario: String,
    seed: Option<u64>,
    scrape_every_ms: i64,
    budget: Option<Duration>,
}

impl Simulation {
    pub fn new(scenario: impl Into<String>) -> Self {
        Self {
            scenario: scenario.into(),
            seed: None,
            scrape_every_ms: 15_000,
            budget: None,
        }
    }

    /// Stop the run, with [`RunError::Budget`], once it has taken `wall` of
    /// real time.
    pub fn budget(mut self, wall: Duration) -> Self {
        self.budget = Some(wall);
        self
    }

    /// Use this seed rather than the scenario's.
    pub fn seed(mut self, seed: u64) -> Self {
        self.seed = Some(seed);
        self
    }

    /// Scrape every `ms` of simulated time; every 15 s, Prometheus's usual
    /// interval, unless set.
    pub fn scrape_every_ms(mut self, ms: i64) -> Self {
        self.scrape_every_ms = ms;
        self
    }

    /// Run the scenario on a thread of its own: tempdes keeps its scheduler's
    /// state in thread-locals, so every run starts on a fresh thread.
    pub fn run(&self) -> Result<Run, RunError> {
        let simulation = self.clone();
        std::thread::Builder::new()
            .name("tempdes".to_owned())
            .spawn(move || simulation.run_here())
            .map_err(|e| RunError::Stopped(e.to_string()))?
            .join()
            .map_err(|panic| {
                RunError::Stopped(
                    panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_owned()))
                        .unwrap_or_else(|| "tempdes panicked".to_owned()),
                )
            })?
    }

    fn run_here(&self) -> Result<Run, RunError> {
        if self.scrape_every_ms <= 0 {
            return Err(RunError::Scenario(
                "scrapes need a positive interval".to_owned(),
            ));
        }
        let scenario = Scenario::parse_str(&self.scenario)
            .map_err(|e| RunError::Scenario(format!("{e:#}")))?;
        let overrides = tempdes::run::Overrides {
            seed: self.seed,
            ..Default::default()
        };
        let params = tempdes::run::prepare(&scenario, &overrides, None)
            .map_err(|e| RunError::Scenario(format!("{e:#}")))?;
        let (name, seed, warmup_us, duration_us) = (
            params.name.clone(),
            params.seed,
            params.warmup,
            params.duration,
        );
        let (ctx, mut executor) = build::build(params);
        let _rates = build::start(&ctx, &mut executor);
        let every_us = self.scrape_every_ms as u64 * 1_000;
        let mut scrapes = Vec::new();
        let mut at_us = every_us;
        let started = Instant::now();
        while at_us <= warmup_us + duration_us {
            if let Some(wall) = self.budget
                && started.elapsed() > wall
            {
                return Err(RunError::Budget {
                    wall,
                    simulated_ms: (at_us / 1_000) as i64,
                });
            }
            executor.run_until(at_us);
            scrapes.push(Scrape {
                at_ms: (at_us / 1_000) as i64,
                samples: parse_exposition(&prom::render_as(&ctx, Reading::Scrape))?,
            });
            at_us += every_us;
        }
        Ok(Run {
            scenario: name,
            seed,
            warmup_ms: (warmup_us / 1_000) as i64,
            duration_ms: (duration_us / 1_000) as i64,
            scrapes,
        })
    }
}
