//! Process-local assistant attempt framing and durable stream accumulation.

use hoh_agent::{AssistantStreamFrame, AssistantStreamOutcome};
use hoh_error::BoxError;
use hoh_llm::{
    AssistantStreamAccumulator, AssistantStreamRecord, BlockAssembler, ContentBlock, FinishReason,
    LlmAttemptId, ReplayEnvelope, StreamChunk, TimedStreamChunk, TokenUsage,
};
use hoh_session::{SessionId, SessionSeq};

/// Publisher of agent-scoped assistant-stream frames.
pub type EmitFrame = Box<dyn Fn(AssistantStreamFrame) + Send + Sync>;
/// Allocator of the next emitted frame revision.
pub type NextRevision = Box<dyn FnMut() -> u64 + Send>;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Folds one model attempt into one compact stream plus ordered transient frames.
pub struct AssistantStreamAttempt {
    accumulator: AssistantStreamAccumulator,
    assembler: BlockAssembler,
    index: u64,
    terminal: bool,
    /// Attempt identity unique within this Agent lifecycle.
    pub attempt_id: LlmAttemptId,
    next_revision: NextRevision,
    pub turn: u64,
    pub step: u64,
    emit: EmitFrame,
}

impl AssistantStreamAttempt {
    /// * `session_id` — identity embedded only in the Agent-lifecycle-local attempt id.
    /// * `attempt` — attached-Session-local attempt counter.
    pub fn new(
        session_id: &SessionId,
        attempt: u64,
        next_revision: NextRevision,
        turn: u64,
        step: u64,
        emit: EmitFrame,
    ) -> Self {
        Self {
            accumulator: AssistantStreamAccumulator::new(),
            assembler: BlockAssembler::new(),
            index: 0,
            terminal: false,
            attempt_id: LlmAttemptId::from(format!("{session_id}:{attempt}")),
            next_revision,
            turn,
            step,
            emit,
        }
    }

    /// Whether this started attempt has emitted its terminal frame.
    pub fn ended(&self) -> bool {
        self.terminal
    }

    /// Publish the opening marker before the first delivered chunk.
    pub fn start(&mut self) {
        let revision = (self.next_revision)();
        (self.emit)(AssistantStreamFrame::Start {
            attempt_id: self.attempt_id.clone(),
            revision,
            turn: self.turn,
            step: self.step,
        });
    }

    /// Snapshot one chunk once, then feed durable compaction, assembly, and live publication.
    pub fn push(&mut self, chunk: StreamChunk) -> Result<(), BoxError> {
        let timed = self.accumulator.push(TimedStreamChunk {
            time: now_ms(),
            chunk,
        })?;
        self.assembler.push(timed.chunk.clone());
        let revision = (self.next_revision)();
        let index = self.index;
        self.index += 1;
        (self.emit)(AssistantStreamFrame::Chunk {
            attempt_id: self.attempt_id.clone(),
            revision,
            index,
            time: timed.time,
            chunk: timed.chunk,
        });
        Ok(())
    }

    /// Publish terminal settlement after the matching durable event commits. `append` is the
    /// synchronous durable append returning its committed seq; a failure publishes abandonment.
    pub fn settle(
        &mut self,
        event_type: &'static str,
        append: impl FnOnce() -> Result<SessionSeq, BoxError>,
    ) -> Result<(), BoxError> {
        let seq = match append() {
            Ok(seq) => seq,
            Err(error) => {
                self.abandon();
                return Err(error);
            }
        };
        self.terminal = true;
        let revision = (self.next_revision)();
        (self.emit)(AssistantStreamFrame::End {
            attempt_id: self.attempt_id.clone(),
            revision,
            index: self.index,
            outcome: AssistantStreamOutcome::Committed { event_type, seq },
        });
        Ok(())
    }

    /// Commit the durable interrupted prefix, then mark the live generation attempt abandoned.
    /// Cancellation preserves visible text in history without claiming that the model completed.
    pub fn settle_abandoned(
        &mut self,
        append: impl FnOnce() -> Result<SessionSeq, BoxError>,
    ) -> Result<(), BoxError> {
        if let Err(error) = append() {
            self.abandon();
            return Err(error);
        }
        self.abandon();
        Ok(())
    }

    /// Publish that generation did not complete, whether or not a partial prefix was persisted.
    pub fn abandon(&mut self) {
        self.terminal = true;
        let revision = (self.next_revision)();
        (self.emit)(AssistantStreamFrame::End {
            attempt_id: self.attempt_id.clone(),
            revision,
            index: self.index,
            outcome: AssistantStreamOutcome::Abandoned,
        });
    }

    /// Exact compact stream for the final durable event.
    pub fn stream(&self) -> Vec<AssistantStreamRecord> {
        self.accumulator.snapshot()
    }

    /// Canonical completed-message blocks from the same chunks.
    pub fn blocks(&self) -> Result<Vec<ContentBlock>, BoxError> {
        Ok(self.assembler.blocks()?)
    }

    /// Safe visible prefix when cancellation interrupts the attempt.
    pub fn interrupted_blocks(&self) -> Vec<ContentBlock> {
        self.assembler.interrupted_blocks()
    }

    /// Latest adapter-reported usage in the stream.
    pub fn usage(&self) -> Option<&TokenUsage> {
        self.assembler.usage()
    }

    /// Terminal reason, defaulting to stop when the stream omitted one.
    pub fn finish(&self) -> FinishReason {
        self.assembler.finish()
    }

    /// Replay state carried by the terminal finish record.
    pub fn replay_state(&self) -> Result<Option<ReplayEnvelope>, BoxError> {
        Ok(self.assembler.replay_state()?)
    }
}
