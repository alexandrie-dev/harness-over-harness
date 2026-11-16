use hoh_kernel::{plugin, Context, Error};
#[plugin(Bad, name = "bad")]
fn apply(ctx: Context) -> Result<(), Error> { Ok(()) }
fn main() {}
