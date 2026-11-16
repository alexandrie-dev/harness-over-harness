use hoh_kernel::Next;
async fn invalid(next: Next<()>) {
    let _ = next.run().await;
    let _ = next.run().await;
}
fn main() {}
