use hoh_kernel::{plugin, Context, Error};
#[plugin(Bad, name = "bad")]
async fn apply(ctx: Context, wrong: ()) -> Result<(), Error> { Ok(()) }
fn main() {}
