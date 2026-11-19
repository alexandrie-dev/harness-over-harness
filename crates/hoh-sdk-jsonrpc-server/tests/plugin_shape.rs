use hoh_kernel::Plugin;
use hoh_sdk_jsonrpc_server::{JsonRpcConfig, SdkJsonRpcServer};

#[test]
fn has_the_native_plugin_shape_with_the_source_row_name_and_config() {
    assert_eq!(SdkJsonRpcServer::NAME, "sdk-jsonrpc-server");
    assert!(!JsonRpcConfig::default().max_tokens_as_success);
    let schema = schemars::schema_for!(JsonRpcConfig);
    let schema = serde_json::to_value(schema).unwrap();
    assert!(schema.to_string().contains("maxTokensAsSuccess"));
}
