use hoh_kernel::Context;
fn invalid(owner: Context) {
    let pinned = owner.isolate("foo", owner.realm_of("foo"));
    let _ = pinned.foo;
}
fn main() {}
