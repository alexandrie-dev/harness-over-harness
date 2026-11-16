#![allow(dead_code)]

use hoh_http_proxy::{DispatcherOptions, ProxyEnvironment, ProxyInstallation};
use indexmap::IndexMap;
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::Notify,
    task::JoinHandle,
};

pub fn env(values: &[(&str, &str)]) -> IndexMap<String, String> {
    values
        .iter()
        .map(|(name, value)| {
            let name = if cfg!(windows) {
                name.to_uppercase()
            } else {
                (*name).to_owned()
            };
            (name, (*value).to_owned())
        })
        .collect()
}

pub fn lookup(values: &IndexMap<String, String>) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| {
        values
            .get(&if cfg!(windows) {
                name.to_uppercase()
            } else {
                name.to_owned()
            })
            .cloned()
    }
}

pub fn inherited(values: &[(&str, &str)]) -> ProxyEnvironment {
    hoh_http_proxy::policy::POLICY_ENV_NAMES
        .iter()
        .flat_map(|(_, names)| names)
        .map(|name| {
            (
                (*name).to_owned(),
                values
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| (*value).to_owned()),
            )
        })
        .collect()
}

pub fn install(values: &[(&str, &str)]) -> (ProxyInstallation, Vec<String>) {
    let values = env(values);
    let mut reports = Vec::new();
    let installation = ProxyInstallation::with_inherited_environment(
        &lookup(&values),
        ProxyEnvironment::new(),
        |message| reports.push(message.to_owned()),
        DispatcherOptions::default(),
    )
    .unwrap();
    (installation, reports)
}

pub struct Server {
    pub address: SocketAddr,
    pub seen: Arc<Mutex<Vec<String>>>,
    task: JoinHandle<()>,
    received: Arc<Notify>,
    release: Arc<Notify>,
}

impl Server {
    pub async fn start(body: &'static str) -> Self {
        Self::start_with_gate(body, false).await
    }

    pub async fn paused(body: &'static str) -> Self {
        Self::start_with_gate(body, true).await
    }

    async fn start_with_gate(body: &'static str, gated: bool) -> Self {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = seen.clone();
        let received = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let signal = received.clone();
        let gate = release.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut byte = [0u8];
                while !request.ends_with(b"\r\n\r\n") {
                    match tokio::time::timeout(Duration::from_secs(3), stream.read(&mut byte)).await
                    {
                        Ok(Ok(1)) => request.push(byte[0]),
                        _ => break,
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let first = request.lines().next().unwrap_or("");
                recorded.lock().unwrap().push(first.to_owned());
                signal.notify_one();
                if gated {
                    gate.notified().await;
                }
                if !first.starts_with("CONNECT ") {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(response.as_bytes()).await;
                }
                let _ = stream.shutdown().await;
            }
        });
        Self {
            address,
            seen,
            task,
            received,
            release,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }
    pub fn requests(&self) -> Vec<String> {
        self.seen.lock().unwrap().clone()
    }

    pub async fn wait_for_request(&self) {
        tokio::time::timeout(Duration::from_secs(3), self.received.notified())
            .await
            .unwrap();
    }

    pub fn respond(&self) {
        self.release.notify_one();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
