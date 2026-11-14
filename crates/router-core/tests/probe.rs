//! The router probe against a mock web server: what it reads, and what it never does.

use router_core::probe::{probe, report, Support};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

async fn page(s: &MockServer, p: &str, t: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(t)
        .mount(s)
        .await;
}

fn html(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(body.as_bytes().to_vec(), "text/html")
}

async fn seen(s: &MockServer) -> Vec<Request> {
    s.received_requests().await.unwrap()
}

/// Shaped like a TP-Link login page; the names are from public documentation, nothing real.
const LOGIN: &str = r#"<html><head><title>TL-MR6400</title>
<script src="../js/lib.js"></script></head>
<body><script>var hardwareVersion = "MR6400 v5.0"; var softwareVersion = "1.2.0 Build 20200101";</script></body></html>"#;

#[tokio::test]
async fn reads_a_tp_link_shaped_router_without_logging_in() {
    let s = MockServer::start().await;
    page(
        &s,
        "/",
        ResponseTemplate::new(302).insert_header("Location", "/webpages/login.html"),
    )
    .await;
    page(&s, "/webpages/login.html", html(LOGIN)).await;
    page(
        &s,
        "/js/lib.js",
        ResponseTemplate::new(200).set_body_raw(
            br#"$.ajax({url:"/cgi_gdpr"}); var other = "TL-WR999N";"#.to_vec(),
            "application/javascript",
        ),
    )
    .await;

    let f = probe(&s.uri()).await;
    assert_eq!(f.support(), Support::Unknown);
    assert_eq!(f.vendor.as_deref(), Some("TP-Link"));
    assert_eq!(f.model.as_deref(), Some("TL-MR6400"));
    assert_eq!(f.hardware.as_deref(), Some("MR6400 v5.0"));
    assert_eq!(f.firmware.as_deref(), Some("1.2.0 Build 20200101"));
    assert!(f.api_paths.contains("/cgi_gdpr"));

    // GET only, with nothing that could log in.
    for r in seen(&s).await {
        assert_eq!(r.method.as_str(), "GET", "{}", r.url);
        assert!(r.headers.get("cookie").is_none());
        assert!(r.headers.get("authorization").is_none());
        assert!(r.body.is_empty());
    }

    let text = report(&f, "0.2.0", 1).unwrap();
    assert!(
        !text.contains(&s.address().ip().to_string()),
        "the address stays out"
    );
    assert!(!text.contains("<html"), "no page contents");
}

#[tokio::test]
async fn stops_early_on_a_supported_model() {
    let s = MockServer::start().await;
    page(
        &s,
        "/api/device/basic_information",
        ResponseTemplate::new(200).set_body_raw(
            b"<response><devicename>H122-373</devicename></response>".to_vec(),
            "text/xml",
        ),
    )
    .await;
    let f = probe(&s.uri()).await;
    assert_eq!(f.support(), Support::Supported);
    assert_eq!(seen(&s).await.len(), 1);
}

#[tokio::test]
async fn another_huawei_is_unknown_with_its_model() {
    let s = MockServer::start().await;
    page(
        &s,
        "/api/device/basic_information",
        ResponseTemplate::new(200).set_body_raw(
            b"<response><devicename>H158-381</devicename></response>".to_vec(),
            "text/xml",
        ),
    )
    .await;
    let f = probe(&s.uri()).await;
    assert_eq!(f.support(), Support::Unknown);
    assert_eq!(f.model.as_deref(), Some("H158-381"));
}

#[tokio::test]
async fn never_leaves_the_routers_address() {
    let other = MockServer::start().await;
    page(&other, "/evil.js", html("x")).await;
    let s = MockServer::start().await;
    let away = format!("{}/evil.js", other.uri());
    page(
        &s,
        "/",
        ResponseTemplate::new(302).insert_header("Location", away.as_str()),
    )
    .await;
    page(
        &s,
        "/index.html",
        html(&format!(
            r#"<script src="{away}"></script><script>location.href="{away}"</script>"#
        )),
    )
    .await;
    probe(&s.uri()).await;
    assert!(seen(&other).await.is_empty());
}

#[tokio::test]
async fn keeps_to_its_budget_on_a_page_that_links_everywhere() {
    let s = MockServer::start().await;
    let links: String = (0..200)
        .map(|i| format!(r#"<script src="/s{i}.js"></script>"#))
        .collect();
    page(&s, "/", html(&links)).await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(b"x".to_vec(), "application/javascript"),
        )
        .mount(&s)
        .await;
    probe(&s.uri()).await;
    assert!(seen(&s).await.len() <= 14, "{}", seen(&s).await.len());
}

#[tokio::test]
async fn a_page_with_nothing_useful_is_still_reportable() {
    let s = MockServer::start().await;
    page(&s, "/", html("<html><title>Welcome</title></html>")).await;
    let f = probe(&s.uri()).await;
    assert_eq!(f.support(), Support::Unknown);
    assert_eq!(f.model, None);
    let v: serde_json::Value = serde_json::from_str(&report(&f, "0.2.0", 1).unwrap()).unwrap();
    assert!(v["model"].is_null());
    assert_eq!(v["titles"][0], "Welcome");
}

#[tokio::test]
async fn nothing_answering_is_unreachable() {
    let f = probe("http://127.0.0.1:9").await;
    assert_eq!(f.support(), Support::Unreachable);
    assert!(f.pages.is_empty());
}
