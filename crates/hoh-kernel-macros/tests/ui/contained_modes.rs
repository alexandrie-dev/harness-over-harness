use hoh_kernel::{event, Context, Contained, Emit};
struct Notice;
event!(Notice = "notice", Contained);
struct Plain;
event!(Plain = "plain", Emit);
fn invalid(ctx: &Context) {
    ctx.emit(&Notice);
    let _ = ctx.bail::<Notice, u32>(&Notice);
    let _ = ctx.parallel(Notice);
    let _ = ctx.serial::<Notice, u32>(Notice);
    let _ = ctx.waterfall::<Notice, u32, _>(Notice, |_| async { Ok(1) });
    let _ = ctx.on::<Notice>(|_| {});
    let _ = ctx.dispatch(Plain);
}
fn main() {}
