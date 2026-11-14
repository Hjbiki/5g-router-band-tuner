//! The read-only page-script saver: path rules, unpacking, redaction, limits, and that only GETs
//! go out.

use std::io::Write;
use std::time::Duration;

use reqwest::Url;
use router_core::drivers::huawei::HuaweiH122Driver;
use router_core::drivers::mock::{mock_error_page, MockDriver, MockScenario};
use router_core::page_scripts::{
    collect, extract_paths, is_allowed_path, is_error_page, maybe_gunzip, redact, still_flagged,
    validate_path, Limits, SkipKind, PAGE_NOT_FOUND, PAGE_NOT_LOGGED_IN, PAGE_PATH_REFUSED,
};
use router_core::{Credentials, RouterDriver, RouterError};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ORIGIN: &str = "http://192.168.8.1";

fn base(page: &str) -> Url {
    Url::parse(ORIGIN).unwrap().join(page).unwrap()
}

fn gz(bytes: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(bytes).unwrap();
    e.finish().unwrap()
}

async fn mock() -> MockDriver {
    let m = MockDriver::new(MockScenario::zain_like());
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    m
}

fn fast() -> Limits {
    Limits {
        gap: Duration::ZERO,
        ..Limits::default()
    }
}

#[test]
fn paths_resolve_on_the_same_router_under_the_allowed_folders() {
    let page = base("/html/content.html");
    assert_eq!(
        validate_path("/js/main.js", &page).as_deref(),
        Some("/js/main.js")
    );
    assert_eq!(
        validate_path("home.html", &page).as_deref(),
        Some("/html/home.html")
    );
    assert_eq!(
        validate_path("./menu.js", &page).as_deref(),
        Some("/html/menu.js")
    );
    assert_eq!(
        validate_path(" /lib/a.js ", &page).as_deref(),
        Some("/lib/a.js")
    );
    // Only the three folders, and only pages and scripts.
    assert_eq!(validate_path("/api/device/control", &page), None);
    assert_eq!(validate_path("/css/main.css", &page), None);
    assert_eq!(validate_path("/js/logo.png", &page), None);
    assert_eq!(validate_path("/config/global/config.xml", &page), None);
}

#[test]
fn traversal_in_any_spelling_is_refused() {
    let page = base("/html/content.html");
    for raw in [
        "../js/main.js",
        "/js/../api/x.js",
        "/js/%2e%2e/x.js",
        "/js/%2E%2E/x.js",
        "/js/%252e%252e/x.js",
        "/js/..%2fapi.js",
        "/js\\..\\x.js",
        "/js/%5c/x.js",
        "/js/%zz.js",
    ] {
        assert_eq!(validate_path(raw, &page), None, "{raw}");
    }
}

#[test]
fn other_hosts_and_schemes_are_refused() {
    let page = base("/html/content.html");
    for raw in [
        "http://203.0.113.9/js/x.js",
        "https://192.168.8.1/js/x.js",
        "//mock.example/js/x.js",
        "/js//x.js",
        "javascript:alert(1).js",
        "data:text/javascript,x.js",
        "http://192.168.8.1:8080/js/x.js",
        "/js/x y.js",
    ] {
        assert_eq!(validate_path(raw, &page), None, "{raw}");
    }
}

#[test]
fn the_query_and_fragment_are_dropped() {
    let page = base("/html/content.html");
    assert_eq!(
        validate_path("/js/a.js?v=2#top", &page).as_deref(),
        Some("/js/a.js")
    );
    assert_eq!(
        validate_path("/js/a.js#x", &page).as_deref(),
        Some("/js/a.js")
    );
    // An encoded `?` is not a query: the decoded name fails the whitelist.
    assert_eq!(validate_path("/js/a%3f.js", &page), None);
}

#[test]
fn the_driver_check_matches_the_rules() {
    assert!(is_allowed_path("/html/content.html"));
    assert!(is_allowed_path("/lib/jquery-1.0.min.js"));
    assert!(!is_allowed_path("html/content.html"));
    assert!(!is_allowed_path("/js/../x.js"));
    assert!(!is_allowed_path("/js//x.js"));
    assert!(!is_allowed_path("/api/net/plmn-list"));
    assert!(!is_allowed_path("/js/a.js?x=1"));
}

#[test]
fn references_come_from_attributes_and_folder_literals() {
    let html = r#"<script src="/js/a.js"></script><script src='b.js'></script>
        <a href="/html/x.html">x</a><img src="/img/logo.png">
        <script>var p = "../js/c.js"; load('lib/d.js'); var t = "plain.js"; var u = `/html/e.html?x`;</script>"#;
    let found = extract_paths(html, &base("/html/content.html"));
    assert_eq!(
        found.paths,
        vec![
            "/js/a.js",
            "/html/b.js",
            "/html/x.html",
            "/html/lib/d.js",
            "/html/e.html"
        ]
    );
    assert_eq!(found.rejected, vec!["../js/c.js"]);
}

#[test]
fn gzip_is_unpacked_by_its_magic_whatever_the_headers() {
    let plain = b"function a() { return 1; }".to_vec();
    assert_eq!(
        maybe_gunzip(plain.clone(), 1024).unwrap(),
        (plain.clone(), false)
    );
    assert_eq!(maybe_gunzip(gz(&plain), 1024).unwrap(), (plain, true));
    // Starts like gzip but isn't.
    assert!(maybe_gunzip(vec![0x1f, 0x8b, 0, 1, 2, 3], 1024).is_err());
}

#[test]
fn a_zip_bomb_stops_at_the_cap() {
    let bomb = gz(&vec![b'a'; 5 * 1024 * 1024]);
    assert!(bomb.len() < 64 * 1024);
    let err = maybe_gunzip(bomb, 4 * 1024 * 1024).unwrap_err();
    assert_eq!(err, SkipKind::TooLargeUnpacked);
    assert!(err.text().contains("too large"));
    assert!(maybe_gunzip(vec![b'a'; 2048], 1024).is_err());
}

#[test]
fn the_short_error_page_counts_as_missing() {
    let page = mock_error_page();
    assert_eq!(page.len(), 88);
    assert!(is_error_page("/js/none.js", &page));
    // A real page, or a short script, is not.
    assert!(!is_error_page("/html/x.html", &page));
    assert!(!is_error_page("/js/tiny.js", b"var a = 1;"));
}

#[test]
fn csrf_tokens_and_identifiers_are_redacted() {
    let page = concat!(
        "<meta name=\"csrf_token\" content=\"AbCdEf0123456789AbCdEf0123456789\">\n",
        "<meta content='ZyXw9876543210ZyXw9876543210' name='csrf_token'/>\n",
        "<input type=\"hidden\" id=\"__RequestVerificationToken\" value=\"Tok123\">\n",
        "<meta name=\"viewport\" content=\"width=device-width\">\n",
        "<script>headers['__RequestVerificationToken'] = token; var g = { csrf_token: \"Q1w2e3r4\" };",
        " var imei = '356789012345678'; var gw = '10.1.2.3';</script>\n",
    );
    let (out, n) = redact(page);
    assert!(!out.contains("AbCdEf0123"));
    assert!(!out.contains("ZyXw98"));
    assert!(!out.contains("Tok123"));
    assert!(!out.contains("Q1w2e3r4"));
    assert!(!out.contains("356789012345678"));
    assert!(!out.contains("10.1.2.3"));
    assert!(out.contains("content=\"width=device-width\""), "{out}");
    assert!(
        out.contains("headers['__RequestVerificationToken'] = token"),
        "{out}"
    );
    assert!(out.contains("[redacted-number]") && out.contains("[redacted-ipv4]"));
    assert_eq!(n, 6);
    assert!(still_flagged(&out).is_empty());
}

#[test]
fn a_number_hidden_after_an_escape_is_caught_by_the_last_scan() {
    let (out, n) = redact("var note = \"line\\n356789012345678\";");
    assert_eq!(n, 0, "the token scan doesn't see it");
    assert!(!still_flagged(&out).is_empty());
}

#[tokio::test(start_paused = true)]
async fn collects_the_mock_pages_with_gets_only() {
    let m = mock().await;
    let b = collect(&m, ORIGIN, &fast()).await.unwrap();
    let paths: Vec<&str> = b.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec![
            "/html/content.html",
            "/js/main.js",
            "/lib/mock-lib.js",
            "/html/settings.html",
            "/js/settings.js",
            "/js/deep.js",
        ]
    );
    // Depth 3 is the last one fetched; its links aren't followed.
    assert!(!m.web_gets().iter().any(|p| p == "/js/too-deep.js"));
    assert_eq!(b.missing, vec!["/js/missing.js"]);
    let file = |p: &str| b.files.iter().find(|f| f.path == p).unwrap();
    assert!(file("/lib/mock-lib.js").gunzipped);
    let content = String::from_utf8(file("/html/content.html").bytes.clone()).unwrap();
    assert!(!content.contains("MockCsrfToken"));
    assert!(!content.contains("203.0.113.9"));
    let main = String::from_utf8(file("/js/main.js").bytes.clone()).unwrap();
    assert!(!main.contains("000000000000000"));
    assert_eq!(file("/js/main.js").redactions, 1);
    // The file only the last scan catches is left out, with its reason.
    assert!(b
        .skipped
        .iter()
        .any(|(p, why)| p == "/js/hidden.js" && *why == SkipKind::StillFlagged));
    // References that failed the rules are listed, never fetched.
    let refs: Vec<&str> = b.skipped.iter().map(|(p, _)| p.as_str()).collect();
    assert!(refs.contains(&"../js/outside.js"));
    assert!(refs.contains(&"/js/%2e%2e/escape.js"));
    assert!(refs.contains(&"//mock.example/js/elsewhere.js"));
    assert!(refs.contains(&"http://[redacted-ipv4]/js/elsewhere.js"));
    for p in m.web_gets() {
        assert!(is_allowed_path(&p), "{p}");
    }
    assert_eq!(m.writes(), 0, "nothing but GETs");
    assert_eq!(b.stopped, None);
}

#[tokio::test(start_paused = true)]
async fn the_count_and_size_caps_end_the_run() {
    let m = mock().await;
    let b = collect(
        &m,
        ORIGIN,
        &Limits {
            max_files: 2,
            ..fast()
        },
    )
    .await
    .unwrap();
    assert_eq!(b.files.len(), 2);
    assert!(b.stopped.unwrap().contains("file limit"));

    let b = collect(
        &m,
        ORIGIN,
        &Limits {
            max_total_bytes: 700,
            ..fast()
        },
    )
    .await
    .unwrap();
    assert!(b.total_bytes() <= 700);
    assert!(b.stopped.unwrap().contains("size limit"));
}

#[tokio::test(start_paused = true)]
async fn the_time_budget_ends_the_run() {
    let m = mock().await;
    let b = collect(
        &m,
        ORIGIN,
        &Limits {
            gap: Duration::from_secs(10),
            budget: Duration::from_secs(15),
            ..Limits::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(b.files.len(), 2);
    assert_eq!(b.stopped.as_deref(), Some("the time limit"));
}

#[tokio::test(start_paused = true)]
async fn logged_out_reads_nothing() {
    let m = MockDriver::new(MockScenario::zain_like());
    let err = collect(&m, ORIGIN, &fast()).await.unwrap_err();
    assert_eq!(
        err,
        RouterError::Validation {
            message: PAGE_NOT_LOGGED_IN.into()
        }
    );
    assert!(m.web_gets().is_empty());
}

#[tokio::test]
async fn the_mock_refuses_paths_outside_the_folders() {
    let m = mock().await;
    let err = m.web_page("/api/device/control").await.unwrap_err();
    assert_eq!(
        err,
        RouterError::Validation {
            message: PAGE_PATH_REFUSED.into()
        }
    );
    assert!(m.web_gets().is_empty());
}

#[tokio::test]
async fn the_huawei_driver_sends_plain_gets_and_reads_a_redirect_as_logged_out() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/html/content.html"))
        .respond_with(ResponseTemplate::new(307).insert_header("Location", "/html/index.html"))
        .mount(&server)
        .await;
    let packed = gz(b"var a = 1;");
    Mock::given(method("GET"))
        .and(path("/js/a.js"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(packed.clone()))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/js/none.js"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let d = HuaweiH122Driver::new(&server.uri()).unwrap();

    assert_eq!(
        d.web_page("/html/content.html").await.unwrap_err(),
        RouterError::Validation {
            message: PAGE_NOT_LOGGED_IN.into()
        }
    );
    // The bytes come back as sent; unpacking is the saver's job.
    assert_eq!(d.web_page("/js/a.js").await.unwrap(), packed);
    assert_eq!(
        d.web_page("/js/none.js").await.unwrap_err(),
        RouterError::Validation {
            message: PAGE_NOT_FOUND.into()
        }
    );
    // Refused before any request.
    for bad in ["/api/device/control", "/js/../api/x.js", "/js/a.js?x=1"] {
        assert_eq!(
            d.web_page(bad).await.unwrap_err(),
            RouterError::Validation {
                message: PAGE_PATH_REFUSED.into()
            }
        );
    }
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    for r in &requests {
        assert_eq!(r.method.as_str(), "GET");
        assert!(r.headers.get("X-Requested-With").is_none());
        assert!(r.headers.get("__RequestVerificationToken").is_none());
    }
}
