use hoh_kernel::{plugin, Context, Error};
#[plugin(Bad, name = "bad")]
async fn apply(ctx: Context, config: String) -> Result<(), Error> { Ok(()) }
fn main() {}
