use std::time::Duration;

use agent_core::{RunBudget, RunId, RunOutcome, SessionId};
use agent_harness::RunContext;
use anyhow::Context;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_target(false)
        .without_time()
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize tracing: {error}"))?;

    let budget = RunBudget::new(0, 0, Duration::from_secs(1))
        .context("failed to construct the M0 run budget")?;
    let mut context = RunContext::new(RunId::new(), SessionId::new(), budget);

    let started = context.start().context("failed to start the M0 run")?;
    tracing::info!(
        run_id = %started.run_id(),
        event_sequence = started.sequence().get(),
        "M0 agent runtime foundation started"
    );

    let finished = context
        .finish(RunOutcome::Completed)
        .context("failed to finish the M0 run")?;
    tracing::info!(
        run_id = %finished.run_id(),
        event_sequence = finished.sequence().get(),
        "M0 agent runtime foundation is ready"
    );

    Ok(())
}
