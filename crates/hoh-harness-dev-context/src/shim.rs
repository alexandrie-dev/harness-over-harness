use hoh_kernel::service;

/// SHIM(hoh-web-app): advertised Web GUI URL; replace when the skipped bundle owner has a crate.
pub trait WebAppApi: Send + Sync + 'static {
    fn get_advertised_url(&self) -> String;
}

// SHIM(hoh-web-app): exact source service name used by the injected dependency.
service!(pub WebApp: dyn WebAppApi = "webApp");
