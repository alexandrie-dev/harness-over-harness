use crate::{
    Config,
    diagnostic::{McpLocalError, McpLocalErrorCode, describe_error, redact_secrets},
    templates::{assert_template_syntax, credential_refs_in, resolve_template_string},
};
use hoh_credentials::{
    CredentialProvider, CredentialRef, Credentials, ResolvedCredential, credential_ref,
};
use hoh_error::BoxError;
use hoh_kernel::Service;
use hoh_timeout::AbortSignal;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, MutexGuard},
};

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[derive(Clone)]
enum CredentialSource {
    Provider(Arc<dyn CredentialProvider>),
    Service(Service<Credentials>),
}
impl CredentialSource {
    async fn resolve(
        &self,
        reference: &CredentialRef,
    ) -> Result<Option<ResolvedCredential>, BoxError> {
        match self {
            Self::Provider(provider) => provider.resolve(reference).await,
            Self::Service(provider) => provider.resolve(reference).await,
        }
    }
}

pub struct McpTemplateBinding {
    provider: Option<CredentialSource>,
    config: Config,
    refs: HashSet<String>,
    secrets: Arc<Mutex<HashSet<String>>>,
}

impl McpTemplateBinding {
    pub fn new(
        provider: Option<Arc<dyn CredentialProvider>>,
        config: Config,
    ) -> Result<Self, BoxError> {
        Self::from_source(provider.map(CredentialSource::Provider), config)
    }
    pub fn from_service(
        provider: Option<Service<Credentials>>,
        config: Config,
    ) -> Result<Self, BoxError> {
        Self::from_source(provider.map(CredentialSource::Service), config)
    }
    fn from_source(provider: Option<CredentialSource>, config: Config) -> Result<Self, BoxError> {
        let strings = config_strings(&config);
        let refs = if config.expand_templates() {
            strings
                .iter()
                .flat_map(|value| credential_refs_in(value))
                .collect()
        } else {
            HashSet::new()
        };
        if config.expand_templates() {
            for value in strings {
                assert_template_syntax(value, &format!("mcp-client({})", config.server_name()))
                    .map_err(|error| Box::new(error) as BoxError)?;
            }
            if !refs.is_empty() && provider.is_none() {
                return Err(Box::new(McpLocalError::new(
                    "MCP credential templates require a credentials provider",
                    McpLocalErrorCode::CredentialsUnavailable,
                )));
            }
        }
        Ok(Self {
            provider,
            config,
            refs,
            secrets: Arc::new(Mutex::new(HashSet::new())),
        })
    }
    pub fn refs(&self) -> &HashSet<String> {
        &self.refs
    }
    pub fn redact(&self, text: &str) -> String {
        redact_secrets(text, lock(&self.secrets).iter())
    }
    pub fn diagnostic(&self, error: &(dyn std::error::Error + 'static), operation: &str) -> String {
        if !self.config.expand_templates() {
            return describe_error(error);
        }
        format!(
            "mcp-client({}): {operation}: {}",
            self.config.server_name(),
            self.redact(&describe_error(error))
        )
    }
    pub async fn resolve(&self, signal: &AbortSignal) -> Result<Config, BoxError> {
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
        if !self.config.expand_templates() {
            return Ok(self.config.clone());
        }
        let mut cache = HashMap::<String, Option<String>>::new();
        let provider = self.provider.as_ref().ok_or_else(|| {
            Box::new(McpLocalError::new(
                "MCP credential templates require a credentials provider",
                McpLocalErrorCode::CredentialsUnavailable,
            )) as BoxError
        })?;
        for name in &self.refs {
            let reference = credential_ref(name).map_err(|error| Box::new(error) as BoxError)?;
            let value = provider
                .resolve(&reference)
                .await?
                .map(|resolved| resolved.value);
            if let Some(value) = value.as_ref().filter(|value| !value.is_empty()) {
                let mut known = lock(&self.secrets);
                known.insert(value.clone());
                known.insert(encode_uri_component(value));
            }
            cache.insert(name.clone(), value);
        }
        match &self.config {
            Config::Stdio {
                server_name,
                command,
                args,
                env,
                cwd,
                tool_call_timeout_ms,
                startup_timeout_ms,
                close_timeout_ms,
                expand_templates,
                fail_on_startup_error,
                max_instruction_bytes,
                reconnect,
            } => {
                let command = self
                    .resolve_value(command, "command", &mut cache, signal)
                    .await?;
                let mut next_args = Vec::with_capacity(args.len());
                for (index, arg) in args.iter().enumerate() {
                    next_args.push(
                        self.resolve_value(arg, &format!("args[{index}]"), &mut cache, signal)
                            .await?,
                    );
                }
                let mut next_env = indexmap::IndexMap::new();
                for (key, value) in env {
                    next_env.insert(
                        key.clone(),
                        self.resolve_value(value, &format!("env.{key}"), &mut cache, signal)
                            .await?,
                    );
                }
                let cwd = self.resolve_value(cwd, "cwd", &mut cache, signal).await?;
                Ok(Config::Stdio {
                    server_name: server_name.clone(),
                    command,
                    args: next_args,
                    env: next_env,
                    cwd,
                    tool_call_timeout_ms: *tool_call_timeout_ms,
                    startup_timeout_ms: *startup_timeout_ms,
                    close_timeout_ms: *close_timeout_ms,
                    expand_templates: *expand_templates,
                    fail_on_startup_error: *fail_on_startup_error,
                    max_instruction_bytes: *max_instruction_bytes,
                    reconnect: *reconnect,
                })
            }
            Config::StreamableHttp {
                server_name,
                url,
                headers,
                tool_call_timeout_ms,
                startup_timeout_ms,
                close_timeout_ms,
                expand_templates,
                fail_on_startup_error,
                max_instruction_bytes,
                reconnect,
            } => {
                let url = self.resolve_value(url, "url", &mut cache, signal).await?;
                let mut next_headers = indexmap::IndexMap::new();
                for (key, value) in headers {
                    next_headers.insert(
                        key.clone(),
                        self.resolve_value(value, &format!("headers.{key}"), &mut cache, signal)
                            .await?,
                    );
                }
                Ok(Config::StreamableHttp {
                    server_name: server_name.clone(),
                    url,
                    headers: next_headers,
                    tool_call_timeout_ms: *tool_call_timeout_ms,
                    startup_timeout_ms: *startup_timeout_ms,
                    close_timeout_ms: *close_timeout_ms,
                    expand_templates: *expand_templates,
                    fail_on_startup_error: *fail_on_startup_error,
                    max_instruction_bytes: *max_instruction_bytes,
                    reconnect: *reconnect,
                })
            }
        }
    }
    async fn resolve_value(
        &self,
        value: &str,
        path: &str,
        cache: &mut HashMap<String, Option<String>>,
        signal: &AbortSignal,
    ) -> Result<String, BoxError> {
        let result = resolve_template_string(value, path, |name| {
            let cached = cache.get(name).cloned();
            Box::pin(async move { Ok(cached.flatten()) })
        })
        .await?;
        signal
            .throw_if_aborted()
            .map_err(|error| Box::new(error) as BoxError)?;
        Ok(result)
    }
}

fn config_strings(config: &Config) -> Vec<&str> {
    match config {
        Config::Stdio {
            command,
            args,
            env,
            cwd,
            ..
        } => std::iter::once(command.as_str())
            .chain(args.iter().map(String::as_str))
            .chain(env.values().map(String::as_str))
            .chain(std::iter::once(cwd.as_str()))
            .collect(),
        Config::StreamableHttp { url, headers, .. } => std::iter::once(url.as_str())
            .chain(headers.values().map(String::as_str))
            .collect(),
    }
}
fn encode_uri_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(
                byte,
                b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')'
            )
        {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[(byte >> 4) as usize]));
            out.push(char::from(HEX[(byte & 15) as usize]));
        }
    }
    out
}
