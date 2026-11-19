# hoh-llm-deepseek

Rust port of `@deepseek-ai/dsh-llm-deepseek`. The crate implements the DeepSeek
Anthropic-Messages-compatible adapter used by HoH: configuration and model
discovery, request serialization, native reasoning replay, SSE translation,
vision projection/pricing, Files API uploads, and credential-scoped listing
and upload caches.

The main integration point is `DeepSeekAdapter`, which implements
`hoh_llm::LlmAdapter`. Construct a validated connection with
`resolve_adapter_options`, attach authentication headers and an attachment
store, and register it through `register_deep_seek_provider`.

No network operation is performed while constructing or resolving the
configuration. Network access begins only in model-listing, Files API, or
stream calls.
