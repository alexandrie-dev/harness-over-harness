use base64::{Engine, engine::general_purpose::STANDARD};
use hoh_atomic_write::{WriteFileAtomicOptions, write_file_atomic};
use hoh_kernel::{Cfg, Context, Error, Inject, NoConfig, Plugin, Service};
use hoh_loader::ProfileContextKey;
use hoh_protocol::{
    WhisperDictationFailure as Failure, WhisperDictationParams, WhisperDictationResult,
};
use hoh_sdk_jsonrpc_server::HohRpcMethods;
use reqwest::{Client, redirect::Policy};
use serde::Deserialize;
use std::{io::ErrorKind, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use url::Url;

const MAX_WAV_BYTES: usize = 44 + 300 * 16_000 * 2;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const MAX_URL_BYTES: usize = 2048;

pub struct AuthorPlugin;

#[derive(Inject)]
pub struct Deps {
    profile: Service<ProfileContextKey>,
    methods: Service<HohRpcMethods>,
}

impl Plugin for AuthorPlugin {
    const NAME: &'static str = "hoh:plugin-whisper-dictation";
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(ctx: Context, deps: Deps, _: Cfg<NoConfig>) -> Result<(), Error> {
        let service =
            Arc::new(Dictation::open(deps.profile.dir.join("whisper-dictation.json")).await?);
        deps.methods
            .register(&ctx, "hoh/voice/whisper", move |params| {
                let service = service.clone();
                async move {
                    let result = service
                        .handle(serde_json::from_value(params)?)
                        .await
                        .unwrap_or_else(|failure| WhisperDictationResult::Error { failure });
                    Ok(serde_json::to_value(result)?)
                }
            })?;
        Ok(())
    }
}

struct Dictation {
    client: Client,
    path: PathBuf,
    server: Mutex<Option<Url>>,
}

#[derive(Deserialize)]
struct Transcript {
    text: String,
}

impl Dictation {
    async fn open(path: PathBuf) -> Result<Self, Error> {
        let server = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<Option<String>>(&bytes)
                .ok()
                .flatten()
                .and_then(|value| parse_url(&value).ok()),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(90))
            .redirect(Policy::none())
            .no_proxy()
            .build()?;
        Ok(Self {
            client,
            path,
            server: Mutex::new(server),
        })
    }

    async fn handle(
        &self,
        params: WhisperDictationParams,
    ) -> Result<WhisperDictationResult, Failure> {
        match params {
            WhisperDictationParams::GetSettings {} => Ok(WhisperDictationResult::Settings {
                server_url: self.server.lock().await.as_ref().map(ToString::to_string),
            }),
            WhisperDictationParams::SetSettings { server_url } => {
                let server = match server_url.trim() {
                    "" => None,
                    value => Some(parse_url(value)?),
                };
                let server_url = server.as_ref().map(ToString::to_string);
                let mut current = self.server.lock().await;
                write_file_atomic(
                    &self.path,
                    serde_json::to_vec(&server_url).map_err(|_| Failure::SaveFailed {})?,
                    WriteFileAtomicOptions {
                        mode: 0o600,
                        dir_mode: None,
                    },
                )
                .await
                .map_err(|_| Failure::SaveFailed {})?;
                *current = server;
                Ok(WhisperDictationResult::Settings { server_url })
            }
            WhisperDictationParams::Transcribe {
                server_url,
                wav_base64,
            } => {
                let server = self
                    .server
                    .lock()
                    .await
                    .clone()
                    .filter(|url| url.as_str() == server_url)
                    .ok_or(Failure::AddressChanged {})?;
                let audio = decode_wav(&wav_base64)?;
                let transcript = self.transcribe(server, audio).await?;
                Ok(WhisperDictationResult::Transcript {
                    text: transcript.text,
                })
            }
        }
    }

    async fn transcribe(&self, mut url: Url, audio: Vec<u8>) -> Result<Transcript, Failure> {
        if !url.path().trim_end_matches('/').ends_with("/transcribe") {
            url.path_segments_mut()
                .map_err(|_| Failure::InvalidAddress {})?
                .pop_if_empty()
                .push("transcribe");
        }
        let boundary = hoh_util_crypto::random_uuid().to_string();
        let mut body = format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.wav\"\r\nContent-Type: audio/wav\r\n\r\n"
        ).into_bytes();
        body.reserve(audio.len() + boundary.len() + 8);
        body.extend(audio);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let mut response = self
            .client
            .post(url)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body)
            .send()
            .await
            .map_err(whisper_connection_error)?;
        if !response.status().is_success() {
            return Err(Failure::ServerRejected {
                status: response.status().as_u16(),
            });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(whisper_connection_error)? {
            if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                return Err(Failure::InvalidResponse {});
            }
            body.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&body).map_err(|_| Failure::InvalidResponse {})
    }
}

fn parse_url(value: &str) -> Result<Url, Failure> {
    if value.len() > MAX_URL_BYTES
        || !matches!(value.split_once("://"), Some((_, rest)) if !rest.is_empty() && !rest.starts_with('/'))
    {
        return Err(Failure::InvalidAddress {});
    }
    let mut url = Url::parse(value).map_err(|_| Failure::InvalidAddress {})?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.as_str().len() > MAX_URL_BYTES
    {
        return Err(Failure::InvalidAddress {});
    }
    if url.path().ends_with('/') && url.path() != "/" {
        let path = url.path().trim_end_matches('/').to_owned();
        url.set_path(&path);
    }
    Ok(url)
}

fn decode_wav(value: &str) -> Result<Vec<u8>, Failure> {
    if value.len() > MAX_WAV_BYTES.div_ceil(3) * 4 {
        return Err(Failure::InvalidAudio {});
    }
    let audio = STANDARD
        .decode(value)
        .map_err(|_| Failure::InvalidAudio {})?;
    if audio.len() <= 44
        || audio.len() > MAX_WAV_BYTES
        || !audio.starts_with(b"RIFF")
        || &audio[8..12] != b"WAVE"
    {
        return Err(Failure::InvalidAudio {});
    }
    Ok(audio)
}

fn whisper_connection_error(error: reqwest::Error) -> Failure {
    if error.is_timeout() {
        Failure::Timeout {}
    } else {
        Failure::Unreachable {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
    };

    #[tokio::test]
    async fn uploads_wav_to_the_saved_daemon_and_restores_its_address() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/whisper", listener.local_addr().unwrap());
        let mut wav = vec![0; 46];
        wav[..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        let wav_base64 = STANDARD.encode(&wav);
        let daemon = tokio::task::spawn_blocking(move || {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut socket = BufReader::new(socket);
            let mut headers = String::new();
            loop {
                let start = headers.len();
                assert_ne!(socket.read_line(&mut headers).unwrap(), 0);
                if &headers[start..] == "\r\n" {
                    break;
                }
            }
            assert!(headers.starts_with("POST /whisper/transcribe HTTP/1.1\r\n"));
            assert!(headers.contains("content-type: multipart/form-data; boundary="));
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let mut body = vec![0; length];
            socket.read_exact(&mut body).unwrap();
            assert!(body.windows(wav.len()).any(|window| window == wav));
            assert!(body.starts_with(b"--"));
            assert!(
                String::from_utf8_lossy(&body)
                    .contains("name=\"file\"; filename=\"dictation.wav\"")
            );
            let json = r#"{"text":"Привет!","language":"ru","segments":[]}"#;
            write!(socket.get_mut(),
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                json.len()).unwrap();
        });

        let profile = tempfile::tempdir().unwrap();
        let path = profile.path().join("whisper.json");
        let dictation = Dictation::open(path.clone()).await.unwrap();
        assert_eq!(
            dictation
                .handle(WhisperDictationParams::SetSettings {
                    server_url: url.clone()
                })
                .await,
            Ok(WhisperDictationResult::Settings {
                server_url: Some(url.clone())
            }),
        );

        let restored = Dictation::open(path).await.unwrap();
        assert_eq!(
            restored
                .handle(WhisperDictationParams::Transcribe {
                    server_url: "http://127.0.0.1:1/".into(),
                    wav_base64: wav_base64.clone()
                })
                .await,
            Err(Failure::AddressChanged {}),
        );
        assert_eq!(
            restored
                .handle(WhisperDictationParams::Transcribe {
                    server_url: url,
                    wav_base64
                })
                .await,
            Ok(WhisperDictationResult::Transcript {
                text: "Привет!".into()
            }),
        );
        daemon.await.unwrap();
    }

    #[test]
    fn rejects_urls_with_credentials_and_invalid_recordings() {
        for value in [
            "file:///tmp/audio",
            "http://u:p@localhost",
            "http://localhost/?key=x",
            "http://localhost/#path",
            "http:///localhost",
        ] {
            assert_eq!(parse_url(value), Err(Failure::InvalidAddress {}));
        }
        assert_eq!(
            parse_url("http://127.0.0.1:8765/transcribe/")
                .unwrap()
                .as_str(),
            "http://127.0.0.1:8765/transcribe",
        );
        assert_eq!(decode_wav("bad!"), Err(Failure::InvalidAudio {}));
        assert_eq!(decode_wav("YQ=="), Err(Failure::InvalidAudio {}));
    }
}
