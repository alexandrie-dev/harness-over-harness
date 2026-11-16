use hoh_kernel::{plugin, service, Cfg, Context, Error, Inject, Service};
service!(pub Key: dyn Api = "api");
pub trait Api: Send + Sync {}
#[derive(Inject)]
pub struct Deps { pub api: Service<Key> }
#[plugin(Full, name = "full")]
pub async fn full(_ctx: Context, _deps: Deps, _config: Cfg<hoh_kernel::NoConfig>) -> Result<(), Error> { Ok(()) }
#[plugin(Empty, name = "empty")]
pub async fn empty(_ctx: Context) -> Result<(), Error> { Ok(()) }
#[plugin(Reordered, name = "reordered")]
pub async fn reordered(_config: Cfg<hoh_kernel::NoConfig>, _ctx: Context) -> Result<(), Error> { Ok(()) }
fn main() {}
