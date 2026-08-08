use goat_protocol::Event;
use goat_tool_plan::{PlanFuture, PlanService, PlanSubmission};
use tokio::sync::mpsc;

use crate::LoopEnv;

pub(crate) struct EnginePlanService {
    events: mpsc::Sender<Event>,
}

impl EnginePlanService {
    pub(crate) fn new(events: mpsc::Sender<Event>) -> Self {
        Self { events }
    }
}

impl PlanService for EnginePlanService {
    fn path(&self, host: Option<&(dyn std::any::Any + Send + Sync)>) -> Option<std::path::PathBuf> {
        host.and_then(|host| host.downcast_ref::<LoopEnv>())
            .and_then(|env| env.plan_path.clone())
    }

    fn submit(&self, submission: PlanSubmission) -> PlanFuture<'_> {
        Box::pin(async move {
            let _ = self
                .events
                .send(Event::PlanProposed {
                    id: submission.task,
                    call: submission.call,
                    plan: submission.plan,
                    path: submission.path.display().to_string(),
                })
                .await;
            Ok(())
        })
    }
}
