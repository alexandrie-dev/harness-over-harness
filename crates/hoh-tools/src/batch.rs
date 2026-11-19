//! Schedules one assistant step's tool calls. Exclusive calls form barriers; parallel calls use a
//! bounded rolling pool and are reclassified before start. Dispatch may overlap, while policy,
//! results, and result context remain model-ordered. Abort or an internal scheduler failure stops
//! replenishment and drains started calls.
//!
//! Abort records synthetic error results for skipped calls so replay stays valid. A terminal
//! scheduler failure rejects after draining; the owning step records conservative recovery results
//! before closing.

use std::{
    collections::BTreeSet,
    sync::{Arc, Mutex},
};

use futures_util::{StreamExt, stream::FuturesUnordered};
use hoh_error::BoxError;
use hoh_llm::ToolResultMessageInput;
use hoh_llm::{ToolCallBlock, UserMessage, create_tool_result_message};
use hoh_session::{SessionSeq, SurfaceIntent, SurfaceOp};
use hoh_timeout::AbortSignal;
use hoh_util_values::js_json_parse;
use serde_json::{Map, Value};

use crate::contracts::{
    ToolExecution, ToolExecutionInput, ToolExecutionMode, ToolExecutionResult, lock,
};
use crate::errors::ToolsError;
use crate::executor::{
    ToolBatchExecutionError, ToolBatchInput, ToolBatchResult, ToolCallResult, ToolLogStep,
};
use crate::pipeline::{
    ScheduledToolDispatch, ScheduledToolPreparation, ToolExecutionPipeline,
    tool_aborted_before_dispatch_result,
};
use crate::shim::{Agent, BoxFuture};

/// One tool call after argument parsing, ready to schedule.
struct PlannedCall {
    block: ToolCallBlock,
    exec: ToolExecutionInput,
}

/// Settled dispatch awaiting model-order finalization.
struct Slot {
    exec: Arc<ToolExecution>,
    outcome: ScheduledToolDispatch,
}

/// One scheduler group outcome, including a drained cancellation.
struct GroupOutcome {
    consumed: usize,
    aborted: bool,
    /// Whether any committed result carried `concludes_turn`.
    concluded: bool,
}

/// Execute raw calls with ordered policy and results around overlapping tool bodies. Abort drains
/// started calls and synthesizes results for skipped calls. Internal failures drain before rejecting
/// with the already committed results and context.
///
/// `max_parallel` reads the executor's current validated limit when the input has no override.
/// A zero override fails with a `RangeError`.
pub async fn execute_tool_batch(
    input: ToolBatchInput,
    pipeline: &ToolExecutionPipeline,
    max_parallel: &(dyn Fn() -> u64 + Sync),
) -> Result<ToolBatchResult, BoxError> {
    let ToolBatchInput {
        calls,
        signal,
        max_parallel_tool_calls,
        agent,
        tool_set,
        log,
    } = input;
    if max_parallel_tool_calls.is_some_and(|limit| limit < 1) {
        return Err(Box::new(ToolsError::RangeError(
            "maxParallelToolCalls must be a positive integer".into(),
        )));
    }
    let mut settlement = BatchSettlement::new(agent.clone(), log);
    // Inputs are distinct because tools/execute wrappers may replace `exec.signal`.
    let planned: Vec<PlannedCall> = calls
        .into_iter()
        .map(|block| PlannedCall {
            exec: ToolExecutionInput {
                call_id: block.id.clone(),
                root_call_id: None,
                name: block.name.clone(),
                schema: None,
                arguments: parse_arguments(&block.arguments),
                agent: agent.clone(),
                tool_set: tool_set.clone(),
                parent: None,
                signal: signal.clone(),
            },
            block,
        })
        .collect();
    let outcome = run_batch(
        &planned,
        pipeline,
        &mut settlement,
        max_parallel_tool_calls,
        max_parallel,
        &signal,
    )
    .await;
    match outcome {
        Ok(concluded) => Ok(settlement.result(Some(concluded))),
        Err(error) => Err(Box::new(ToolBatchExecutionError::new(
            error,
            settlement.result(None),
        ))),
    }
}

async fn run_batch(
    planned: &[PlannedCall],
    pipeline: &ToolExecutionPipeline,
    settlement: &mut BatchSettlement,
    override_limit: Option<u64>,
    max_parallel: &(dyn Fn() -> u64 + Sync),
    signal: &AbortSignal,
) -> Result<bool, BoxError> {
    let mut next = 0;
    let mut concluded = false;
    while next < planned.len() {
        // Commit before classifying again so registry changes affect unstarted calls.
        let first = &planned[next];
        let mode = pipeline.execution_mode(&first.exec);
        let group = if mode == ToolExecutionMode::Parallel {
            &planned[next..]
        } else {
            &planned[next..=next]
        };
        let limit = override_limit.unwrap_or_else(max_parallel);
        let outcome = run_group(pipeline, settlement, limit, group, mode, signal).await?;
        next += outcome.consumed;
        concluded = concluded || outcome.concluded;
        if outcome.aborted {
            for call in &planned[next..] {
                settlement.skip(&call.block)?;
            }
            return Ok(concluded);
        }
    }
    Ok(concluded)
}

/// Parse model arguments, preserving invalid JSON as text and mapping empty input to `{}`.
fn parse_arguments(raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Object(Map::new());
    }
    // Preserve malformed model input for tool-owned argument diagnostics.
    js_json_parse(raw).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

#[derive(Default)]
struct Shared {
    slots: Vec<Option<Slot>>,
    /// Dispatches whose future finished and that the scheduler has not collected yet.
    done: BTreeSet<usize>,
}

/// Run one exclusive barrier or parallel pool. Later calls are reclassified before start; an
/// exclusive reclassification waits for the current pool to drain and remains for the caller's next
/// barrier. Results and contexts commit in model order. Abort stops starts, drains and commits started
/// calls, accepts their contexts into the owning batch, records results for skipped calls, and returns
/// an aborted outcome. Scheduler failure drains dispatches and rejects for the owning step to record
/// recovery results.
async fn run_group(
    pipeline: &ToolExecutionPipeline,
    settlement: &mut BatchSettlement,
    max_parallel_tool_calls: u64,
    group: &[PlannedCall],
    mode: ToolExecutionMode,
    signal: &AbortSignal,
) -> Result<GroupOutcome, BoxError> {
    let shared = Arc::new(Mutex::new(Shared {
        slots: group.iter().map(|_| None).collect(),
        done: BTreeSet::new(),
    }));
    // Started slots retain their `tool/call` seq so the result can cite it.
    let mut call_seqs: Vec<Option<SessionSeq>> = vec![None; group.len()];
    let mut next_to_start = 0;
    let mut committed = 0;
    let mut started = 0;
    let mut aborted = signal.aborted();
    let mut concluded = false;
    let mut in_flight: BTreeSet<usize> = BTreeSet::new();
    let mut futures: FuturesUnordered<BoxFuture<'static, ()>> = FuturesUnordered::new();

    // Await `$fut` while the started dispatches keep running, as their promises do in the source.
    macro_rules! drive {
        ($fut:expr) => {{
            let fut = $fut;
            tokio::pin!(fut);
            loop {
                tokio::select! {
                    biased;
                    out = &mut fut => break out,
                    Some(()) = futures.next(), if !futures.is_empty() => {}
                }
            }
        }};
    }

    // `committed` advances only across contiguous model-order slots.
    macro_rules! commit_ready {
        () => {{
            while committed < group.len() {
                let Some(slot) = lock(&shared).slots[committed].take() else {
                    break;
                };
                let result = drive!(pipeline.complete(&slot.exec, slot.outcome));
                settlement.commit(
                    &group[committed].block,
                    result.clone(),
                    call_seqs[committed],
                )?;
                concluded = concluded || result.concludes_turn();
                committed += 1;
            }
        }};
    }

    macro_rules! fill_pool {
        () => {{
            while !aborted
                && next_to_start < group.len()
                && (in_flight.len() as u64) < max_parallel_tool_calls
            {
                // Re-read later modes after ordered commits so registry changes can create a barrier.
                let next_call = &group[next_to_start];
                if next_to_start > 0
                    && mode == ToolExecutionMode::Parallel
                    && pipeline.execution_mode(&next_call.exec) != ToolExecutionMode::Parallel
                {
                    break;
                }
                let index = next_to_start;
                call_seqs[index] = settlement.start(&next_call.block)?;
                started += 1;
                let prepared = drive!(pipeline.try_prepare(next_call.exec.clone()))?;
                match prepared {
                    ScheduledToolPreparation::Dispatch(exec) => {
                        let pipeline = pipeline.clone();
                        let shared = shared.clone();
                        in_flight.insert(index);
                        let mut ack = exec.arm_started();
                        futures.push(Box::pin(async move {
                            let outcome = pipeline.dispatch(exec.clone()).await;
                            // A dispatch that settled without reaching its wrappers still acknowledges.
                            exec.signal_started();
                            let mut state = lock(&shared);
                            state.slots[index] = Some(Slot { exec, outcome });
                            state.done.insert(index);
                        }));
                        // The source's `dispatch` runs synchronously up to its first await: every
                        // around-wrapper of this call runs up to its first suspension before the next
                        // call is prepared. The dispatch acknowledges the end of that initial pass
                        // (the kernel's `waterfall_with_start`); the started dispatches keep running.
                        loop {
                            tokio::select! {
                                biased;
                                _ = &mut ack => break,
                                Some(()) = futures.next(), if !futures.is_empty() => {}
                            }
                        }
                    }
                    ScheduledToolPreparation::PostResult { exec, result } => {
                        lock(&shared).slots[index] = Some(Slot {
                            exec,
                            outcome: ScheduledToolDispatch::PostResult(result),
                        });
                    }
                    ScheduledToolPreparation::FinalResult { exec, result } => {
                        lock(&shared).slots[index] = Some(Slot {
                            exec,
                            outcome: ScheduledToolDispatch::FinalResult(result),
                        });
                    }
                }
                next_to_start += 1;
                commit_ready!();
                // Abort may arrive while pre-execute awaits.
                if signal.aborted() {
                    aborted = true;
                }
            }
        }};
    }

    // Ordered pre-execute may await; only dispatch/body overlaps. A scheduler failure stops new
    // dispatches and reaches the turn boundary after every already-started dispatch settles.
    let body: Result<(), BoxError> = async {
        fill_pool!();
        while !in_flight.is_empty() {
            let settled = loop {
                let ready = lock(&shared).done.iter().next().copied();
                if let Some(index) = ready {
                    break index;
                }
                if futures.next().await.is_none() {
                    return Err(Box::new(ToolsError::Error(
                        "tool-call scheduler: dispatch settled without a result".into(),
                    )) as BoxError);
                }
            };
            in_flight.remove(&settled);
            lock(&shared).done.remove(&settled);
            commit_ready!();
            // Abort may arrive while a tool or ordered commit awaits.
            if signal.aborted() {
                aborted = true;
            }
            fill_pool!();
        }
        Ok(())
    }
    .await;
    if let Err(error) = body {
        while futures.next().await.is_some() {}
        return Err(error);
    }

    if aborted {
        // Started calls and accepted context settle first; every remaining model call then receives
        // an ordered synthetic result before the turn aborts.
        for call in &group[started..] {
            settlement.skip(&call.block)?;
        }
        return Ok(GroupOutcome {
            consumed: group.len(),
            aborted: true,
            concluded,
        });
    }
    if committed != started {
        return Err(Box::new(ToolsError::Error(
            "tool-call scheduler: uncommitted settled calls".into(),
        )));
    }
    Ok(GroupOutcome {
        consumed: started,
        aborted: false,
        concluded,
    })
}

/// Collect committed results and optionally append their durable Session events.
struct BatchSettlement {
    agent: Option<Arc<dyn Agent>>,
    log: Option<ToolLogStep>,
    results: Vec<ToolCallResult>,
    contexts: Vec<UserMessage>,
}

impl BatchSettlement {
    fn new(agent: Option<Arc<dyn Agent>>, log: Option<ToolLogStep>) -> Self {
        Self {
            agent,
            log,
            results: Vec::new(),
            contexts: Vec::new(),
        }
    }

    fn logging(&self) -> Result<Option<(&Arc<dyn Agent>, ToolLogStep)>, BoxError> {
        let Some(log) = self.log else {
            return Ok(None);
        };
        // ToolBatchInput requires agent whenever log is provided.
        let agent = self.agent.as_ref().ok_or_else(|| {
            Box::new(ToolsError::TypeError(
                "tool batch log requires an agent".into(),
            )) as BoxError
        })?;
        Ok(Some((agent, log)))
    }

    fn start(&self, block: &ToolCallBlock) -> Result<Option<SessionSeq>, BoxError> {
        let Some((agent, log)) = self.logging()? else {
            return Ok(None);
        };
        let mut data = Map::new();
        data.insert("turn".into(), log.turn.into());
        data.insert("step".into(), log.step.into());
        data.insert("callId".into(), Value::String(block.id.to_string()));
        data.insert("name".into(), Value::String(block.name.clone()));
        data.insert("arguments".into(), Value::String(block.arguments.clone()));
        let event = agent
            .session()
            .append("tool/call", Value::Object(data), None)?;
        Ok(Some(event.seq()))
    }

    fn commit(
        &mut self,
        block: &ToolCallBlock,
        result: ToolExecutionResult,
        call_seq: Option<SessionSeq>,
    ) -> Result<(), BoxError> {
        if let Some((agent, log)) = self.logging()? {
            let message = create_tool_result_message(ToolResultMessageInput {
                call_id: block.id.clone(),
                content: result.content().to_vec(),
                is_error: result.is_error(),
            });
            let mut data = Map::new();
            data.insert("turn".into(), log.turn.into());
            data.insert("step".into(), log.step.into());
            data.insert("message".into(), serde_json::to_value(&message)?);
            if let Some(info) = result.error().and_then(|error| error.info.as_ref()) {
                data.insert("error".into(), serde_json::to_value(info)?);
            }
            if let Some(meta) = result.meta() {
                data.insert("meta".into(), meta.clone());
            }
            // `start` records a call seq whenever this batch logs results.
            let call_seq = call_seq.ok_or_else(|| {
                Box::new(ToolsError::Error("tool call seq is missing".into())) as BoxError
            })?;
            let intent = SurfaceIntent {
                surface_op: Some(SurfaceOp::append()),
                source_event_seqs: Some(vec![call_seq]),
            };
            agent
                .session()
                .append("tool/result", Value::Object(data), Some(intent))?;
        }
        self.contexts
            .extend(result.additional_contexts().cloned().unwrap_or_default());
        self.results.push(ToolCallResult {
            call_id: block.id.clone(),
            result,
        });
        Ok(())
    }

    fn skip(&mut self, block: &ToolCallBlock) -> Result<(), BoxError> {
        let seq = self.start(block)?;
        self.commit(block, tool_aborted_before_dispatch_result(None), seq)
    }

    /// The committed outcomes; `concluded` defaults to whether any committed result concludes the turn.
    fn result(&self, concluded: Option<bool>) -> ToolBatchResult {
        ToolBatchResult {
            results: self.results.clone(),
            additional_contexts: self.contexts.clone(),
            concluded: concluded.unwrap_or_else(|| {
                self.results
                    .iter()
                    .any(|outcome| outcome.result.concludes_turn())
            }),
        }
    }
}
