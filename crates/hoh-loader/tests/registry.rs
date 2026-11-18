mod support;
use hoh_kernel::{Cfg, Context, Error, Kernel, Plugin};
use hoh_loader::*;
use serde_json::{Value, json};
use std::sync::Arc;
use support::{live::*, *};
#[test]
fn exact_names_precede_aliases_and_alias_cycles_do_not_resolve() {
    let mut registry = PluginRegistry::default();
    registry
        .add::<Noop>("exact")
        .alias("exact", "missing")
        .alias("first", "second")
        .alias("second", "exact");
    assert_eq!(registry.resolve("exact").unwrap().name(), "noop");
    assert_eq!(registry.resolve("first").unwrap().name(), "noop");
    registry.alias("a", "b").alias("b", "a");
    assert!(registry.resolve("a").is_none());
    assert!(registry.resolve("missing").is_none());
    assert_eq!(
        registry
            .resolve("@deepseek-ai/cordis-plugin-group")
            .unwrap()
            .name(),
        "cordis:group"
    );
}
#[tokio::test(start_paused = true)]
async fn expression_views_distinguish_absent_services_from_services_without_data() {
    hoh_kernel::service!(Opaque: () = "opaque");
    let kernel = Kernel::start();
    let ctx = kernel.root();
    let registry = PluginRegistry::default();
    let env = env(std::path::Path::new("/fixtures"));
    assert_eq!(
        Expr::parse("ctx.get('opaque')")
            .unwrap()
            .eval(&env, &ctx, &registry, &env.cwd)
            .unwrap(),
        None
    );
    ctx.provide::<Opaque>(Arc::new(())).await.unwrap();
    assert_eq!(
        Expr::parse("ctx.get('opaque')")
            .unwrap()
            .eval(&env, &ctx, &registry, &env.cwd)
            .unwrap(),
        Some(json!({}))
    );
    assert_eq!(
        Expr::parse("!ctx.get('opaque')")
            .unwrap()
            .eval(&env, &ctx, &registry, &env.cwd)
            .unwrap(),
        Some(json!(false))
    );
    kernel.shutdown().await;
}
#[tokio::test(start_paused = true)]
async fn resource_directories_follow_aliases_without_requiring_plugin_constructors() {
    let mut registry = PluginRegistry::default();
    registry
        .resource_dir("pkg", "/resources/pkg".into())
        .alias("alias", "pkg");
    let kernel = Kernel::start();
    let env = env(std::path::Path::new("/fixtures"));
    let source = "process.getBuiltinModule('node:path').join(process.getBuiltinModule('node:path').dirname(process.getBuiltinModule('node:module').createRequire(baseUrl).resolve('alias/package.json')), 'skills')";
    assert_eq!(
        Expr::parse(source)
            .unwrap()
            .eval(&env, &kernel.root(), &registry, &env.cwd)
            .unwrap(),
        Some(json!("/resources/pkg/skills"))
    );
    kernel.shutdown().await;
}
struct CarrierSetup {
    registry: Arc<PluginRegistry>,
    env: EvalEnv,
}
struct Carrier;
impl Plugin for Carrier {
    const NAME: &'static str = "carrier";
    type Config = Value;
    type Deps = ();
    async fn apply(ctx: Context, _: (), cfg: Cfg<Value>) -> Result<(), Error> {
        let setup = ctx.tag::<CarrierSetup>().unwrap();
        let observed = ctx.tag::<Observed>().unwrap();
        locked(&observed.values).push((*cfg.get()).clone());
        let group = Group::mount(&ctx, setup.registry.clone(), setup.env.clone())?;
        group
            .update(serde_json::from_value((*cfg.get()).clone())?)
            .await;
        Ok(())
    }
}
#[tokio::test(start_paused = true)]
async fn carrier_flags_follow_plugin_aliases_and_defer_nested_expressions() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("cordis.yml");
    std::fs::write(&base,"- id: c\n  name: carrier-alias\n  config:\n    - id: child\n      name: noop\n      inject: [ready]\n      config:\n        absent: !!js undefined\n        array: [!!js undefined]\n").unwrap();
    let kernel = Kernel::start();
    let mut registry = PluginRegistry::default();
    registry
        .add::<Carrier>("carrier")
        .alias("carrier-marked", "carrier")
        .carrier("carrier-marked")
        .alias("carrier-alias", "carrier")
        .add::<Noop>("noop");
    let registry = Arc::new(registry);
    let ctx = kernel
        .root()
        .with_tag(Observed::default())
        .with_tag(CarrierSetup {
            registry: registry.clone(),
            env: env(root.path()),
        });
    let loader = boot(
        &ctx,
        BootOptions {
            registry,
            base,
            layers: vec![],
            env: env(root.path()),
        },
    )
    .await
    .unwrap();
    let observed = ctx.tag::<Observed>().unwrap();
    assert_eq!(
        locked(&observed.values)[0][0]["config"]["absent"],
        json!({"__jsExpr":"undefined"})
    );
    hoh_kernel::service!(Ready: () = "ready");
    ctx.provide::<Ready>(Arc::new(())).await.unwrap();
    loader.settled().await;
    let child = loader.resolve("include:child").unwrap();
    assert_eq!(
        *child.fiber.unwrap().effective_config().unwrap(),
        json!({"array":[null]})
    );
    kernel.shutdown().await;
}
