//! The `job` family of Workspace archive admission.

use crate::{JobRegistryApi, JobView};
use hoh_kernel::{Context, Disposer, Event, KernelError, Parallel, Waterfall};
use hoh_session::SessionId;
use std::sync::Arc;

// SHIM(hoh-workspace): SessionActivity and the two archive-admission events; replace when
// hoh-workspace provides its service vocabulary and event types.
mod shim {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct SessionActivityItem {
        pub id: String,
        pub label: Option<String>,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(crate) struct SessionActivity {
        pub kind: String,
        pub items: Option<Vec<SessionActivityItem>>,
    }

    #[derive(Clone)]
    pub(crate) struct WorkspaceSessionActivity {
        pub session_id: SessionId,
    }
    impl Event for WorkspaceSessionActivity {
        const NAME: &'static str = "workspace/session-activity";
        type Mode = Waterfall<Vec<SessionActivity>>;
    }

    #[derive(Clone)]
    pub(crate) struct WorkspaceSessionStop {
        pub session_id: SessionId,
    }
    impl Event for WorkspaceSessionStop {
        const NAME: &'static str = "workspace/session-stop";
        type Mode = Parallel;
    }
}

/// Install the Workspace archive-admission listeners for one registry provider.
pub fn install_job_archive_admission(
    ctx: &Context,
    registry: Arc<dyn JobRegistryApi>,
) -> Result<(Disposer, Disposer), KernelError> {
    let activity_registry = registry.clone();
    let activity = ctx
        .on_waterfall::<shim::WorkspaceSessionActivity, Vec<shim::SessionActivity>, _>(
            move |request, next| {
                let registry = activity_registry.clone();
                async move {
                    let jobs = running_jobs(registry.as_ref(), &request.session_id);
                    let rest = next.run().await?;
                    if jobs.is_empty() {
                        return Ok(rest);
                    }
                    let own = shim::SessionActivity {
                        kind: "job".to_owned(),
                        items: Some(
                            jobs.into_iter()
                                .map(|job| shim::SessionActivityItem {
                                    id: job.id.into(),
                                    label: Some(job.label),
                                })
                                .collect(),
                        ),
                    };
                    Ok(std::iter::once(own).chain(rest).collect())
                }
            },
        )?;

    let stop_registry = registry;
    let logger = ctx.logger();
    let stop = ctx.on_parallel::<shim::WorkspaceSessionStop, _>(move |request| {
        let registry = stop_registry.clone();
        let logger = logger.clone();
        async move {
            for job in running_jobs(registry.as_ref(), &request.session_id) {
                if let Err(error) =
                    registry.kill(&job.id, Some(&request.session_id), Some("session archived"))
                {
                    logger.warn(format_args!(
                        "jobs: killing \"{}\" for an archived Session failed: {error}",
                        job.id
                    ));
                }
            }
            Ok(())
        }
    })?;
    Ok((activity, stop))
}

fn running_jobs(registry: &dyn JobRegistryApi, owner: &SessionId) -> Vec<JobView> {
    registry
        .list(Some(owner))
        .into_iter()
        .filter(|job| job.owner.as_ref() == Some(owner) && job.status.is_live())
        .collect()
}

#[cfg(test)]
#[path = "../tests/archive_admission.rs"]
mod tests;
