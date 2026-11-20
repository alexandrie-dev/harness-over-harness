use hoh_tool_subagent::model_selection::{
    AllowedModelRoute, DelegationModelRequest, assert_allowed_model_routes,
    has_delegation_model_request,
};

#[test]
fn route_and_request_boundaries_are_exact() {
    let route = AllowedModelRoute {
        provider: "provider".into(),
        model: "model".into(),
    };
    assert_allowed_model_routes(std::slice::from_ref(&route)).unwrap();
    assert!(assert_allowed_model_routes(&[route.clone(), route]).is_err());
    assert!(!has_delegation_model_request(
        &DelegationModelRequest::default()
    ));
    assert!(has_delegation_model_request(&DelegationModelRequest {
        provider: Some("provider".into()),
        ..Default::default()
    }));
}
