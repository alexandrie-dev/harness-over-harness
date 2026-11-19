mod support;
use hoh_commander::{Command, Output, ParseError};
use hoh_kernel::{Cfg, Context, Error, Kernel, NoConfig, Plugin, Service, ServiceKey};
use hoh_loader::cmdline::*;
use hoh_loader::{BootOptions, PluginRegistry, boot, parse_entries};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use support::*;
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap()
}
#[derive(Default)]
struct Seen {
    started: Mutex<Option<Value>>,
    exits: Mutex<Vec<i32>>,
    out: Mutex<String>,
}
fn output(seen: &Arc<Seen>) -> Output {
    let a = seen.clone();
    let b = seen.clone();
    Output::new(
        move |s| lock(&a.out).push_str(s),
        move |s| lock(&b.out).push_str(s),
    )
}
struct DemoKey;
impl ServiceKey for DemoKey {
    type Value = Value;
    const NAME: &'static str = "demoStartup";
    fn view(v: &Value) -> Option<Value> {
        Some(v.clone())
    }
}
#[derive(hoh_kernel::Inject)]
struct DemoDeps {
    _args: Service<CmdlineArgsKey>,
}
struct Demo;
fn demo_command() -> Command {
    let mut p = Command::new();
    p.name("demo")
        .exit_override()
        .option("--port <port>", "listen port");
    p
}
fn demo_value(options: &hoh_commander::Options, p: &Command) -> Result<Value, ParseError> {
    let Some(port) = options.get("port").and_then(Value::as_str) else {
        return Ok(json!({}));
    };
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return Err(p
            .error(&format!(
                "error: --port must be a number, got {}",
                hoh_util_values::js_json_stringify(&json!(port))
            ))
            .into());
    }
    Ok(json!({"port":port.parse::<u64>().map_err(|_|p.error("port out of range"))?}))
}
impl Plugin for Demo {
    const NAME: &'static str = "demo-startup";
    type Config = NoConfig;
    type Deps = DemoDeps;
    async fn apply(ctx: Context, _: DemoDeps, _: Cfg<NoConfig>) -> Result<(), Error> {
        let seen = ctx.tag::<Seen>().unwrap();
        let value = Arc::new(Mutex::new(None));
        let result = value.clone();
        let mut p = demo_command();
        p.action(move |_, options, p| {
            *lock(&result) = Some(demo_value(options, p)?);
            Ok(())
        });
        parse_cmdline(&ctx, &mut p, &output(&seen))?;
        let next = lock(&value).take();
        if let Some(next) = next {
            ctx.provide::<DemoKey>(Arc::new(next)).await?;
        }
        Ok(())
    }
}
#[derive(hoh_kernel::Inject)]
struct ReaderDeps {
    _demo: Service<DemoKey>,
}
struct Reader;
impl Plugin for Reader {
    const NAME: &'static str = "reader";
    type Config = Value;
    type Deps = ReaderDeps;
    async fn apply(ctx: Context, _: ReaderDeps, config: Cfg<Value>) -> Result<(), Error> {
        *lock(&ctx.tag::<Seen>().unwrap().started) = Some((*config.get()).clone());
        Ok(())
    }
}
struct Fixture {
    kernel: Kernel,
    ctx: Context,
    seen: Arc<Seen>,
    _dir: tempfile::TempDir,
}
async fn fixture(args: &[&str], without_provider: bool, object_inject: bool) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("cordis.yml");
    let mut text = if without_provider {
        String::new()
    } else {
        "- id: demo-startup\n  name: demo-startup\n".into()
    };
    text.push_str(&format!("- id: reader\n  name: reader\n  inject: {}\n  config:\n    port: !!js ctx.demoStartup.port ?? 3080\n",if object_inject{"{demoStartup: {required: true}}"}else{"[demoStartup]"}));
    parse_entries(&text, &base).unwrap();
    std::fs::write(&base, text).unwrap();
    let kernel = Kernel::start();
    let ctx = kernel.root().with_tag(Seen::default());
    let seen = ctx.tag::<Seen>().unwrap();
    let captured = seen.clone();
    provide_cmdline(
        &ctx,
        CmdlineHost {
            args: args.iter().map(|s| (*s).into()).collect(),
            exit: AppExit::new(move |code| lock(&captured.exits).push(code)),
            ready: None,
        },
    )
    .await
    .unwrap();
    let mut registry = PluginRegistry::default();
    registry.add::<Demo>("demo-startup").add::<Reader>("reader");
    boot(
        &ctx,
        BootOptions {
            base,
            registry: Arc::new(registry),
            layers: vec![],
            env: env(dir.path()),
        },
    )
    .await
    .unwrap();
    Fixture {
        kernel,
        ctx,
        seen,
        _dir: dir,
    }
}
#[tokio::test(start_paused = true)]
async fn lets_a_row_read_the_flag_value_the_app_resolved() {
    let f = fixture(&["--port", "8080"], false, false).await;
    assert_eq!(*lock(&f.seen.started), Some(json!({"port":8080})));
    assert!(lock(&f.seen.exits).is_empty());
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn leaves_a_row_on_the_value_written_beside_the_expression_when_no_flag_names_one() {
    let f = fixture(&[], false, false).await;
    assert_eq!(*lock(&f.seen.started), Some(json!({"port":3080})));
    f.kernel.shutdown().await;
}
#[test]
fn binding_l4_rejects_the_source_object_form_of_a_provider_service_injection() {
    let error = parse_entries(
        "- id: reader\n  name: reader\n  inject: {demoStartup: {required: true}}\n",
        std::path::Path::new("/injected.yml"),
    )
    .unwrap_err();
    assert!(matches!(error, hoh_loader::LoadError::Shape { .. }));
    assert!(error.to_string().contains("sequence"));
}
#[tokio::test(start_paused = true)]
async fn prints_the_app_help_starts_no_reading_row_and_requests_exit_0() {
    let f = fixture(&["--help"], false, false).await;
    assert!(lock(&f.seen.out).contains("Usage: demo"));
    assert!(lock(&f.seen.started).is_none());
    assert_eq!(*lock(&f.seen.exits), [0]);
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rejects_the_invocation_from_the_action_without_starting_the_app() {
    let f = fixture(&["--port", "abc"], false, false).await;
    assert!(lock(&f.seen.out).contains("--port must be a number"));
    assert!(lock(&f.seen.started).is_none());
    assert_eq!(*lock(&f.seen.exits), [1]);
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rethrows_an_action_failure_that_is_not_commander_asking_to_exit() {
    let f = fixture(&[], true, false).await;
    let mut p = demo_command();
    p.action(|_, _, _| Err(ParseError::other(std::io::Error::other("action exploded"))));
    assert_eq!(
        parse_cmdline(&f.ctx, &mut p, &output(&f.seen))
            .unwrap_err()
            .to_string(),
        "action exploded"
    );
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn rethrows_a_thrown_value_that_is_not_an_object_at_all() {
    let f = fixture(&[], true, false).await;
    let mut p = demo_command();
    p.action(|_, _, _| Err(ParseError::Other("action threw a string".into())));
    assert_eq!(
        parse_cmdline(&f.ctx, &mut p, &output(&f.seen))
            .unwrap_err()
            .to_string(),
        "action threw a string"
    );
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn runs_the_action_without_inspecting_loader_rows_or_owning_a_service() {
    let f = fixture(&[], true, false).await;
    let value = Arc::new(Mutex::new(None));
    let next = value.clone();
    let mut p = demo_command();
    p.action(move |_, options, p| {
        *lock(&next) = Some(demo_value(options, p)?);
        Ok(())
    });
    parse_cmdline(&f.ctx, &mut p, &output(&f.seen)).unwrap();
    assert_eq!(*lock(&value), Some(json!({})));
    assert!(f.ctx.get::<DemoKey>().is_none());
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn hands_the_app_a_snapshot_the_caller_cannot_mutate_afterwards() {
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let mut args = vec!["--resume".into(), "abc".into()];
    provide_cmdline(
        &ctx,
        CmdlineHost {
            args: args.clone(),
            exit: AppExit::new(|_| {}),
            ready: None,
        },
    )
    .await
    .unwrap();
    args.push("--tampered".into());
    assert_eq!(
        ctx.get::<CmdlineArgsKey>().unwrap().get(),
        ["--resume", "abc"]
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn refuses_at_load_a_program_in_which_no_command_declares_an_action() {
    let f = fixture(&[], true, false).await;
    assert!(
        parse_cmdline(&f.ctx, &mut demo_command(), &output(&f.seen))
            .unwrap_err()
            .to_string()
            .contains("no command in the program declares an action")
    );
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn routes_a_pre_registered_subcommand_rejection_through_the_launcher_exit_request() {
    let f = fixture(&["serve"], true, false).await;
    let mut p = Command::new();
    p.name("demo")
        .command("serve")
        .action(|_, _, p| Err(p.error("error: serve rejected").into()));
    parse_cmdline(&f.ctx, &mut p, &output(&f.seen)).unwrap();
    assert!(lock(&f.seen.out).contains("serve rejected"));
    assert_eq!(*lock(&f.seen.exits), [1]);
    f.kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn fails_loud_when_a_parser_runs_without_the_launcher_values() {
    let k = Kernel::start();
    let s = Arc::new(Seen::default());
    assert_eq!(
        parse_cmdline(&k.root(), &mut demo_command(), &output(&s))
            .unwrap_err()
            .to_string(),
        "demo: the launcher must provide ctx.cmdlineArgs and ctx.appExit before the tree mounts"
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn lets_multiple_parsers_read_the_same_immutable_snapshot() {
    let f = fixture(&["--port", "8080"], true, false).await;
    let args = f.ctx.get::<CmdlineArgsKey>().unwrap();
    let address = args.get().as_ptr();
    for _ in 0..2 {
        let value = Arc::new(Mutex::new(None));
        let next = value.clone();
        let mut p = demo_command();
        p.action(move |_, options, p| {
            *lock(&next) = Some(demo_value(options, p)?);
            Ok(())
        });
        parse_cmdline(&f.ctx, &mut p, &output(&f.seen)).unwrap();
        assert_eq!(*lock(&value), Some(json!({"port":8080})));
        assert_eq!(args.get().as_ptr(), address);
    }
    f.kernel.shutdown().await;
}
#[derive(Default)]
struct StdinState {
    ended: bool,
    listeners: Vec<Listener>,
    buffer: Vec<u8>,
}
#[derive(Default)]
struct Stdin(Arc<Mutex<StdinState>>);
impl AppStdin for Stdin {
    fn readable_ended(&self) -> bool {
        lock(&self.0).ended
    }
    fn once_end(&self, listener: Listener) -> Subscription {
        lock(&self.0).listeners.push(listener.clone());
        let state = self.0.clone();
        Subscription::new(move || {
            lock(&state)
                .listeners
                .retain(|l| !Arc::ptr_eq(l, &listener))
        })
    }
}
impl Stdin {
    fn end(&self) {
        let listeners = {
            let mut state = lock(&self.0);
            state.ended = true;
            std::mem::take(&mut state.listeners)
        };
        for listener in listeners {
            listener();
        }
    }
}
async fn eof_fixture(ready: Option<AppReady>) -> (Kernel, Arc<Stdin>, Arc<Mutex<Vec<i32>>>) {
    let k = Kernel::start();
    let s = Arc::new(Stdin::default());
    let exits = Arc::new(Mutex::new(vec![]));
    let e = exits.clone();
    provide_cmdline(
        &k.root(),
        CmdlineHost {
            args: vec![],
            exit: AppExit::new(move |c| lock(&e).push(c)),
            ready,
        },
    )
    .await
    .unwrap();
    (k, s, exits)
}
fn ready() -> AppReady {
    AppReady::new(|listener| {
        listener();
        Subscription::new(|| {})
    })
}
#[tokio::test(start_paused = true)]
async fn requests_bounded_exit_on_eof_and_removes_the_listener_on_disposal() {
    let (k, s, exits) = eof_fixture(Some(ready())).await;
    exit_on_stdin_end(&k.root(), "test.stdin", s.clone()).unwrap();
    s.end();
    assert_eq!(*lock(&exits), [0]);
    k.shutdown().await;
    assert!(lock(&s.0).listeners.is_empty());
    s.end();
    assert_eq!(*lock(&exits), [0]);
}
#[tokio::test(start_paused = true)]
async fn requests_exit_after_binding_to_stdin_that_has_already_ended() {
    let (k, s, exits) = eof_fixture(Some(ready())).await;
    s.end();
    exit_on_stdin_end(&k.root(), "test.stdin", s.clone()).unwrap();
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
    assert_eq!(*lock(&exits), [0]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn cancels_an_already_ended_stream_before_its_queued_eof_handler_runs() {
    let (k, s, exits) = eof_fixture(Some(ready())).await;
    s.end();
    let pending = Mutex::new(None);
    exit_on_stdin_end_with_queue(&k.root(), "test.stdin", s, &|callback| {
        *lock(&pending) = Some(callback);
        Ok(())
    })
    .unwrap();
    k.shutdown().await;
    lock(&pending).take().unwrap()();
    assert!(lock(&exits).is_empty());
}
#[tokio::test(start_paused = true)]
async fn leaves_protocol_bytes_buffered_until_the_transport_claims_stdin() {
    let (k, s, exits) = eof_fixture(Some(ready())).await;
    exit_on_stdin_end(&k.root(), "test.stdin", s.clone()).unwrap();
    let frame = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\"}\n";
    lock(&s.0).buffer.extend_from_slice(frame);
    assert_eq!(lock(&s.0).buffer, frame);
    let received = std::mem::take(&mut lock(&s.0).buffer);
    s.end();
    assert_eq!(received, frame);
    assert_eq!(*lock(&exits), [0]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn waits_for_the_launcher_to_commit_successful_startup_after_eof() {
    let listeners = Arc::new(Mutex::new(vec![]));
    let pending = listeners.clone();
    let ready = AppReady::new(move |listener| {
        lock(&pending).push(listener.clone());
        let p = pending.clone();
        Subscription::new(move || lock(&p).retain(|l| !Arc::ptr_eq(l, &listener)))
    });
    let (k, s, exits) = eof_fixture(Some(ready)).await;
    exit_on_stdin_end(&k.root(), "test.stdin", s.clone()).unwrap();
    s.end();
    assert!(lock(&exits).is_empty());
    let callbacks = std::mem::take(&mut *lock(&listeners));
    for callback in callbacks {
        callback();
    }
    assert_eq!(*lock(&exits), [0]);
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn fails_loud_without_a_launcher_exit_request() {
    let k = Kernel::start();
    let error = exit_on_stdin_end(&k.root(), "test.stdin", Arc::new(Stdin::default())).unwrap_err();
    assert_eq!(
        error.to_string(),
        "stdio app: the launcher must provide ctx.appExit and ctx.appReady before the tree mounts"
    );
    k.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn fails_loud_without_launcher_startup_readiness() {
    let (k, s, _) = eof_fixture(None).await;
    assert_eq!(
        exit_on_stdin_end(&k.root(), "test.stdin", s)
            .unwrap_err()
            .to_string(),
        "stdio app: the launcher must provide ctx.appExit and ctx.appReady before the tree mounts"
    );
    k.shutdown().await;
}
