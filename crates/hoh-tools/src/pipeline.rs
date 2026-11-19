//! Shared tool invocation policy, cancellation, and result settlement.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use futures_util::FutureExt;
use hoh_agent::Agent;
use hoh_error::{BoxError, HarnessError};
use hoh_kernel::{Context, ListenerFailure, ServiceKey};
use hoh_llm::{ContentBlock, UserMessage};
use hoh_scope::{ScopeFilter, ScopeKey, Scoped, scope_target};
use hoh_timeout::{AbortSignal, is_abort_error};
use hoh_user_approval::{Approval, ApprovalOutcome, ApprovalRequest, DisplayReason};
use hoh_util_values::{is_json_value, snapshot_json_value};
use serde_json::{Map, Value};

use crate::contracts::{
    PostToolDecision, PreToolDecision, ResolvedToolSet, TOOL_ABORTED, TOOL_ABORTED_BEFORE_DISPATCH,
    ToolDefinition, ToolErrorInfo, ToolExecution, ToolExecutionFailure, ToolExecutionInput,
    ToolExecutionMode, ToolExecutionResult, ToolExecutionSuccess, ToolFailure, ToolNotFoundError,
    ToolOutputError,
};
use crate::error_codes::{
    APPROVAL_CANCELLED, APPROVAL_ERROR_NAME, APPROVAL_REJECTED, APPROVAL_UNAVAILABLE,
};
use crate::errors::{ToolsError, catch_panic, panic_error};
use crate::events::{ToolsExecute, ToolsPostExecute, ToolsPreExecute, ToolsResult};
use crate::{Tools, ToolsService};

pub use crate::batch::execute_tool_batch;

type GetFn = dyn Fn(&str, Option<&ScopeKey>, Option<&ResolvedToolSet>) -> Option<Arc<ToolDefinition>>
    + Send
    + Sync;
type RejectFn = dyn Fn(&str, bool) -> Option<BoxError> + Send + Sync;

/// Executor-owned lookup and restrictions applied before extensible execution policy.
#[derive(Clone, Default)]
pub struct ToolExecutionSource {
    /// Override registry lookup for executor-owned capabilities; omitted uses the retained registry's
    /// executable lookup, which applies the call's step admission.
    pub get: Option<Arc<GetFn>>,
    /// Reject an otherwise visible invocation before policy; omitted permits it.
    pub reject: Option<Arc<RejectFn>>,
}

/// Prepared invocation; immediate results carry their post-execute requirement.
pub enum ScheduledToolPreparation {
    Dispatch(Arc<ToolExecution>),
    /// Receives post-execute.
    PostResult {
        exec: Arc<ToolExecution>,
        result: ToolExecutionResult,
    },
    /// Bypasses post-execute.
    FinalResult {
        exec: Arc<ToolExecution>,
        result: ToolExecutionResult,
    },
}

impl ScheduledToolPreparation {
    /// The prepared execution of any variant.
    pub fn exec(&self) -> &Arc<ToolExecution> {
        match self {
            Self::Dispatch(exec)
            | Self::PostResult { exec, .. }
            | Self::FinalResult { exec, .. } => exec,
        }
    }
}

/// A `PostResult` receives post-execute; a `FinalResult` bypasses it.
pub enum ScheduledToolDispatch {
    PostResult(ToolExecutionResult),
    FinalResult(ToolExecutionResult),
}

/// Routes a dispatch to the listeners of the pipeline's `tools` realm.
pub(crate) struct ToolsFilter {
    realm: hoh_kernel::Realm,
}

impl ScopeFilter for ToolsFilter {
    fn matches_filter(&self, listener: &Context) -> bool {
        listener.realm_of(<Tools as ServiceKey>::NAME) == self.realm
    }
}

struct ToolAskResolution {
    decision: PreToolDecision,
    approval_cancelled: bool,
}

#[allow(clippy::large_enum_variant)]
enum Created {
    Ready(Arc<ToolExecution>),
    Done(ScheduledToolPreparation),
}

struct PipelineInner {
    ctx: Context,
    /// The retained `tools` service; its absence is an error only for operations that need it.
    tools: Option<ToolsService>,
    source: ToolExecutionSource,
    filter: Arc<ToolsFilter>,
}

/// Shared invocation stages for batch and program schedulers. Schedulers order prepare and
/// complete calls while dispatch may overlap. Use [`execute_tool_batch`] for complete calls,
/// including a single-call batch.
#[derive(Clone)]
pub struct ToolExecutionPipeline(Arc<PipelineInner>);

fn scope_of_agent(agent: &Option<Arc<dyn Agent>>) -> Option<ScopeKey> {
    agent.as_ref().map(|agent| agent.scope_key())
}

impl ToolExecutionPipeline {
    /// Retain the `tools` service of `ctx`: accepted calls keep their event subject while the service is
    /// withdrawn during shutdown. Construction never fails: without a `tools` service the pipeline is
    /// still created and only the operations that need the registry report `TOOL_EXECUTOR_UNAVAILABLE`.
    /// The `Result` is kept so callers of the earlier signature compile unchanged.
    pub fn new(ctx: Context) -> Result<Self, HarnessError> {
        Self::with_source(ctx, ToolExecutionSource::default())
    }

    pub fn with_source(ctx: Context, source: ToolExecutionSource) -> Result<Self, HarnessError> {
        let tools = ctx.get::<Tools>().map(|tools| ToolsService::clone(&tools));
        Ok(Self(Arc::new(PipelineInner {
            filter: Arc::new(ToolsFilter {
                realm: ctx.realm_of(<Tools as ServiceKey>::NAME),
            }),
            tools,
            ctx,
            source,
        })))
    }

    /// The retained registry. The source reads `ctx.tools` once and dereferences it on use, so a pipeline
    /// built without the service fails with the engine's bare `TypeError` naming the property it read.
    fn tools(&self, property: &str) -> Result<&ToolsService, BoxError> {
        self.0.tools.as_ref().ok_or_else(|| {
            type_error(&format!(
                "Cannot read properties of undefined (reading '{property}')"
            ))
        })
    }

    fn carrier(&self, agent: &Option<Arc<dyn Agent>>) -> Scoped<ToolsFilter> {
        scope_target(self.0.filter.clone(), scope_of_agent(agent))
    }

    fn get(
        &self,
        name: &str,
        scope: Option<&ScopeKey>,
        tool_set: Option<&ResolvedToolSet>,
    ) -> Result<Option<Arc<ToolDefinition>>, BoxError> {
        match &self.0.source.get {
            None => Ok(self.tools("executable")?.executable(name, scope, tool_set)),
            Some(get) => Ok(get(name, scope, tool_set)),
        }
    }

    fn rejection(&self, name: &str, nested: bool) -> Option<BoxError> {
        self.0
            .source
            .reject
            .as_ref()
            .and_then(|reject| reject(name, nested))
    }

    fn resolve_execution(
        &self,
        name: &str,
        scope: Option<&ScopeKey>,
        nested: bool,
        tool_set: Option<&ResolvedToolSet>,
    ) -> Result<Option<Arc<ToolDefinition>>, BoxError> {
        let Some(tool) = self.get(name, scope, tool_set)? else {
            return Ok(None);
        };
        Ok(self.rejection(name, nested).is_none().then_some(tool))
    }

    fn resolve_for(&self, exec: &ToolExecution) -> Result<Option<Arc<ToolDefinition>>, BoxError> {
        self.resolve_execution(
            &exec.name,
            scope_of_agent(&exec.agent).as_ref(),
            exec.parent.is_some(),
            exec.tool_set.as_ref(),
        )
    }

    /// Classify a pending call through the caller's visible tool definition. Only an exact `true`
    /// is parallel; unknown, hidden, undeclared, or throwing classifiers are exclusive. A failed
    /// lookup is not a classification: it rejects, as the source's `executionMode` throws.
    pub fn execution_mode(&self, exec: &ToolExecutionInput) -> Result<ToolExecutionMode, BoxError> {
        let tool = self.resolve_execution(
            &exec.name,
            scope_of_agent(&exec.agent).as_ref(),
            exec.parent.is_some(),
            exec.tool_set.as_ref(),
        )?;
        let Some(classify) = tool.and_then(|tool| tool.is_concurrency_safe.clone()) else {
            return Ok(ToolExecutionMode::Exclusive);
        };
        Ok(match catch_panic(|| classify(&exec.arguments)) {
            Ok(true) => ToolExecutionMode::Parallel,
            _ => ToolExecutionMode::Exclusive,
        })
    }

    /// The lookup runs outside the source's `try`: a failure of it rejects `prepare` instead of becoming
    /// a call result.
    fn create_execution(&self, input: &ToolExecutionInput) -> Result<Created, BoxError> {
        let scope = scope_of_agent(&input.agent);
        // Executor refusals precede extensible policy; unknown tools retain dispatch-stage handling.
        let visible = self.get(&input.name, scope.as_ref(), input.tool_set.as_ref())?;
        let rejection = visible
            .as_ref()
            .and_then(|_| self.rejection(&input.name, input.parent.is_some()));
        let rejected = rejection.is_some();
        // Capture callbacks before arguments are read; rejected calls retain finalization only when aborted.
        let captured_finalizer = visible
            .as_ref()
            .and_then(|tool| tool.finalize_content.clone());
        let captured_projector = visible
            .as_ref()
            .and_then(|tool| tool.project_content.clone());
        let finalizer = if rejected && !input.signal.aborted() {
            None
        } else {
            captured_finalizer
        };
        match snapshot_json_value(&input.arguments) {
            None => {
                let exec = Arc::new(ToolExecution::mint(input, Value::Null, finalizer, None));
                Ok(Created::Done(ScheduledToolPreparation::FinalResult {
                    exec,
                    result: tool_error_result(&type_error(
                        "tool execution arguments must be losslessly JSON-serializable",
                    )),
                }))
            }
            Some(arguments) => {
                let projector = if rejected { None } else { captured_projector };
                let exec = Arc::new(ToolExecution::mint(input, arguments, finalizer, projector));
                Ok(match rejection {
                    Some(error) => {
                        // Cancellation takes precedence over an executor refusal.
                        let result = if input.signal.aborted() {
                            tool_aborted_before_dispatch_result(None)
                        } else {
                            tool_error_result(&error)
                        };
                        Created::Done(ScheduledToolPreparation::FinalResult { exec, result })
                    }
                    None => Created::Ready(exec),
                })
            }
        }
    }

    /// Snapshot input, assign its correlation token, and run ordered pre-execute policy and guards.
    /// Cancellation skips a body that has not started.
    ///
    /// Like the source's `prepare`, it rejects when the tool lookup fails: a pipeline built without a
    /// `tools` service fails with a bare `TypeError` while looking the tool up, exactly as
    /// `undefined.executable` does, and that failure is not a call result.
    pub async fn prepare(
        &self,
        input: ToolExecutionInput,
    ) -> Result<ScheduledToolPreparation, BoxError> {
        let exec = match self.create_execution(&input)? {
            Created::Done(prepared) => return Ok(prepared),
            Created::Ready(exec) => exec,
        };
        if exec.caller_signal.aborted() {
            return Ok(ScheduledToolPreparation::FinalResult {
                exec,
                result: tool_aborted_before_dispatch_result(None),
            });
        }
        Ok(match self.prepare_policy(&exec).await {
            Ok(prepared) => prepared,
            Err(error) => ScheduledToolPreparation::FinalResult {
                exec,
                result: tool_error_result(&error),
            },
        })
    }

    async fn prepare_policy(
        &self,
        exec: &Arc<ToolExecution>,
    ) -> Result<ScheduledToolPreparation, BoxError> {
        let carrier = self.carrier(&exec.agent);
        let event = ToolsPreExecute { exec: exec.clone() };
        let gate = carrier
            .context(&self.0.ctx)
            .waterfall::<ToolsPreExecute, PreToolDecision, _>(event, |_| async {
                Ok(PreToolDecision::Allow)
            })
            .await?;
        let resolution = match gate {
            PreToolDecision::Ask {
                reason,
                display_reason,
            } => self.service_ask(exec, reason, display_reason).await?,
            decision => ToolAskResolution {
                decision,
                approval_cancelled: false,
            },
        };
        let aborted = || ScheduledToolPreparation::PostResult {
            exec: exec.clone(),
            result: tool_aborted_before_dispatch_result(None),
        };
        if exec.caller_signal.aborted() && resolution.approval_cancelled {
            return Ok(aborted());
        }
        let denial = match resolution.decision {
            PreToolDecision::Cancel => return Ok(aborted()),
            PreToolDecision::Allow => self
                .tools("checkGuards")?
                .check_guards(exec)?
                .map(|denial| (denial.reason, denial.info)),
            PreToolDecision::Deny { reason, info } => Some((reason, info)),
            PreToolDecision::Ask { .. } => None,
        };
        if let Some((reason, info)) = denial {
            let result = ToolExecutionResult::Failure(ToolExecutionFailure::new(
                ToolFailure {
                    message: reason.clone(),
                    info,
                },
                vec![ContentBlock::text(format!("Error: {reason}"))],
            ));
            return Ok(ScheduledToolPreparation::PostResult {
                exec: exec.clone(),
                result: materialize_final_result(result)?,
            });
        }
        if exec.caller_signal.aborted() {
            return Ok(aborted());
        }
        Ok(ScheduledToolPreparation::Dispatch(exec.clone()))
    }

    /// Canonical cancellation outcome selected by whether the tool body started.
    fn cancellation_result(
        &self,
        exec: &ToolExecution,
        prior: &ToolExecutionResult,
    ) -> ToolExecutionResult {
        if exec.body_invoked.load(std::sync::atomic::Ordering::SeqCst) {
            tool_aborted_result(Some(prior))
        } else {
            tool_aborted_before_dispatch_result(Some(prior))
        }
    }

    /// Dispatch the registered body with the original caller signal fused back into any
    /// around-wrapper replacement. Cancellation never abandons the body: a started future reaches
    /// quiescence before its outcome becomes `ABORTED`. After cancellation, a thrown signal reason or
    /// an `AbortError` without `HarnessError` identity keeps its message and gains the `ABORTED`
    /// identity; a thrown `HarnessError` and any other failure stay unchanged.
    async fn dispatch_tool_body(&self, exec: Arc<ToolExecution>) -> ToolExecutionResult {
        let wrapper_signal = exec.signal();
        let caller = exec.caller_signal.clone();
        let signal = if exec.signal_was_replaced() {
            // The wrapper's abort wins a simultaneous abort, as in the source's relay.
            AbortSignal::any([wrapper_signal.clone(), caller])
        } else {
            caller
        };
        if signal.aborted() {
            return tool_aborted_before_dispatch_result(None);
        }
        exec.restore_signal(signal.clone());
        let outcome = self.run_body(&exec, &signal).await;
        exec.restore_signal(wrapper_signal);
        match outcome {
            Ok(result) => result,
            Err(error) => {
                if signal.aborted() && is_unclassified_cancellation(&error, &signal) {
                    tool_error_result_with(
                        &error,
                        Some(ToolErrorInfo::new("AbortError", TOOL_ABORTED)),
                    )
                } else {
                    tool_error_result(&error)
                }
            }
        }
    }

    async fn run_body(
        &self,
        exec: &Arc<ToolExecution>,
        signal: &AbortSignal,
    ) -> Result<ToolExecutionResult, BoxError> {
        let Some(tool) = self.resolve_for(exec)? else {
            return Err(Box::new(ToolNotFoundError::new(&exec.name, None)));
        };
        exec.body_invoked
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let future = catch_panic(|| (tool.execute)(exec.arguments.clone(), exec.clone()))?;
        let returned = AssertUnwindSafe(future)
            .catch_unwind()
            .await
            .map_err(panic_error)??;
        let result = self.create_success_result(exec, &tool, returned)?;
        Ok(if signal.aborted() {
            tool_aborted_result(Some(&result))
        } else {
            result
        })
    }

    /// Run around-dispatch and the tool body. Tool and unknown-tool failures still receive
    /// post-execute; pipeline failures are already final. Started bodies drain on cancellation before
    /// a successful outcome becomes `ABORTED`.
    pub async fn dispatch(&self, exec: Arc<ToolExecution>) -> ScheduledToolDispatch {
        match self.dispatch_inner(&exec).await {
            Ok(result) => ScheduledToolDispatch::PostResult(result),
            Err(error) => {
                ScheduledToolDispatch::FinalResult(self.wrapper_failure_result(&exec, &error))
            }
        }
    }

    async fn dispatch_inner(
        &self,
        exec: &Arc<ToolExecution>,
    ) -> Result<ToolExecutionResult, BoxError> {
        let carrier = self.carrier(&exec.agent);
        let pipeline = self.clone();
        let body_exec = exec.clone();
        // The source's `dispatch` runs synchronously up to its first await. The kernel reports the end of
        // the initial polling pass of the whole wrapper chain; that is the scheduler's acknowledgement.
        let (started, answer) = carrier
            .context(&self.0.ctx)
            .waterfall_with_start::<ToolsExecute, ToolExecutionResult, _>(
                ToolsExecute { exec: exec.clone() },
                move |_| async move { Ok(pipeline.dispatch_tool_body(body_exec).await) },
            );
        let acknowledge = async {
            started.await;
            exec.signal_started();
        };
        let (result, ()) = tokio::join!(answer, acknowledge);
        let result = result?;
        let normalized = self.normalize_dispatch_result(exec, result)?;
        let deferred = exec.deferred_contexts();
        let with_deferred = if deferred.is_empty() {
            normalized
        } else {
            let mut contexts = deferred;
            contexts.extend(
                normalized
                    .additional_contexts()
                    .cloned()
                    .unwrap_or_default(),
            );
            with_additional_contexts(normalized, Some(contexts)).marked_editable(exec)
        };
        Ok(
            if exec.caller_signal.aborted() && !with_deferred.is_error() {
                self.cancellation_result(exec, &with_deferred)
            } else {
                with_deferred
            },
        )
    }

    /// Classify a failure thrown around dispatch; see [`Self::dispatch`].
    fn wrapper_failure_result(
        &self,
        exec: &ToolExecution,
        error: &BoxError,
    ) -> ToolExecutionResult {
        if !exec.caller_signal.aborted()
            || !is_unclassified_cancellation(error, &exec.caller_signal)
        {
            return tool_error_result(error);
        }
        let code = if exec.body_invoked.load(std::sync::atomic::Ordering::SeqCst) {
            TOOL_ABORTED
        } else {
            TOOL_ABORTED_BEFORE_DISPATCH
        };
        tool_error_result_with(error, Some(ToolErrorInfo::new("AbortError", code)))
    }

    /// Run post-execute when required, apply definition-owned content finalization, and publish
    /// the lossless result. Policy and finalization failures become error results. Cancellation
    /// replaces a successful outcome before final materialization; tool-owned errors remain intact.
    pub async fn complete(
        &self,
        exec: &Arc<ToolExecution>,
        outcome: ScheduledToolDispatch,
    ) -> ToolExecutionResult {
        let result = match outcome {
            ScheduledToolDispatch::FinalResult(result) => return self.finish(exec, result),
            ScheduledToolDispatch::PostResult(result) => result,
        };
        match self.complete_post(exec, result).await {
            Ok(result) => self.finish(exec, result),
            Err(error) => self.finish(exec, tool_error_result(&error)),
        }
    }

    async fn complete_post(
        &self,
        exec: &Arc<ToolExecution>,
        result: ToolExecutionResult,
    ) -> Result<ToolExecutionResult, BoxError> {
        let project = lock_take(&exec.projector);
        let content = match project {
            Some(project) => catch_panic(|| project(exec, &result))?,
            None => None,
        };
        let projected = match content {
            None => result,
            Some(content) => materialize_final_result(with_content(result, content))?.marked(exec),
        };
        let post = self.post_execute(exec, projected).await?;
        Ok(if exec.caller_signal.aborted() && !post.is_error() {
            self.cancellation_result(exec, &post)
        } else {
            post
        })
    }

    /// Materialize the candidate, apply definition-owned content finalization, then materialize
    /// and notify the authoritative result.
    fn finish(
        &self,
        exec: &Arc<ToolExecution>,
        result: ToolExecutionResult,
    ) -> ToolExecutionResult {
        let materialized = match materialize_final_result(result) {
            Ok(result) => result,
            Err(error) => materialize_or_fallback(tool_error_result(&error)),
        };
        let final_result = match self
            .apply_final_content(exec, materialized)
            .and_then(materialize_final_result)
        {
            Ok(result) => result,
            Err(error) => materialize_or_fallback(tool_error_result(&error)),
        };
        // The source's `materializeFinalResult` deep-freezes the authoritative outcome it publishes.
        let final_result = final_result.marked(exec);
        self.notify_result(exec, &final_result);
        final_result
    }

    /// Apply the snapshotted tool-owned content transform without exposing other result fields.
    fn apply_final_content(
        &self,
        exec: &Arc<ToolExecution>,
        result: ToolExecutionResult,
    ) -> Result<ToolExecutionResult, BoxError> {
        let Some(finalize) = exec.finalizer.clone() else {
            return Ok(result);
        };
        Ok(match catch_panic(|| finalize(exec, &result))? {
            None => result,
            Some(content) => with_content(result, content),
        })
    }

    /// Notify observers without exposing a mutation or error channel into the outcome.
    fn notify_result(&self, exec: &Arc<ToolExecution>, result: &ToolExecutionResult) {
        // Freeze the registry's live object before observers receive it.
        exec.freeze();
        let tool_name = exec.name.clone();
        let call_id = exec.call_id.clone();
        let log = self.0.ctx.logger();
        let report = move |failure: ListenerFailure| {
            let error = match failure {
                ListenerFailure::Threw(error) | ListenerFailure::Rejected(error) => error,
            };
            log.warn(format!(
                "tool \"{tool_name}\" ({call_id}): tools/result observer failed: {}",
                tool_error_message(&error)
            ));
        };
        let event = ToolsResult {
            exec: exec.clone(),
            result: Arc::new(result.clone()),
        };
        self.carrier(&exec.agent)
            .context(&self.0.ctx)
            .dispatch::<ToolsResult>(event)
            .invoke(report);
    }

    /// Resolve an `ask` decision to allow/deny through the approval seam. The seam is consumed
    /// opportunistically — a deployment that composes no approval service keeps the historical degrade
    /// to deny, and an unmount mid-session degrades the same way on the next ask. An agent-less
    /// execution also degrades: without an agent there is no session to audit to and no UI to route to.
    /// Every denial carries `ApprovalError` with `APPROVAL_REJECTED`, `APPROVAL_CANCELLED`, or
    /// `APPROVAL_UNAVAILABLE`.
    async fn service_ask(
        &self,
        exec: &Arc<ToolExecution>,
        reason: Option<String>,
        display_reason: Option<DisplayReason>,
    ) -> Result<ToolAskResolution, BoxError> {
        let Some(approval) = self.0.ctx.get::<Approval>() else {
            return Ok(ToolAskResolution {
                decision: approval_denial(
                    reason.unwrap_or_else(|| {
                        format!(
                            "tool \"{}\" requires approval (not yet supported)",
                            exec.name
                        )
                    }),
                    APPROVAL_UNAVAILABLE,
                ),
                approval_cancelled: false,
            });
        };
        let Some(agent) = exec.agent.clone() else {
            return Ok(ToolAskResolution {
                decision: approval_denial(
                    format!(
                        "tool \"{}\" requires approval, but the call has no agent to route it through",
                        exec.name
                    ),
                    APPROVAL_UNAVAILABLE,
                ),
                approval_cancelled: false,
            });
        };
        let mut request = ApprovalRequest::new(agent, exec.name.clone());
        request.call_id = Some(exec.call_id.clone());
        request.reason = reason;
        request.display_reason = display_reason;
        request.signal = Some(exec.signal());
        let outcome = approval.request(request).await?;
        let name = &exec.name;
        Ok(match outcome {
            ApprovalOutcome::AllowedOnce => ToolAskResolution {
                decision: PreToolDecision::Allow,
                approval_cancelled: false,
            },
            ApprovalOutcome::Rejected => ToolAskResolution {
                decision: approval_denial(
                    format!("the user rejected tool \"{name}\""),
                    APPROVAL_REJECTED,
                ),
                approval_cancelled: false,
            },
            ApprovalOutcome::Cancelled => ToolAskResolution {
                decision: approval_denial(
                    format!("approval for tool \"{name}\" was cancelled"),
                    APPROVAL_CANCELLED,
                ),
                approval_cancelled: true,
            },
            ApprovalOutcome::Unavailable => ToolAskResolution {
                decision: approval_denial(
                    format!(
                        "tool \"{name}\" requires approval, but no approval channel is available"
                    ),
                    APPROVAL_UNAVAILABLE,
                ),
                approval_cancelled: false,
            },
        })
    }

    /// Run the `tools/post-execute` waterfall over a dispatched `result` and apply its decision:
    /// `Accept` keeps the call successful (replacing `content` or `value` when given), `Block` turns it
    /// into an error whose content is the corrective `feedback`. Either decision may attach additional
    /// contexts. Context deferred by the tool body survives an accepted result but is discarded when the
    /// outer call is blocked.
    async fn post_execute(
        &self,
        exec: &Arc<ToolExecution>,
        result: ToolExecutionResult,
    ) -> Result<ToolExecutionResult, BoxError> {
        let event = ToolsPostExecute {
            exec: exec.clone(),
            result: Arc::new(result.clone()),
        };
        let decision = self
            .carrier(&exec.agent)
            .context(&self.0.ctx)
            .waterfall::<ToolsPostExecute, PostToolDecision, _>(event, |_| async {
                Ok(PostToolDecision::accept())
            })
            .await?;
        match decision {
            PostToolDecision::Block {
                feedback,
                info,
                additional_contexts,
            } => {
                let message = failure_message_from_content(&feedback);
                let mut failure =
                    ToolExecutionFailure::new(ToolFailure { message, info }, feedback);
                failure.set_additional_contexts(
                    additional_contexts.filter(|contexts| !contexts.is_empty()),
                );
                Ok(ToolExecutionResult::Failure(failure).marked_editable(exec))
            }
            PostToolDecision::Accept {
                content,
                value,
                additional_contexts,
            } => {
                if content.is_some() && value.is_some() {
                    return Err(type_error(
                        "tools/post-execute accept decision cannot replace both value and content",
                    ));
                }
                let mut contexts = result.additional_contexts().cloned().unwrap_or_default();
                contexts.extend(additional_contexts.unwrap_or_default());
                let contexts = (!contexts.is_empty()).then_some(contexts);
                if let Some(value) = value {
                    if result.is_error() {
                        return Err(type_error(
                            "tools/post-execute cannot replace the value of a failed result",
                        ));
                    }
                    let Some(tool) = self.resolve_for(exec)? else {
                        return Err(Box::new(ToolNotFoundError::new(&exec.name, None)));
                    };
                    let replaced = self.create_success_result(exec, &tool, value)?;
                    let replaced = match contexts {
                        Some(contexts) => with_additional_contexts(replaced, Some(contexts)),
                        None => replaced,
                    };
                    return Ok(replaced.marked_editable(exec));
                }
                let result = match content {
                    Some(content) => with_content(result, content),
                    None => result,
                };
                let result = match contexts {
                    Some(contexts) => with_additional_contexts(result, Some(contexts)),
                    None => result,
                };
                Ok(result.marked_editable(exec))
            }
        }
    }

    /// Snapshot, validate, render, and optionally project one successful body value.
    fn create_success_result(
        &self,
        exec: &ToolExecution,
        tool: &ToolDefinition,
        candidate: Value,
    ) -> Result<ToolExecutionResult, BoxError> {
        let value = snapshot_tool_value(&tool.name, &candidate)?;
        // A forged schema makes the validator throw in the source; the failure is normalized like any
        // other body failure, so post-execute still runs.
        let violations = catch_panic(|| {
            crate::json_schema::validate_json_schema_value(&tool.output.schema, &value, "value")
        })?;
        if !violations.is_empty() {
            return Err(Box::new(ToolOutputError::new(&tool.name, violations)));
        }
        let rendered = catch_panic(|| (tool.output.render)(&exec.arguments, &value))
            .and_then(|rendered| rendered)
            .map_err(|error| projection_error(&tool.name, "render", &error))?;
        let content = snapshot_projection(&tool.name, "render", &rendered)?;
        let meta = match &tool.output.presentation_meta {
            None => None,
            Some(project) => {
                let projected = catch_panic(|| project(&exec.arguments, &value))
                    .and_then(|projected| projected)
                    .map_err(|error| projection_error(&tool.name, "presentationMeta", &error))?;
                if !is_json_value(&projected) {
                    return Err(Box::new(ToolOutputError::new(
                        &tool.name,
                        vec!["output.presentationMeta returned non-lossless JSON".into()],
                    )));
                }
                Some(projected)
            }
        };
        let mut success = ToolExecutionSuccess::new(value, content);
        success.set_meta(meta);
        success.set_concludes_turn(exec.concludes());
        Ok(materialize_final_result(ToolExecutionResult::Success(success))?.marked(exec))
    }

    /// Normalize an around-dispatch wrapper's authored result through the owning output contract.
    fn normalize_dispatch_result(
        &self,
        exec: &ToolExecution,
        result: ToolExecutionResult,
    ) -> Result<ToolExecutionResult, BoxError> {
        // A canonical result of this dispatch is frozen: no wrapper could edit it in place, so what the
        // wrapper returns is the original.
        if result.seal().is_some_and(|seal| seal.token == exec.token) {
            return Ok(result);
        }
        match result {
            ToolExecutionResult::Failure(failure) => {
                // A copy loses any mark of another dispatch: it is a new object.
                Ok(ToolExecutionResult::Failure(failure.clone()).marked_editable(exec))
            }
            ToolExecutionResult::Success(success) => {
                let Some(tool) = self.resolve_for(exec)? else {
                    return Err(Box::new(ToolNotFoundError::new(&exec.name, None)));
                };
                let normalized = self.create_success_result(exec, &tool, success.value.clone())?;
                Ok(match success.additional_contexts.clone() {
                    Some(contexts) => with_additional_contexts(normalized, Some(contexts)),
                    None => normalized,
                }
                .marked_editable(exec))
            }
        }
    }
}

fn lock_take<T>(mutex: &std::sync::Mutex<Option<T>>) -> Option<T> {
    crate::contracts::lock(mutex).take()
}

fn type_error(message: &str) -> BoxError {
    Box::new(ToolsError::TypeError(message.into()))
}

/// Replace the content of a result, keeping every other field.
fn with_content(result: ToolExecutionResult, content: Vec<ContentBlock>) -> ToolExecutionResult {
    let edited = match result {
        ToolExecutionResult::Success(mut success) => {
            success.content = content;
            ToolExecutionResult::Success(success)
        }
        ToolExecutionResult::Failure(mut failure) => {
            failure.content = content;
            ToolExecutionResult::Failure(failure)
        }
    };
    edited.unsealed()
}

fn with_additional_contexts(
    result: ToolExecutionResult,
    contexts: Option<Vec<UserMessage>>,
) -> ToolExecutionResult {
    let edited = match result {
        ToolExecutionResult::Success(mut success) => {
            success.additional_contexts = contexts;
            ToolExecutionResult::Success(success)
        }
        ToolExecutionResult::Failure(mut failure) => {
            failure.additional_contexts = contexts;
            ToolExecutionResult::Failure(failure)
        }
    };
    edited.unsealed()
}

/// Convert one projector failure into the canonical invalid-output failure.
fn projection_error(tool_name: &str, projector: &str, error: &BoxError) -> BoxError {
    Box::new(ToolOutputError::new(
        tool_name,
        vec![format!(
            "output.{projector} failed: {}",
            tool_error_message(error)
        )],
    ))
}

/// Snapshot one projector result before later durable-result materialization.
fn snapshot_projection(
    tool_name: &str,
    projector: &str,
    content: &[ContentBlock],
) -> Result<Vec<ContentBlock>, BoxError> {
    if blocks_lossless(content) {
        Ok(content.to_vec())
    } else {
        Err(Box::new(ToolOutputError::new(
            tool_name,
            vec![format!("output.{projector} returned non-lossless JSON")],
        )))
    }
}

/// Snapshot one body or policy value into the canonical invalid-output failure class.
fn snapshot_tool_value(tool_name: &str, candidate: &Value) -> Result<Value, BoxError> {
    snapshot_json_value(candidate).ok_or_else(|| {
        Box::new(ToolOutputError::new(
            tool_name,
            vec!["value is not lossless JSON".into()],
        )) as BoxError
    })
}

fn blocks_lossless(blocks: &[ContentBlock]) -> bool {
    blocks.iter().all(|block| {
        let data = Value::Object(Map::from_iter(
            block.data.iter().map(|(k, v)| (k.clone(), v.clone())),
        ));
        is_json_value(&data)
    })
}

/// The complete serialized message (content, source, id) must survive JSON without loss.
fn message_lossless(message: &UserMessage) -> bool {
    serde_json::to_value(message)
        .map(|value| is_json_value(&value))
        .unwrap_or(false)
}

/// Best-effort human-readable message from a thrown value.
///
/// Error normalization is the outermost safety boundary, so it is total: a `Display` that panics
/// (the hostile thrown value of the source) yields the same placeholder as the source.
pub fn tool_error_message(error: &BoxError) -> String {
    catch_unwind(AssertUnwindSafe(|| error.to_string()))
        .unwrap_or_else(|_| "<unprintable thrown value>".into())
}

/// Derive one failure message from policy feedback without changing its rendered blocks.
fn failure_message_from_content(content: &[ContentBlock]) -> String {
    let text = content
        .iter()
        .map(|block| match block.text_value() {
            Some(text) if block.r#type == "text" => text.to_owned(),
            _ => format!("[{} content]", block.r#type),
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.encode_utf16().count() > 0 {
        text
    } else {
        "tool result blocked by post-execute policy".into()
    }
}

/// Validate one durable tool-result projection or reject lossy data.
fn materialize_final_result(result: ToolExecutionResult) -> Result<ToolExecutionResult, BoxError> {
    let lossless = blocks_lossless(result.content())
        && result.meta().is_none_or(is_json_value)
        && result
            .additional_contexts()
            .is_none_or(|contexts| contexts.iter().all(message_lossless));
    if lossless {
        Ok(result)
    } else {
        Err(type_error(
            "tool result must be losslessly JSON-serializable",
        ))
    }
}

/// The fallback of the source's nested `try`: an error result is always lossless.
fn materialize_or_fallback(result: ToolExecutionResult) -> ToolExecutionResult {
    materialize_final_result(result.clone()).unwrap_or(result)
}

/// Structured `{ name, code }` for a thrown HarnessError, else `None`.
fn error_info(error: &BoxError) -> Option<ToolErrorInfo> {
    HarnessError::of(error.as_ref()).map(|error| ToolErrorInfo::new(error.name(), error.code()))
}

/// Materialize a non-granting approval outcome as a denial with routable `ApprovalError` identity.
fn approval_denial(reason: String, code: &str) -> PreToolDecision {
    PreToolDecision::Deny {
        reason,
        info: Some(ToolErrorInfo::new(APPROVAL_ERROR_NAME, code)),
    }
}

/// Whether a value thrown after cancellation is that cancellation and carries no identity of its own.
fn is_unclassified_cancellation(error: &BoxError, signal: &AbortSignal) -> bool {
    if error_info(error).is_some() {
        return false;
    }
    let is_reason = hoh_error::SharedError::of(error.as_ref())
        .zip(signal.reason())
        .is_some_and(|(thrown, reason)| thrown.ptr_eq(&reason));
    is_reason || is_abort_error(error.as_ref())
}

fn tool_error_result(error: &BoxError) -> ToolExecutionResult {
    tool_error_result_with(error, error_info(error))
}

fn tool_error_result_with(error: &BoxError, info: Option<ToolErrorInfo>) -> ToolExecutionResult {
    let message = tool_error_message(error);
    ToolExecutionResult::Failure(ToolExecutionFailure::new(
        ToolFailure {
            message: message.clone(),
            info,
        },
        vec![ContentBlock::text(format!("Error: {message}"))],
    ))
}

/// Canonical result when cancellation supersedes success after body invocation.
pub(crate) fn tool_aborted_result(prior: Option<&ToolExecutionResult>) -> ToolExecutionResult {
    aborted(
        "tool call aborted",
        ToolErrorInfo::new("AbortError", TOOL_ABORTED),
        prior,
    )
}

/// Canonical result when cancellation prevents tool body invocation.
pub(crate) fn tool_aborted_before_dispatch_result(
    prior: Option<&ToolExecutionResult>,
) -> ToolExecutionResult {
    aborted(
        "tool call aborted before dispatch",
        ToolErrorInfo::new("AbortError", TOOL_ABORTED_BEFORE_DISPATCH),
        prior,
    )
}

fn aborted(
    message: &str,
    info: ToolErrorInfo,
    prior: Option<&ToolExecutionResult>,
) -> ToolExecutionResult {
    let mut failure = ToolExecutionFailure::new(
        ToolFailure {
            message: message.into(),
            info: Some(info),
        },
        vec![ContentBlock::text(format!("Error: {message}"))],
    );
    failure.set_additional_contexts(
        prior
            .and_then(|prior| prior.additional_contexts())
            .filter(|contexts| !contexts.is_empty())
            .cloned(),
    );
    ToolExecutionResult::Failure(failure)
}
