//! Exercise selected transport revocation with runtime auth enabled.
#![cfg(test)]

use super::*;
use crate::client::ClientRouting;
use crate::client::RequestRouteTelemetry;
use codex_client::HttpTransport;
use codex_http_client::DestinationPolicy;
use codex_http_client::NetworkPolicyController;
use codex_http_client::NetworkPolicyDenied;
use codex_login::AuthCredentialsStoreMode;
use codex_login::AuthKeyringBackendKind;
use codex_login::AuthRouteConfig;
use pretty_assertions::assert_eq;

#[test_case::test_case(RuntimeAuthMode::DualHeader; "dual_header")]
#[test_case::test_case(RuntimeAuthMode::AssertionOnly; "assertion_only")]
#[tokio::test]
async fn selected_runtime_transport_stays_revoked(mode: RuntimeAuthMode) {
    let server = MockServer::start().await;
    Mock::given(path("/register"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "runtime_agent_instance_id": "runtime-a", "agent_host_id": "host-a",
            "actor": {"kind": "platform", "user_id": "user-a", "organization_id": "org-a", "project_id": null},
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let controller = NetworkPolicyController::default();
    let policy = controller.policy();
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
        .with_network_policy(policy.clone());
    let home = TempDir::new().unwrap();
    codex_login::login_with_api_key(
        home.path(),
        "key-a",
        AuthCredentialsStoreMode::Ephemeral,
        AuthKeyringBackendKind::Direct,
    )
    .unwrap();
    let manager = Arc::new(
        AuthManager::new(
            home.path().to_path_buf(),
            /*enable_codex_api_key_env*/ false,
            AuthCredentialsStoreMode::Ephemeral,
            /*forced_chatgpt_workspace_id*/ None,
            /*chatgpt_base_url*/ None,
            AuthKeyringBackendKind::Direct,
            AuthRouteConfig::from_http_client_factory(factory.clone()),
        )
        .await,
    );
    assert!(controller.publish(policy.revision(), DestinationPolicy::Unrestricted));
    let runtime = Arc::new(RuntimeAuthManager::new(
        manager.clone(),
        AgentHostAuthorization::InstallationId {
            installation_id: uuid::Uuid::nil(),
        },
        RuntimeRegistrationMetadata {
            harness: None,
            harness_version: None,
            role: RuntimeRole::OrchestratorAndExecutor,
        },
        format!("{}/register", server.uri()).parse().unwrap(),
    ));
    let mut client = test_model_client(SessionSource::Exec);
    client.http_client_factory = factory;
    Arc::get_mut(&mut client.state).unwrap().provider = codex_model_provider::ProviderAuthContext {
        credentials: manager,
        runtime_auth: Some((runtime, mode)),
    }
    .model_provider(ModelProviderInfo::create_openai_provider(
        /*base_url*/ None,
    ));
    let mut setup = client
        .current_client_setup(ClientRouting::Workspace)
        .await
        .unwrap();
    setup.api_provider.base_url = format!("{}/v1", server.uri());
    let transport = setup.http_transport("/responses").unwrap();
    let request = setup
        .api_provider
        .build_request(http::Method::POST, "/responses");
    let request = setup.api_auth.apply_auth(request).await.unwrap();
    transport.execute(request.clone()).await.unwrap();

    // A new policy may allow the destination, but cannot authorize the old account's request.
    policy.invalidate();
    assert!(controller.publish(policy.revision(), DestinationPolicy::Unrestricted));
    assert!(matches!(
        transport.execute(request).await,
        Err(TransportError::Policy(NetworkPolicyDenied::Revoked))
    ));
    let auth_context = AuthRequestTelemetryContext::new(
        setup.auth.as_ref().map(CodexAuth::auth_mode),
        setup.api_auth.as_ref(),
        /*agent_identity_telemetry*/ None,
        PendingUnauthorizedRetry::default(),
    )
    .unwrap();
    let metadata = test_responses_metadata_for_client(
        &client,
        /*turn_id*/ None,
        "test-session".into(),
        /*parent_thread_id*/ None,
        TestCodexResponsesRequestKind::WebsocketConnection,
    );
    let result = client
        .connect_websocket(
            &test_session_telemetry(),
            setup.api_provider,
            setup.api_auth,
            &setup.http_client_factory,
            &metadata,
            auth_context,
            RequestRouteTelemetry::for_endpoint("/responses"),
            &http::HeaderMap::new(),
            /*parent_routing_affinity*/ None,
            /*reauth*/ false,
        )
        .await;
    assert!(matches!(
        result,
        Err(ApiError::Transport(TransportError::Policy(
            NetworkPolicyDenied::Revoked
        )))
    ));
    server.verify().await;
}

#[test_case::test_case(RuntimeAuthMode::DualHeader; "dual_header")]
#[test_case::test_case(RuntimeAuthMode::AssertionOnly; "assertion_only")]
#[tokio::test]
async fn request_owner_tracks_the_selected_explicit_key(mode: RuntimeAuthMode) {
    let server = MockServer::start().await;
    Mock::given(path("/register"))
        .respond_with(|request: &wiremock::Request| ResponseTemplate::new(200).set_body_json(json!({
            "runtime_agent_instance_id": request.headers["idempotency-key"].to_str().unwrap(),
            "agent_host_id": "host-a",
            "actor": {"kind": "platform", "user_id": "user-a", "organization_id": "org-a", "project_id": null},
        })))
        .expect(2)
        .mount(&server).await;
    let credentials = AuthManager::from_auth_for_testing(CodexAuth::from_api_key("saved-key"));
    let context = codex_model_provider::ProviderAuthContext {
        runtime_auth: Some((
            Arc::new(RuntimeAuthManager::new(
                credentials.clone(),
                AgentHostAuthorization::InstallationId {
                    installation_id: uuid::Uuid::nil(),
                },
                RuntimeRegistrationMetadata {
                    harness: None,
                    harness_version: None,
                    role: RuntimeRole::Orchestrator,
                },
                format!("{}/register", server.uri()).parse().unwrap(),
            )),
            mode,
        )),
        credentials,
    };
    let mut owners = Vec::new();
    for key in ["key-a", "key-b", "key-a"] {
        let mut info = ModelProviderInfo::create_openai_provider(/*base_url*/ None);
        info.experimental_bearer_token = Some(key.into());
        let mut client = test_model_client(SessionSource::Exec);
        Arc::get_mut(&mut client.state).unwrap().provider = context.model_provider(info);
        let owner = client.auth_owner();
        let setup = client
            .current_client_setup(ClientRouting::ConfiguredProvider)
            .await
            .unwrap();
        assert!(owner.is_some());
        assert_eq!(setup.auth_owner, owner);
        assert_eq!(setup.auth.unwrap().api_key(), Some(key));
        let mut headers = http::HeaderMap::new();
        setup.api_auth.add_auth_headers(&mut headers).unwrap();
        match mode {
            RuntimeAuthMode::DualHeader => {
                assert_eq!(
                    headers[http::header::AUTHORIZATION],
                    format!("Bearer {key}")
                );
                assert!(headers.contains_key("x-openai-agent-assertion"));
            }
            RuntimeAuthMode::AssertionOnly => {
                assert!(
                    headers[http::header::AUTHORIZATION]
                        .to_str()
                        .unwrap()
                        .starts_with("AgentAssertion ")
                );
                assert!(!headers.contains_key("x-openai-agent-assertion"));
            }
        }
        owners.push(owner);
    }
    assert_ne!(
        owners[0], owners[1],
        "different keys cannot share turn or connection ownership"
    );
    assert_eq!(
        owners[0], owners[2],
        "reselecting a key preserves its owner"
    );
    server.verify().await;
}

#[test_case::test_case(RuntimeAuthMode::DualHeader; "dual_header")]
#[test_case::test_case(RuntimeAuthMode::AssertionOnly; "assertion_only")]
#[tokio::test]
async fn provider_switch_does_not_reuse_original_runtime_auth(mode: RuntimeAuthMode) {
    let registration = MockServer::start().await;
    Mock::given(path("/register"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&registration)
        .await;
    let (client, _) = model_client_with_counting_attestation(true);
    let manager = client.auth_manager().unwrap();
    let runtime = Arc::new(RuntimeAuthManager::new(
        manager,
        AgentHostAuthorization::InstallationId {
            installation_id: uuid::Uuid::nil(),
        },
        RuntimeRegistrationMetadata {
            harness: None,
            harness_version: None,
            role: RuntimeRole::OrchestratorAndExecutor,
        },
        format!("{}/register", registration.uri()).parse().unwrap(),
    ));
    let mut client = client;
    Arc::get_mut(&mut client.state).unwrap().provider =
        codex_model_provider::test_support::runtime_model_provider(
            client.provider_info().clone(),
            client.auth_manager().unwrap(),
            (Arc::clone(&runtime), mode),
        );

    // The default-provider path still reuses its runtime and websocket cache owner.
    let same_provider = client.new_session_for_provider(Arc::clone(&client.state.provider));
    assert!(Arc::ptr_eq(&same_provider.client.state, &client.state));
    assert!(Arc::ptr_eq(
        &same_provider
            .client
            .state
            .provider
            .runtime_auth()
            .unwrap()
            .0,
        &runtime,
    ));

    // A different explicit provider must use its own bearer token, even when the original
    // client uses assertion-only runtime auth. Preparing this request must not register A.
    let mut alternate_info = create_oss_provider_with_base_url(
        &format!("{}/provider-b/v1", registration.uri()),
        WireApi::Responses,
    );
    alternate_info.experimental_bearer_token = Some("provider-b-secret".into());
    let alternate = client.new_session_for_provider(create_model_provider(alternate_info, None));
    let setup = alternate
        .client
        .current_client_setup(ClientRouting::ConfiguredProvider)
        .await
        .unwrap();
    let headers = setup
        .api_auth
        .apply_auth_headers(http::HeaderMap::new())
        .await
        .unwrap();
    assert_eq!(
        headers[http::header::AUTHORIZATION],
        "Bearer provider-b-secret"
    );
    assert!(!headers.contains_key("x-openai-agent-assertion"));
    assert_eq!(setup.runtime_id, None);
    assert!(alternate.client.state.provider.runtime_auth().is_none());
    assert_eq!(
        setup.api_provider.base_url,
        format!("{}/provider-b/v1", registration.uri())
    );
    assert!(registration.received_requests().await.unwrap().is_empty());
    registration.verify().await;
}
