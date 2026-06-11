use http_parity::{
    assert_expected_response, assert_invalid_validation_response, http_client,
    send_invalid_validation_request, send_request, spawn_server, Backend, Scenario,
};

#[tokio::test]
async fn parity_scenarios_match_expected_contract() {
    for backend in Backend::ALL {
        for scenario in Scenario::ALL {
            let server = spawn_server(backend, scenario).await.unwrap();
            let client = http_client();
            let base_uri = server.base_uri().unwrap();
            let response = send_request(&client, &base_uri, scenario).await.unwrap();
            assert_expected_response(scenario, &response).unwrap();
            server.shutdown().await.unwrap();
        }
    }
}

#[tokio::test]
async fn invalid_validation_payload_matches_contract() {
    for backend in Backend::ALL {
        let server = spawn_server(backend, Scenario::JsonValidate).await.unwrap();
        let client = http_client();
        let base_uri = server.base_uri().unwrap();
        let response = send_invalid_validation_request(&client, &base_uri)
            .await
            .unwrap();
        assert_invalid_validation_response(&response).unwrap();
        server.shutdown().await.unwrap();
    }
}
