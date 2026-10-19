//! The dispatch-limit scenario: the wait a Task Queue's rate limit alone causes.

use std::time::Duration;

use kyumei_sim::dispatch::DispatchCase;

const BUDGET: Duration = Duration::from_secs(120);

#[test]
fn adds_above_the_limit_wait_in_the_backlog() {
    // The golden investigation: 2,112 Workflow Tasks a second against two
    // partitions at 1,000 a second each, for the 15-minute window.
    let case = DispatchCase {
        add_rate: 2_112.4,
        dispatch_limit: 2_000.0,
        read_partitions: 2,
        duration_s: 900,
        seed: 7,
    };
    let result = case.run(BUDGET).expect("run");
    eprintln!("{result:#?}");
    assert_eq!(result.scale, 20.0);
    // Dispatch holds at the scaled limit while adds exceed it.
    assert!(
        (95.0..=101.0).contains(&result.dispatch_rate),
        "{result:#?}"
    );
    assert!(result.add_rate > result.dispatch_rate * 1.03, "{result:#?}");
    // The excess, 5.6 % of the limit for 15 minutes, waits about 50 s by the end.
    assert!(
        (30.0..=80.0).contains(&result.schedule_to_start_max),
        "{result:#?}"
    );
    assert!(result.backlog_age_max >= result.schedule_to_start_max * 0.5);
}

#[test]
fn adds_below_the_limit_hardly_wait() {
    let case = DispatchCase {
        add_rate: 1_000.0,
        dispatch_limit: 2_000.0,
        read_partitions: 2,
        duration_s: 300,
        seed: 7,
    };
    let result = case.run(BUDGET).expect("run");
    assert!(result.schedule_to_start_max < 1.0, "{result:#?}");
}

#[test]
fn the_same_case_gives_the_same_result() {
    let case = DispatchCase {
        add_rate: 160.0,
        dispatch_limit: 150.0,
        read_partitions: 3,
        duration_s: 120,
        seed: 3,
    };
    let (a, b) = (
        case.run(BUDGET).expect("run"),
        case.run(BUDGET).expect("run"),
    );
    assert_eq!(a.schedule_to_start_max, b.schedule_to_start_max);
    assert_eq!(a.scenario, b.scenario);
}
