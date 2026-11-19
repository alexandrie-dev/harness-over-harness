//! SDK profile command-line acceptance and stdin-lifetime provider.

use hoh_commander::{Command, Output, ParseError};
use hoh_kernel::{Cfg, Context, Inject, Plugin, Service, ServiceKey};
use hoh_loader::cmdline::{
    AppStdin, CmdlineArgsKey, Subscription, exit_on_stdin_end, parse_cmdline,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// The unchanged profile patch exported by the source package.
pub const CORDIS_PATCH: &str = include_str!("../cordis.patch.yml");
/// Service the JSON-RPC server row waits for before claiming stdio.
pub const SDK_APP_STARTUP_SERVICE: &str = "sdkAppStartup";

fn default_profile() -> String {
    "sdk".into()
}

// SHIM(hoh-kernel): Schemastery defaults an explicit null as well as an absent value; replace this
// deserializer and the `schemars(with)` annotation when kernel configuration resolution applies
// property defaults to null (the same stance as `hoh-user-approval`'s policy).
fn profile_or_default<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_profile))
}

/// SDK stdio startup configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Profile name rendered in help and diagnostics.
    #[serde(default = "default_profile", deserialize_with = "profile_or_default")]
    #[schemars(with = "Option<String>")]
    pub profile: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profile: default_profile(),
        }
    }
}

/// Accepted SDK invocation published to dependent rows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SdkAppStartup {
    pub accepted: bool,
}

/// Typed key for [`SDK_APP_STARTUP_SERVICE`].
pub struct SdkAppStartupKey;

impl ServiceKey for SdkAppStartupKey {
    type Value = SdkAppStartup;
    const NAME: &'static str = SDK_APP_STARTUP_SERVICE;

    fn view(value: &Self::Value) -> Option<serde_json::Value> {
        Some(serde_json::json!({ "accepted": value.accepted }))
    }
}

/// Host-only adapters for observing stdin EOF and capturing command output.
///
/// This service is not serialized plugin configuration. The window/server glue can publish it
/// when both components share one non-consuming EOF observer.
pub struct SdkAppRuntimeHooks {
    stdin: Arc<dyn AppStdin>,
    output: Output,
}

impl SdkAppRuntimeHooks {
    pub fn new(stdin: Arc<dyn AppStdin>, output: Output) -> Self {
        Self { stdin, output }
    }
}

hoh_kernel::service!(pub SdkAppRuntimeKey: SdkAppRuntimeHooks = "sdkAppRuntimeHooks");

#[derive(Inject)]
pub struct Deps {
    _cmdline_args: Service<CmdlineArgsKey>,
}

/// The SDK profile's zero-option startup plugin.
pub struct SdkApp;

fn sdk_command(profile: &str) -> Command {
    let mut program = Command::new();
    program
        .name(format!("dsh --profile {profile}"))
        .description("Serve DeepSeek Harness SDK clients over stdio JSON-RPC.")
        .help_option("-h, --help", "show this help")
        .add_help_text(
            "after",
            format!(
                "\nExample:\n  dsh --profile {profile}     serve one SDK runtime until its client disconnects\n"
            ),
        );
    program
}

struct PendingStdin;

impl AppStdin for PendingStdin {
    fn readable_ended(&self) -> bool {
        false
    }

    fn once_end(&self, _listener: hoh_loader::cmdline::Listener) -> Subscription {
        // GLUE-SHIM(2b): the JSON-RPC owner must replace this with its non-consuming EOF signal.
        Subscription::new(|| {})
    }
}

impl Plugin for SdkApp {
    const NAME: &'static str = "sdk-app-startup";
    type Config = Config;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        _deps: Deps,
        config: Cfg<Config>,
    ) -> Result<(), hoh_kernel::Error> {
        let (stdin, output): (Arc<dyn AppStdin>, Output) = match ctx.get::<SdkAppRuntimeKey>() {
            Some(runtime) => (runtime.stdin.clone(), runtime.output.clone()),
            None => {
                // GLUE-SHIM(2b): mounting must not start a competing reader for protocol stdin.
                (Arc::new(PendingStdin), Output::default())
            }
        };
        let accepted = Arc::new(AtomicBool::new(false));
        let mark_accepted = accepted.clone();
        let action_ctx = ctx.clone();
        let mut program = sdk_command(&config.get().profile);
        program.action(move |_, _, _| {
            exit_on_stdin_end(&action_ctx, "sdk-app.stdin", stdin.clone())
                .map_err(ParseError::Other)?;
            mark_accepted.store(true, Ordering::Release);
            Ok(())
        });
        parse_cmdline(&ctx, &mut program, &output)?;
        if accepted.load(Ordering::Acquire) {
            ctx.provide::<SdkAppStartupKey>(Arc::new(SdkAppStartup { accepted: true }))
                .await?;
        }
        Ok(())
    }
}
