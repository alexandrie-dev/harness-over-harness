use hoh_kernel::{service, Service};
use std::sync::Arc;
service!(Key: u32 = "service");
fn main() { let _ = Service::<Key> { value: Arc::new(1) }; }
