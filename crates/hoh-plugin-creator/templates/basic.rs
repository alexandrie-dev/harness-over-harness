use hoh_kernel::{Cfg, Context, Error, NoConfig, Plugin};

pub struct AuthorPlugin;

impl Plugin for AuthorPlugin {
    const NAME: &'static str = "hoh:plugin-@ID@";
    type Config = NoConfig;
    type Deps = ();

    async fn apply(_: Context, _: (), _: Cfg<NoConfig>) -> Result<(), Error> {
        Ok(())
    }
}
