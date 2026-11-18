use hoh_http_proxy::{DispatcherOptions, ProxyEnvironment, ProxyInstallation};
use hoh_subprocess::{SubprocessEnvironment, scrubbed_parent_env_from};
use indexmap::IndexMap;
use std::{
    io::Write,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

fn environment(entries: &[(&str, &str)]) -> SubprocessEnvironment {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Some((*value).to_owned())))
        .collect()
}

fn install(entries: &[(&str, &str)], inherited: ProxyEnvironment) -> ProxyInstallation {
    let snapshot: IndexMap<String, String> = entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    ProxyInstallation::with_inherited_environment(
        &snapshot,
        inherited,
        |_| {},
        DispatcherOptions::default(),
    )
    .unwrap()
}

struct ProxyServer {
    url: String,
    seen: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
}
impl ProxyServer {
    async fn listen() -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8];
                while !request.ends_with(b"\r\n\r\n") {
                    match tokio::time::timeout(Duration::from_secs(3), socket.read(&mut byte)).await
                    {
                        Ok(Ok(1)) => request.push(byte[0]),
                        _ => break,
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let first = request.lines().next().unwrap_or("");
                recorded.lock().unwrap().push(first.to_owned());
                let reply = if first.starts_with("CONNECT ") {
                    "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 9\r\nConnection: close\r\n\r\nVIA-PROXY"
                };
                let _ = socket.write_all(reply.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        Self { url, seen, task }
    }
}
impl Drop for ProxyServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn child_fetch(target: &str, environment: &IndexMap<String, String>) {
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap());
    child
        .args([
            "--ignored",
            "--exact",
            "egress_child_fetch_fixture",
            "--nocapture",
        ])
        .env_clear()
        .envs(environment)
        .env("HOH_SUBPROCESS_EGRESS_TARGET", target)
        .kill_on_drop(true);
    let result = tokio::time::timeout(Duration::from_secs(10), child.output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).contains("FETCH_RESULT:VIA-PROXY"));
}

#[tokio::test]
#[ignore = "child fixture executed by the two real egress tests"]
async fn egress_child_fetch_fixture() {
    // Rust has no global fetch. The fixture explicitly installs the forwarded Node-style policy.
    let environment: IndexMap<String, String> = std::env::vars().collect();
    assert_eq!(
        environment.get("NODE_USE_ENV_PROXY").map(String::as_str),
        Some("1")
    );
    let dispatcher =
        hoh_http_proxy::create_route_dispatcher(&environment, DispatcherOptions::default())
            .unwrap();
    let target = environment.get("HOH_SUBPROCESS_EGRESS_TARGET").unwrap();
    let body = dispatcher
        .get(target)
        .timeout(Duration::from_millis(1500))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    std::io::stdout()
        .write_all(format!("FETCH_RESULT:{body}\n").as_bytes())
        .unwrap();
}

#[tokio::test]
async fn a_child_node_honors_the_proxy_the_user_exported() {
    let proxy = ProxyServer::listen().await;
    let parent = environment(&[("HTTP_PROXY", &proxy.url)]);
    let installed = install(&[("HTTP_PROXY", &proxy.url)], parent.clone());
    let child = scrubbed_parent_env_from(parent, Some(&installed));
    assert_eq!(child["NODE_USE_ENV_PROXY"], "1");
    assert_eq!(child["HTTP_PROXY"], proxy.url);
    child_fetch("http://child-probe.invalid/x", &child).await;
    assert!(
        proxy
            .seen
            .lock()
            .unwrap()
            .join("|")
            .contains("child-probe.invalid")
    );
}

#[tokio::test]
async fn a_child_node_reaches_a_proxy_the_user_gave_only_as_all_proxy() {
    let proxy = ProxyServer::listen().await;
    let parent = environment(&[("ALL_PROXY", &proxy.url)]);
    let installed = install(&[("ALL_PROXY", &proxy.url)], parent.clone());
    let child = scrubbed_parent_env_from(parent, Some(&installed));
    assert_eq!(child["NODE_USE_ENV_PROXY"], "1");
    assert_eq!(child["ALL_PROXY"], proxy.url);
    assert_eq!(child["HTTP_PROXY"], proxy.url);
    child_fetch("http://all-proxy-probe.invalid/x", &child).await;
    assert!(
        proxy
            .seen
            .lock()
            .unwrap()
            .join("|")
            .contains("all-proxy-probe.invalid")
    );
}

#[test]
fn keeps_a_proxy_the_user_set_for_another_tool_and_fills_only_a_scheme_they_never_named() {
    let parent = environment(&[
        ("HTTP_PROXY", "http://127.0.0.1:7897"),
        ("https_proxy", "socks5://127.0.0.1:1080"),
    ]);
    let installed = install(
        &[
            ("HTTP_PROXY", "http://127.0.0.1:7897"),
            ("https_proxy", "socks5://127.0.0.1:1080"),
        ],
        parent.clone(),
    );
    let child = scrubbed_parent_env_from(parent, Some(&installed));
    assert_eq!(child["https_proxy"], "socks5://127.0.0.1:1080");
    assert!(!child.contains_key("HTTPS_PROXY"));
    assert_eq!(child["NO_PROXY"], "localhost,127.0.0.1,::1,[::1]");
    assert!(!child.contains_key("NODE_USE_ENV_PROXY"));
}

#[test]
fn gives_a_child_the_same_routing_as_its_parent_for_a_scheme_the_user_never_named() {
    let proxy = "http://127.0.0.1:7897";
    let parent = environment(&[("HTTP_PROXY", proxy)]);
    let installed = install(&[("HTTP_PROXY", proxy)], parent.clone());
    assert_eq!(
        installed.environment()["HTTPS_PROXY"].as_deref(),
        Some(proxy)
    );
    assert_eq!(
        scrubbed_parent_env_from(parent, Some(&installed))["HTTPS_PROXY"],
        proxy
    );
}

#[test]
fn adds_nothing_when_no_proxy_is_active() {
    assert!(
        !scrubbed_parent_env_from(SubprocessEnvironment::new(), None)
            .contains_key("NODE_USE_ENV_PROXY")
    );
}

#[test]
fn preserves_the_users_environment_during_a_direct_layer_over_a_proxy() {
    let original = environment(&[
        ("HTTP_PROXY", "http://proxy.test"),
        ("https_proxy", "socks5://proxy.test:1080"),
    ]);
    let outer = install(
        &[
            ("HTTP_PROXY", "http://proxy.test"),
            ("https_proxy", "socks5://proxy.test:1080"),
        ],
        original.clone(),
    );
    let off = outer
        .layer(&|_: &str| None, |_| {}, DispatcherOptions::default())
        .unwrap();
    let child = scrubbed_parent_env_from(outer.environment().clone(), Some(&off));
    assert_eq!(child["HTTP_PROXY"], "http://proxy.test");
    assert_eq!(child["https_proxy"], "socks5://proxy.test:1080");
    assert!(!child.contains_key("NODE_USE_ENV_PROXY"));
}
