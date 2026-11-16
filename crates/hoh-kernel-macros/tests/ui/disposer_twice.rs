use hoh_kernel::Disposer;
async fn invalid(disposer: Disposer) {
    disposer.dispose().await;
    disposer.dispose().await;
}
fn main() {}
