use @CRATE@::AuthorPlugin;
use hoh_kernel::{FiberState, Kernel};

#[tokio::test]
async fn mounts() {
    let kernel = Kernel::start();
    let fiber = kernel
        .root()
        .plugin::<AuthorPlugin>(Default::default())
        .unwrap();
    fiber.settled().await.unwrap();
    assert_eq!(fiber.state(), FiberState::Active);
    kernel.shutdown().await;
}
