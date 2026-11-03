//! The demo's one Workflow and its Activity: an invoice charged through a payment provider.

use std::{sync::Arc, time::Duration};

use temporalio_macros::{activities, workflow, workflow_methods};
use temporalio_sdk::{
    ActivityCloseTimeouts, ActivityOptions, WorkflowContext, WorkflowContextView, WorkflowResult,
    activities::{ActivityContext, ActivityError},
};

/// The Activity: a call to a payment provider that takes `seconds`.
pub struct Payments {
    pub seconds: f64,
}

#[activities]
impl Payments {
    // The SDK would otherwise name it `Payments::charge_customer`.
    #[activity(name = "charge_customer")]
    pub async fn charge_customer(
        self: Arc<Self>,
        _ctx: ActivityContext,
        invoice: i64,
    ) -> Result<String, ActivityError> {
        tokio::time::sleep(Duration::from_secs_f64(self.seconds)).await;
        Ok(format!("charged invoice {invoice}"))
    }
}

#[workflow]
pub struct Invoice {
    invoice: i64,
}

#[workflow_methods]
impl Invoice {
    #[init]
    fn new(_ctx: &WorkflowContextView, invoice: i64) -> Self {
        Self { invoice }
    }

    #[run]
    pub async fn run(ctx: &mut WorkflowContext<Self>) -> WorkflowResult<String> {
        let invoice = ctx.state(|s| s.invoice);
        // Long enough to outlast any backlog the demo builds, so tasks wait rather than time out.
        let options =
            ActivityOptions::with_close_timeouts(ActivityCloseTimeouts::ScheduleAndStartToClose {
                schedule_to_close: Duration::from_secs(6 * 3600),
                start_to_close: Duration::from_secs(60),
            })
            .build();
        Ok(ctx
            .execute_activity(Payments::charge_customer, invoice, options)
            .await?)
    }
}
