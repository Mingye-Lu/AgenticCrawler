mod fixtures;

#[tokio::test]
async fn fixture_server_starts_and_responds() {
    let server = fixtures::TestServer::start().await;

    let resp = reqwest::get(server.url("/health"))
        .await
        .expect("request failed");
    assert_eq!(resp.status(), 200);

    let body = resp.text().await.unwrap();
    assert_eq!(body, "ok");
}

#[tokio::test]
async fn fixture_server_network_basic_serves_html() {
    let server = fixtures::TestServer::start().await;

    let resp = reqwest::get(server.url("/network-basic"))
        .await
        .expect("request failed");
    assert_eq!(resp.status(), 200);

    let body = resp.text().await.unwrap();
    assert!(
        body.contains("fetch"),
        "network-basic page should contain fetch calls"
    );
}
