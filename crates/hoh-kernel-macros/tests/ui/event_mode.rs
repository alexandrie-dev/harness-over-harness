use hoh_kernel::{event, Context, Parallel};
struct E;
event!(E = "parallel", Parallel);
fn invalid(ctx: &Context) { ctx.emit(&E); }
fn main() {}
