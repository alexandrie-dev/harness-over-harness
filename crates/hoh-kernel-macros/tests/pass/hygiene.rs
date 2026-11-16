use hoh_kernel::{Context,Cfg,NoConfig,Inject};
type Result<T> = std::result::Result<T,hoh_kernel::Error>;
#[allow(unused_macros)]
macro_rules! vec {($($tt:tt)*)=>{compile_error!("shadowed vec must not be used")};}
hoh_kernel::service!(Key:u32="key");
#[derive(Inject)]pub struct Deps{key:hoh_kernel::Service<Key>}
#[hoh_kernel::plugin(ConfigPlugin,name="config")]
async fn config(ctx:Context,config:Cfg<NoConfig>)->Result<()>{let _=(ctx,config);Ok(())}
#[hoh_kernel::plugin(CtxPlugin,name="ctx")]
async fn ctx(_ctx:Context)->Result<()>{Ok(())}
#[hoh_kernel::plugin(DepsPlugin,name="deps")]
async fn deps(_ctx:Context,_deps:Deps)->Result<()>{Ok(())}
fn main(){let _=std::mem::size_of::<Deps>();}
#[hoh_kernel::plugin(InternalName,name="internal-name")]
async fn __hoh_ctx(_ctx:Context)->Result<()>{Ok(())}
