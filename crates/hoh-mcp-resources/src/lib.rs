//! Scoped MCP resource providers and the shared model-facing resource tools.
//!
//! This crate owns neither an MCP transport nor pagination policy. Connection plugins register a
//! provider for each configured server; the runtime exposes three tools only while at least one
//! provider is visible in that exact scope. Every invocation selects the provider again from the
//! calling agent's scope, so unloading or replacing a connection takes effect immediately.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
};

use hoh_error::BoxError;
use hoh_kernel::{Cfg, Context, Disposer, Inject, NoConfig, Plugin, Service, Undo};
use hoh_llm::ContentBlock;
use hoh_scope::{
    CreateScopeOptions, NamedEntries, Scope, ScopeKey, ScopeLayer, ScopedLayers, create_scope,
    scope_of,
    store::{EffectOptions, EntryUndo},
};
use hoh_system_prompt::{
    PromptSection, PromptSectionOrderName, PromptText, SystemPrompt, SystemPromptKey,
};
use hoh_tools::{
    DefineToolOptions, DefineToolOutput, ToolArgsError, ToolExecution, ToolRegistrationOptions,
    Tools, define_tool,
};
use hoh_util_values::js_json_stringify;
use serde_json::{Value, json};

/// A provider request future. Providers may borrow their own connection state for the request.
pub type McpResourceFuture<'a> = Pin<Box<dyn Future<Output = Result<Value, BoxError>> + Send + 'a>>;

/// One supported resource operation, with server-owned cursors and URIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum McpResourceRequest {
    /// `resources/list`, optionally beginning at an opaque server cursor.
    List { cursor: Option<String> },
    /// `resources/templates/list`, optionally beginning at an opaque server cursor.
    ListTemplates { cursor: Option<String> },
    /// `resources/read` for an explicit resource URI.
    Read { uri: String },
}

impl McpResourceRequest {
    /// The MCP protocol method represented by this request.
    pub fn method(&self) -> &'static str {
        match self {
            Self::List { .. } => "resources/list",
            Self::ListTemplates { .. } => "resources/templates/list",
            Self::Read { .. } => "resources/read",
        }
    }

    /// The opaque cursor carried by either list request.
    pub fn cursor(&self) -> Option<&str> {
        match self {
            Self::List { cursor } | Self::ListTemplates { cursor } => cursor.as_deref(),
            Self::Read { .. } => None,
        }
    }
}

/// One configured server's resource access, owned by its MCP connection plugin.
pub trait McpResourceProvider: Send + Sync + 'static {
    /// Run an operation against one live connection generation.
    fn request<'a>(
        &'a self,
        request: McpResourceRequest,
        exec: Arc<ToolExecution>,
    ) -> McpResourceFuture<'a>;
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct ToolLease {
    registration: Disposer,
    owned_scope: Option<Scope>,
}

type DisposeFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

impl ToolLease {
    /// Begin visibility withdrawal synchronously and return the complete teardown lifetime.
    fn dispose(self) -> DisposeFuture {
        let registration = self.registration.dispose();
        let scope = self.owned_scope.map(|scope| scope.raw_dispose().dispose());
        Box::pin(async move {
            registration.await;
            if let Some(scope) = scope {
                scope.await;
            }
        })
    }
}

struct ResourceLayer {
    servers: NamedEntries<Arc<dyn McpResourceProvider>>,
    tools: Arc<Mutex<Option<ToolLease>>>,
}

impl ResourceLayer {
    fn new() -> Self {
        Self {
            servers: NamedEntries::new(|name| {
                format!("MCP resource server \"{name}\" is already registered in this scope").into()
            }),
            tools: Arc::new(Mutex::new(None)),
        }
    }
}

impl ScopeLayer for ResourceLayer {
    fn is_empty(&self) -> bool {
        self.servers.is_empty()
    }
}

struct RuntimeInner {
    self_ctx: Context,
    tools: Service<Tools>,
    layers: ScopedLayers<ResourceLayer>,
}

/// Scoped resource access plus three tools shared by configured MCP servers.
#[derive(Clone)]
pub struct McpResourceRuntime(Arc<RuntimeInner>);

hoh_kernel::service!(pub McpResources: McpResourceRuntime = "mcpResources");

/// The runtime's sole required service (`static inject = ['tools']`).
#[derive(Inject)]
pub struct Deps {
    tools: Service<Tools>,
}

#[derive(Inject)]
struct PromptDeps {
    system_prompt: Service<SystemPromptKey>,
}

impl Plugin for McpResourceRuntime {
    const NAME: &'static str = "mcp-resources";
    type Config = NoConfig;
    type Deps = Deps;

    async fn apply(
        ctx: Context,
        deps: Self::Deps,
        _config: Cfg<Self::Config>,
    ) -> Result<(), BoxError> {
        let runtime = Arc::new(Self::new(ctx.clone(), deps.tools)?);
        let prompt_runtime = runtime.clone();
        let prompt = ctx.inject::<PromptDeps, _>(move |prompt_ctx, deps| {
            let prompt_runtime = prompt_runtime.clone();
            async move {
                deps.system_prompt.section(
                    &prompt_ctx,
                    PromptSection {
                        name: "mcp-resource-servers".into(),
                        order: SystemPrompt::get_section_order(PromptSectionOrderName::McpServers),
                        interpolate: Some(false),
                        text: PromptText::provider(move |assemble| {
                            Ok(prompt_runtime.server_prompt(assemble.scope.as_ref()))
                        }),
                        complete: false,
                    },
                )?;
                Ok(())
            }
        })?;
        if ctx.has("systemPrompt") {
            prompt.settled().await?;
        }
        ctx.provide::<McpResources>(runtime).await?;
        Ok(())
    }
}

impl McpResourceRuntime {
    /// Construct a runtime for embedders and tests. Normal compositions mount the plugin instead.
    pub fn new(self_ctx: Context, tools: Service<Tools>) -> Result<Self, BoxError> {
        let layers = ScopedLayers::new(|_| Ok(ResourceLayer::new()), || Ok(()))?;
        Ok(Self(Arc::new(RuntimeInner {
            self_ctx,
            tools,
            layers,
        })))
    }

    /// Register one server in the calling context's scope.
    ///
    /// The first server in a layer installs the shared tools. The exact returned disposer removes
    /// this provider; removing the last provider also withdraws the tools synchronously.
    pub fn register(
        &self,
        ctx: &Context,
        server: impl Into<String>,
        provider: Arc<dyn McpResourceProvider>,
    ) -> Result<Disposer, BoxError> {
        let server = server.into();
        let label = format!("mcpResources.provider({server})");
        let scope = scope_of(ctx);
        let runtime = self.clone();
        let cleanup: Arc<Mutex<Option<DisposeFuture>>> = Arc::new(Mutex::new(None));
        ctx.try_effect(label, move |undos| -> Result<(), BoxError> {
            let cleanup_on_remove = cleanup.clone();
            let layer_label = format!("mcpResources.provider.layer({server})");
            let layers = runtime.0.layers.clone();
            let register_runtime = runtime.clone();
            let layer_registration = layers.effect(
                ctx,
                move |layer| {
                    let first = layer.servers.is_empty();
                    let remove = layer.servers.insert(server.clone(), provider)?;
                    if first {
                        match register_runtime.register_tools(scope.clone()) {
                            Ok(lease) => *lock(&layer.tools) = Some(lease),
                            Err(error) => {
                                remove.undo();
                                return Err(error);
                            }
                        }
                    }
                    let tools = layer.tools.clone();
                    let servers = layer.servers.clone();
                    let cleanup = cleanup_on_remove.clone();
                    Ok(EntryUndo::new(move || {
                        remove.undo();
                        if servers.is_empty()
                            && let Some(lease) = lock(&tools).take()
                        {
                            *lock(&cleanup) = Some(lease.dispose());
                        }
                    }))
                },
                EffectOptions::new(layer_label),
            )?;
            let cleanup_wait = cleanup.clone();
            if let Err(error) = undos.push(Undo::future(move || async move {
                let cleanup = lock(&cleanup_wait).take();
                if let Some(cleanup) = cleanup {
                    cleanup.await;
                }
                Ok(())
            })) {
                drop(layer_registration.dispose());
                return Err(error.into());
            }
            // LIFO cleanup removes the layer registration first, then waits for any tool scope
            // teardown it started. Adoption also prevents the inner effect from racing its owner.
            undos.push(layer_registration)?;
            Ok(())
        })
    }

    fn register_tools(&self, scope: Option<ScopeKey>) -> Result<ToolLease, BoxError> {
        let owned_scope = scope
            .map(|key| create_scope(&self.0.self_ctx, key, CreateScopeOptions::default()))
            .transpose()?;
        let tool_ctx = owned_scope
            .as_ref()
            .map_or(&self.0.self_ctx, |owned| &owned.ctx);
        let runtime = self.clone();
        let request: Arc<RequestResource> =
            Arc::new(move |server, request, exec| runtime.request(&server, request, exec));
        let registration = match register_resource_tools_with(&self.0.tools, tool_ctx, request) {
            Ok(registration) => registration,
            Err(error) => {
                if let Some(scope) = owned_scope {
                    drop(scope.dispose());
                }
                return Err(error);
            }
        };
        Ok(ToolLease {
            registration,
            owned_scope,
        })
    }

    fn request(
        &self,
        server: &str,
        request: McpResourceRequest,
        exec: Arc<ToolExecution>,
    ) -> McpResourceFuture<'static> {
        let scope = exec.agent.as_ref().map(|agent| agent.scope_key());
        let provider = self
            .0
            .layers
            .merge(scope.as_ref(), |layer| &layer.servers)
            .get(server)
            .cloned();
        let server = server.to_owned();
        Box::pin(async move {
            let provider = provider.ok_or_else(|| -> BoxError {
                Box::new(ToolArgsError::new(vec![format!(
                    "server: MCP resource server \"{server}\" is unavailable in this agent's scope"
                )]))
            })?;
            provider.request(request, exec).await
        })
    }

    fn server_prompt(&self, scope: Option<&ScopeKey>) -> String {
        let mut names: Vec<String> = self
            .0
            .layers
            .merge(scope, |layer| &layer.servers)
            .into_keys()
            .collect();
        names.sort_by(|a, b| a.encode_utf16().cmp(b.encode_utf16()));
        if names.is_empty() {
            return String::new();
        }
        format!(
            "## MCP resource servers\n\nUse list_mcp_resources, list_mcp_resource_templates, or read_mcp_resource with one of these names as the server argument: {}.",
            js_json_stringify(&Value::Array(
                names.into_iter().map(Value::String).collect()
            ))
        )
    }
}

type RequestResource = dyn Fn(String, McpResourceRequest, Arc<ToolExecution>) -> McpResourceFuture<'static>
    + Send
    + Sync;

fn list_parameters() -> Value {
    json!({
        "server": {
            "type": "string",
            "required": true,
            "description": "Configured MCP server name."
        },
        "cursor": {
            "type": "string",
            "description": "Continuation cursor returned by this server."
        }
    })
}

fn resource_output() -> DefineToolOutput {
    DefineToolOutput {
        schema: json!({ "type": "json" }),
        render: Arc::new(|args, value| {
            let server = args
                .get("server")
                .and_then(Value::as_str)
                .expect("validated server argument");
            render_resource_result(server, value)
        }),
        presentation_meta: None,
    }
}

fn cursor_from(args: &Value) -> Option<String> {
    args.get("cursor")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

fn resource_tool(
    name: &'static str,
    description: &'static str,
    parameters: Value,
    request: Arc<RequestResource>,
    operation: impl Fn(&Value) -> McpResourceRequest + Send + Sync + 'static,
) -> Result<hoh_tools::ToolDefinition, BoxError> {
    let mut options = DefineToolOptions::new(
        name,
        description,
        parameters,
        resource_output(),
        move |args, exec| {
            let server = args
                .get("server")
                .and_then(Value::as_str)
                .expect("validated server argument")
                .to_owned();
            request(server, operation(&args), exec)
        },
    );
    options.is_concurrency_safe = Some(Arc::new(|_| true));
    define_tool(options)
}

/// Register the three shared resource tools in `ctx`'s scope.
pub fn register_resource_tools(
    ctx: &Context,
    request: impl Fn(String, McpResourceRequest, Arc<ToolExecution>) -> McpResourceFuture<'static>
    + Send
    + Sync
    + 'static,
) -> Result<Disposer, BoxError> {
    let tools = ctx
        .get::<Tools>()
        .ok_or_else(|| -> BoxError { "mcp-resources: ctx.tools is unavailable".into() })?;
    let request: Arc<RequestResource> = Arc::new(request);
    register_resource_tools_with(&tools, ctx, request)
}

fn register_resource_tools_with(
    tools: &Service<Tools>,
    ctx: &Context,
    request: Arc<RequestResource>,
) -> Result<Disposer, BoxError> {
    ctx.try_effect("mcpResources.resourceTools", |undos| -> Result<(), BoxError> {
        let list = resource_tool(
            "list_mcp_resources",
            "List resources available from an MCP server.",
            list_parameters(),
            request.clone(),
            |args| McpResourceRequest::List {
                cursor: cursor_from(args),
            },
        )?;
        undos.push(tools.register(
            ctx,
            list,
            ToolRegistrationOptions::default(),
        )?)?;

        let templates = resource_tool(
            "list_mcp_resource_templates",
            "List parameterized resource URI templates from an MCP server.",
            list_parameters(),
            request.clone(),
            |args| McpResourceRequest::ListTemplates {
                cursor: cursor_from(args),
            },
        )?;
        undos.push(tools.register(
            ctx,
            templates,
            ToolRegistrationOptions::default(),
        )?)?;

        let read = resource_tool(
            "read_mcp_resource",
            "Read an MCP resource by URI from the named server. Use a listed URI or an expanded resource template.",
            json!({
                "server": {
                    "type": "string",
                    "required": true,
                    "description": "Configured MCP server name."
                },
                "uri": {
                    "type": "string",
                    "required": true,
                    "description": "Resource URI to read."
                }
            }),
            request,
            |args| McpResourceRequest::Read {
                uri: args
                    .get("uri")
                    .and_then(Value::as_str)
                    .expect("validated URI argument")
                    .to_owned(),
            },
        )?;
        undos.push(tools.register(
            ctx,
            read,
            ToolRegistrationOptions::default(),
        )?)?;
        Ok(())
    })
}

fn project_binary(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(project_binary).collect()),
        Value::Object(entries) => Value::Object(
            entries
                .iter()
                .map(|(key, item)| {
                    let projected = if key == "blob" {
                        item.as_str().map(|blob| {
                            Value::String(format!(
                                "[binary resource: {} base64 characters; available to programmatic callers]",
                                blob.encode_utf16().count()
                            ))
                        })
                    } else {
                        None
                    };
                    (key.clone(), projected.unwrap_or_else(|| project_binary(item)))
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Render resource JSON while retaining raw binary data only for programmatic callers.
pub fn render_resource_result(server: &str, value: &Value) -> Vec<ContentBlock> {
    let rendered = js_json_stringify(&project_binary(value));
    vec![ContentBlock::text(format!(
        "MCP server: {server}\n{rendered}"
    ))]
}
