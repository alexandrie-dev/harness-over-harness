use super::*;
use hoh_kernel::*;
use std::sync::atomic::{AtomicBool, Ordering};
pub struct Resource {
    pub live: AtomicBool,
    pub t: Arc<Track>,
}
impl Resource {
    pub fn read(&self) {
        assert!(self.live.load(Ordering::Acquire));
        self.t.push("consumer read: open");
    }
}
service!(pub Foo:Resource="foo");
#[derive(Inject)]
pub struct Deps {
    pub foo: Service<Foo>,
}
pub async fn provider(ctx: &Context, composed: bool) -> Fiber {
    let f = ctx
        .inject(move |ctx, (): ()| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            let t = (*t).clone();
            lock(&t.contexts).push(ctx.clone());
            let resource = Arc::new(Resource {
                live: AtomicBool::new(true),
                t: t.clone(),
            });
            let close = resource.clone();
            let undo = Undo::sync(move || {
                close.live.store(false, Ordering::Release);
                close.t.push("provider closed");
            });
            if composed {
                let ctx2 = ctx.clone();
                ctx.effect_async::<Error, _>("provider composed", move |u| async move {
                    u.push(undo)?;
                    u.push(ctx2.provide::<Foo>(resource).await?)?;
                    Ok(())
                })
                .await?;
            } else {
                ctx.effect("resource", || undo)?;
                ctx.provide::<Foo>(resource).await?;
            }
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    f
}
pub async fn consumer(ctx: &Context, wait: bool, error: bool) -> Fiber {
    let f = ctx
        .inject(move |ctx, d: Deps| async move {
            let t = ctx.tag::<Arc<Track>>().unwrap();
            ctx.effect("consumer", move || {
                Undo::future(move || async move {
                    if wait {
                        t.gate.acquire().await.unwrap().forget();
                    }
                    d.foo.read();
                    if error {
                        return Err("cleanup failed".into());
                    }
                    Ok(())
                })
            })?;
            Ok(())
        })
        .unwrap();
    f.settled().await.unwrap();
    f
}
