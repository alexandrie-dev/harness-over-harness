//! A restart (or a stop) gives the engine time to write what it keeps before it is killed.
use hoh_desktop::engine::{Connector, Engine, HostEvent, Message, Transport};
use hoh_protocol::{framing, transport::classify};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

/// Whether the engine had finished writing when it was killed (none: not killed yet).
type WrittenAtKill = Arc<Mutex<Option<bool>>>;

/// Polls of `exited` after `shutdown` before the stand-in engine is done writing: far more than
/// the old 500 ms grace allows (the host polls every 10 ms), so the old code killed it first.
const POLLS_TO_FINISH_WRITING: u32 = 150;

struct Slow {
    inbox: mpsc::Sender<Message>,
    generation: u64,
    shutdown_seen: bool,
    polls: u32,
    written: Arc<AtomicBool>,
    written_at_kill: WrittenAtKill,
}
impl Transport for Slow {
    fn send(&mut self, frame: Value) -> Result<(), String> {
        let result = if frame["method"] == "initialize" {
            json!({"serverInfo":{"name":"deepseek-harness-sdk-runtime","version":"test"},"hoh":{"protocolVersion":1,"approvals":"host"}})
        } else {
            if frame["method"] == "shutdown" {
                self.shutdown_seen = true;
            }
            json!({})
        };
        let _ = self.inbox.send(Message::Incoming(
            self.generation,
            classify(framing::success(frame["id"].clone(), result)).unwrap(),
        ));
        Ok(())
    }
    fn exited(&mut self) -> bool {
        if !self.shutdown_seen {
            return false;
        }
        self.polls += 1;
        if self.polls >= POLLS_TO_FINISH_WRITING {
            self.written.store(true, Ordering::SeqCst);
        }
        self.written.load(Ordering::SeqCst)
    }
    fn close(&mut self) {
        let mut at_kill = self.written_at_kill.lock().unwrap();
        at_kill.get_or_insert(self.written.load(Ordering::SeqCst));
    }
}
struct SlowConnector {
    kills: Arc<Mutex<Vec<WrittenAtKill>>>,
}
impl Connector for SlowConnector {
    fn connect(
        &self,
        generation: u64,
        inbox: mpsc::Sender<Message>,
    ) -> Result<Box<dyn Transport>, String> {
        let written_at_kill = Arc::new(Mutex::new(None));
        self.kills.lock().unwrap().push(written_at_kill.clone());
        Ok(Box::new(Slow {
            inbox,
            generation,
            shutdown_seen: false,
            polls: 0,
            written: Arc::new(AtomicBool::new(false)),
            written_at_kill,
        }))
    }
}

fn ready(events: &mpsc::Receiver<HostEvent>, count: usize) {
    let mut seen = 0;
    while seen < count {
        if matches!(events.recv_timeout(Duration::from_secs(30)).unwrap(),
            HostEvent::Status(value) if value["state"] == "ready")
        {
            seen += 1;
        }
    }
}

#[test]
fn a_restart_waits_for_the_engine_to_finish_writing_before_it_kills_it() {
    let kills = Arc::new(Mutex::new(Vec::new()));
    let (events, receiver) = mpsc::channel();
    let events = Mutex::new(events);
    let engine = Engine::start(
        Arc::new(SlowConnector {
            kills: kills.clone(),
        }),
        json!({"cwd":"/","provider":"p","model":"m"}),
        Arc::new(move |event| {
            let _ = events.lock().unwrap().send(event);
        }),
    );
    ready(&receiver, 1);
    engine.restart_in(std::path::Path::new("/")).unwrap();
    ready(&receiver, 1);
    let first = kills.lock().unwrap()[0].clone();
    assert_eq!(
        *first.lock().unwrap(),
        Some(true),
        "the engine was killed before it finished writing"
    );
    engine.stop();
}

#[test]
fn a_stop_waits_for_the_engine_to_finish_writing_before_it_kills_it() {
    let kills = Arc::new(Mutex::new(Vec::new()));
    let (events, receiver) = mpsc::channel();
    let events = Mutex::new(events);
    let engine = Engine::start(
        Arc::new(SlowConnector {
            kills: kills.clone(),
        }),
        json!({"cwd":"/","provider":"p","model":"m"}),
        Arc::new(move |event| {
            let _ = events.lock().unwrap().send(event);
        }),
    );
    ready(&receiver, 1);
    engine.stop();
    let first = kills.lock().unwrap()[0].clone();
    assert_eq!(*first.lock().unwrap(), Some(true));
}
