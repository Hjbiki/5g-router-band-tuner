use router_core::drivers::huawei::scram::ScramClient;
use router_core::drivers::huawei::session::HuaweiSession;
use router_core::{ApiErrorKind, Credentials, LoginGate, RouterError, RouterIdentity};
use serde_json::Value;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const STATE_LOGIN: &str = "/api/user/state-login";
const TOKEN: &str = "/api/webserver/token";
const CHALLENGE: &str = "/api/user/challenge_login";
const AUTH: &str = "/api/user/authentication_login";
const OK_BODY: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<response>OK</response>";

fn fixture(n: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/huawei_h122/{n}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

fn vector(name: &str) -> Value {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/scram_vectors.json"
    );
    let all: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    all.into_iter().find(|v| v["name"] == name).unwrap()
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap()
}

fn token_headers() -> Value {
    serde_json::from_str(&fixture("token_headers.json")).unwrap()
}

/// The last 32 characters of the fixture's `<token>`.
fn fixture_token_tail() -> String {
    let body = fixture("webserver_token.xml");
    let a = body.find("<token>").unwrap() + "<token>".len();
    let b = body.find("</token>").unwrap();
    let t = &body[a..b];
    assert_eq!(t.len(), 64);
    t[32..].to_string()
}

fn header<'a>(r: &'a Request, name: &str) -> Option<&'a str> {
    r.headers.get(name).map(|v| v.to_str().unwrap())
}

fn body(r: &Request) -> String {
    String::from_utf8(r.body.clone()).unwrap()
}

fn tag(xml: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let a = xml.find(&open).unwrap() + open.len();
    let b = xml[a..].find(&format!("</{name}>")).unwrap();
    xml[a..a + b].to_string()
}

async fn requests_to(server: &MockServer, m: &str, p: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == m && r.url.path() == p)
        .collect()
}

async fn mount_get(server: &MockServer, p: &str, body: String) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

async fn mount_token(server: &MockServer) {
    mount_get(server, TOKEN, fixture("webserver_token.xml")).await;
}

fn challenge_body(v: &Value) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><salt>{}</salt><modeselected>1</modeselected><servernonce>{}</servernonce><newType>0</newType><iterations>{}</iterations></response>",
        s(v, "salt"),
        s(v, "server_nonce"),
        v["iterations"].as_u64().unwrap()
    )
}

fn auth_body(sig: &str) -> String {
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><serversignature>{sig}</serversignature></response>")
}

fn auth_response(sig: &str) -> ResponseTemplate {
    let h = &token_headers()["authentication_login"];
    let mut t = ResponseTemplate::new(200).set_body_string(auth_body(sig));
    for name in [
        "__RequestVerificationToken",
        "__RequestVerificationTokenone",
        "__RequestVerificationTokentwo",
    ] {
        t = t.insert_header(name, s(h, name));
    }
    t
}

/// Mounts state-login (logged out), token and challenge_login for the ascii vector.
async fn mount_login_prelude(server: &MockServer, v: &Value) {
    mount_get(server, STATE_LOGIN, fixture("state_login_logged_out.xml")).await;
    mount_token(server).await;
    let ch = &token_headers()["challenge_login"];
    Mock::given(method("POST"))
        .and(path(CHALLENGE))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(challenge_body(v))
                .insert_header(
                    "__RequestVerificationToken",
                    s(ch, "__RequestVerificationToken"),
                ),
        )
        .mount(server)
        .await;
}

fn creds(v: &Value) -> Credentials {
    Credentials::new("admin", s(v, "password"))
}

fn scram(v: &Value) -> ScramClient {
    ScramClient::with_nonce(s(v, "first_nonce").to_string())
}

#[tokio::test]
async fn login_happy_path() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(auth_response(s(&v, "serversignature")))
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap();

    let ch = requests_to(&server, "POST", CHALLENGE).await;
    assert_eq!(ch.len(), 1);
    let c = &ch[0];
    assert_eq!(
        header(c, "__RequestVerificationToken"),
        Some(fixture_token_tail().as_str())
    );
    assert_eq!(header(c, "X-Requested-With"), Some("XMLHttpRequest"));
    assert_eq!(header(c, "_ResponseSource"), Some("Broswer"));
    assert_eq!(
        header(c, "Content-Type"),
        Some("application/x-www-form-urlencoded; charset=UTF-8")
    );
    let cb = body(c);
    assert!(cb.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?><request>"));
    assert!(cb.contains("<mode>1</mode>"));
    assert!(cb.contains("<username>admin</username>"));
    assert_eq!(tag(&cb, "firstnonce"), s(&v, "first_nonce"));

    let au = requests_to(&server, "POST", AUTH).await;
    assert_eq!(au.len(), 1);
    let ab = body(&au[0]);
    assert_eq!(tag(&ab, "clientproof"), s(&v, "clientproof"));
    assert_eq!(tag(&ab, "finalnonce"), s(&v, "server_nonce"));
    // The challenge response's header token is used for authentication_login.
    assert_eq!(
        header(&au[0], "__RequestVerificationToken"),
        Some(s(
            &token_headers()["challenge_login"],
            "__RequestVerificationToken"
        ))
    );

    // one/two are preferred over the 32-token pool.
    assert_eq!(session.queued_tokens(), 2);
}

/// The H122 only issues its `SessionID` cookie with the web page, and a token fetched without
/// that cookie is refused with 125003. Login must open the page first, like the browser does.
#[tokio::test]
async fn login_opens_web_session_before_token() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("<html></html>")
                .insert_header("Set-Cookie", "SessionID=web-session; path=/; HttpOnly;"),
        )
        .mount(&server)
        .await;
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(auth_response(s(&v, "serversignature")))
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap();

    let all = server.received_requests().await.unwrap();
    assert_eq!(all[0].url.path(), "/", "the web page is loaded first");
    for p in [TOKEN, CHALLENGE] {
        let r = all.iter().find(|r| r.url.path() == p).unwrap();
        assert_eq!(header(r, "Cookie"), Some("SessionID=web-session"), "{p}");
    }
}

#[tokio::test]
async fn pool_used_without_one_two() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/test/pool"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(OK_BODY)
                .insert_header("__RequestVerificationToken", "aaa#bbb#ccc"),
        )
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    let before = session.queued_tokens();
    session.post("/api/test/pool", "<x>1</x>").await.unwrap();
    assert_eq!(session.queued_tokens(), before + 3);
}

#[tokio::test]
async fn login_locked_out_makes_no_post() {
    let server = MockServer::start().await;
    let locked = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><State>-1</State><lockstatus>1</lockstatus><remainwaittime>120</remainwaittime></response>";
    mount_get(&server, STATE_LOGIN, locked.to_string()).await;
    mount_token(&server).await;

    let v = vector("ascii_password");
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LockedOut { wait_s: 120 });

    let posts = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(posts, 0);
}

#[tokio::test]
async fn login_bad_password_is_terminal() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>108006</code><message></message></error>",
        ))
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::BadCredentials);
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 1);
    assert_eq!(requests_to(&server, "POST", CHALLENGE).await.len(), 1);
}

#[tokio::test]
async fn login_already_logged_in_elsewhere_maps_to_takeover() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>108003</code></error>",
        ))
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::SessionTakenOver);
}

#[tokio::test]
async fn login_too_many_attempts_maps_to_locked_out() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>108007</code></error>",
        ))
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LockedOut { wait_s: 0 });
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 1);
}

#[tokio::test]
async fn bad_server_signature() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    let wrong = "00".repeat(32);
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(auth_response(&wrong))
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LoginAlgorithmMismatch);
    // Tokens from an unverified server are not kept.
    assert_eq!(session.queued_tokens(), 0);
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 1);
}

#[tokio::test]
async fn post_consumes_one_token_each() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/test/a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(OK_BODY)
                .insert_header("__RequestVerificationToken", "tok1#tok2#tok3"),
        )
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    session.post("/api/test/a", "<x>1</x>").await.unwrap();
    session.post("/api/test/a", "<x>2</x>").await.unwrap();

    let posts = requests_to(&server, "POST", "/api/test/a").await;
    assert_eq!(posts.len(), 2);
    let t0 = header(&posts[0], "__RequestVerificationToken").unwrap();
    let t1 = header(&posts[1], "__RequestVerificationToken").unwrap();
    assert_eq!(t0, fixture_token_tail());
    assert_eq!(t1, "tok1");
    assert_ne!(t0, t1);
    // 3 harvested, then 1 consumed, then 3 harvested again.
    assert_eq!(session.queued_tokens(), 5);
    assert_eq!(requests_to(&server, "GET", TOKEN).await.len(), 1);
}

/// Primes the queue with one harvested token so the measured POST doesn't need a fetch.
async fn primed_session(server: &MockServer) -> HuaweiSession {
    Mock::given(method("POST"))
        .and(path("/api/test/prime"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(OK_BODY)
                .insert_header("__RequestVerificationToken", "primed0000"),
        )
        .mount(server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    session.post("/api/test/prime", "<x>0</x>").await.unwrap();
    assert_eq!(session.queued_tokens(), 1);
    session
}

#[tokio::test]
async fn post_token_error_retries_once() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    let session = primed_session(&server).await;
    let gets_before = requests_to(&server, "GET", TOKEN).await.len();

    Mock::given(method("POST"))
        .and(path("/api/test/write"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>125003</code></error>",
        ))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/test/write"))
        .respond_with(ResponseTemplate::new(200).set_body_string(OK_BODY))
        .with_priority(2)
        .mount(&server)
        .await;

    let body = session.post("/api/test/write", "<x>1</x>").await.unwrap();
    assert!(body.contains("OK"));
    let posts = requests_to(&server, "POST", "/api/test/write").await;
    assert_eq!(posts.len(), 2);
    assert_eq!(
        header(&posts[0], "__RequestVerificationToken"),
        Some("primed0000")
    );
    assert_eq!(
        header(&posts[1], "__RequestVerificationToken"),
        Some(fixture_token_tail().as_str())
    );
    assert_eq!(
        requests_to(&server, "GET", TOKEN).await.len() - gets_before,
        1
    );
}

#[tokio::test]
async fn post_token_error_twice_is_session_lost() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    let session = primed_session(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/test/write"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>125002</code></error>",
        ))
        .mount(&server)
        .await;

    let err = session
        .post("/api/test/write", "<x>1</x>")
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::SessionLost { code: Some(125002) });
    assert_eq!(
        requests_to(&server, "POST", "/api/test/write").await.len(),
        2
    );
}

#[tokio::test]
async fn error_in_http_200_surfaces() {
    let server = MockServer::start().await;
    mount_get(
        &server,
        "/api/test/get",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100005</code><message></message></error>"
            .to_string(),
    )
    .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session.get("/api/test/get").await.unwrap_err();
    assert_eq!(
        err,
        RouterError::Api {
            code: 100005,
            kind: ApiErrorKind::BadParam
        }
    );
}

#[tokio::test]
async fn get_sends_huawei_headers_and_returns_body() {
    let server = MockServer::start().await;
    mount_get(&server, "/api/user/heartbeat", fixture("heartbeat.xml")).await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let body = session.get("/api/user/heartbeat").await.unwrap();
    assert!(body.contains("<userlevel>2</userlevel>"));
    let r = &requests_to(&server, "GET", "/api/user/heartbeat").await[0];
    assert_eq!(header(r, "X-Requested-With"), Some("XMLHttpRequest"));
    assert_eq!(header(r, "_ResponseSource"), Some("Broswer"));
}

#[tokio::test]
async fn unreachable_maps() {
    let session = HuaweiSession::new("http://127.0.0.1:9").unwrap();
    let err = session.get("/api/user/heartbeat").await.unwrap_err();
    assert_eq!(err, RouterError::Unreachable);
}

#[tokio::test]
async fn debug_hides_tokens() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/test/a"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(OK_BODY)
                .insert_header("__RequestVerificationToken", "secrettokenvalue"),
        )
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    session.post("/api/test/a", "<x>1</x>").await.unwrap();
    let dbg = format!("{session:?}");
    assert!(!dbg.contains("secrettokenvalue"));
    assert!(!dbg.contains(&fixture_token_tail()));
}

#[tokio::test]
async fn login_when_already_logged_in_makes_no_post() {
    let server = MockServer::start().await;
    mount_get(&server, STATE_LOGIN, fixture("state_login_logged_in.xml")).await;
    mount_token(&server).await;
    let v = vector("ascii_password");
    let session = HuaweiSession::new(&server.uri()).unwrap();
    session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap();
    let posts = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST")
        .count();
    assert_eq!(posts, 0);
}

#[tokio::test]
async fn redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/a"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/api/b"))
        .mount(&server)
        .await;
    mount_get(&server, "/api/b", OK_BODY.to_string()).await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let r = session.get("/api/a").await;
    assert!(r.is_err(), "302 must not be followed or treated as success");
    assert!(requests_to(&server, "GET", "/api/b").await.is_empty());
}

const NO_RIGHTS: &str =
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100003</code><message></message></error>";

#[tokio::test]
async fn no_rights_on_get_is_session_lost() {
    let server = MockServer::start().await;
    mount_get(&server, "/api/device/signal", NO_RIGHTS.to_string()).await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session.get("/api/device/signal").await.unwrap_err();
    assert_eq!(err, RouterError::SessionLost { code: Some(100003) });
}

/// Observed on the H122: about 5 minutes after login the data GETs answer 125002. Treated as a
/// transient error, the app polled the dead session forever ("Reconnecting in 30 s").
#[tokio::test]
async fn token_error_on_get_is_session_lost() {
    let server = MockServer::start().await;
    for code in [125002, 125003] {
        let body =
            format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>{code}</code></error>");
        let p = format!("/api/device/signal{code}");
        mount_get(&server, &p, body).await;
        let session = HuaweiSession::new(&server.uri()).unwrap();
        let err = session.get(&p).await.unwrap_err();
        assert_eq!(
            err,
            RouterError::SessionLost { code: Some(code) },
            "code {code}"
        );
    }
}

#[tokio::test]
async fn no_rights_on_post_is_session_lost() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    let session = primed_session(&server).await;
    Mock::given(method("POST"))
        .and(path("/api/net/net-mode"))
        .respond_with(ResponseTemplate::new(200).set_body_string(NO_RIGHTS))
        .mount(&server)
        .await;
    let err = session
        .post("/api/net/net-mode", "<x>1</x>")
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::SessionLost { code: Some(100003) });
    // Not a token error: no second attempt.
    assert_eq!(
        requests_to(&server, "POST", "/api/net/net-mode")
            .await
            .len(),
        1
    );
}

#[tokio::test]
async fn unknown_auth_error_is_login_rejected() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>108009</code></error>",
        ))
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LoginRejected { code: 108009 });
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 1);
}

#[tokio::test]
async fn unknown_challenge_error_is_login_rejected() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_get(&server, STATE_LOGIN, fixture("state_login_logged_out.xml")).await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path(CHALLENGE))
        .respond_with(ResponseTemplate::new(200).set_body_string(NO_RIGHTS))
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LoginRejected { code: 100003 });
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 0);
}

const TOKEN_REFUSED: &str =
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>125003</code></error>";

/// A refused token on challenge_login (125003, seen on the first login of every fresh
/// process on the H122) is checked before any credential: refresh the web session and token
/// once and resend the challenge.
#[tokio::test]
async fn challenge_token_refused_refreshes_and_retries_once() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_get(&server, STATE_LOGIN, fixture("state_login_logged_out.xml")).await;
    mount_token(&server).await;
    // Mounted first, so it answers the first challenge only.
    Mock::given(method("POST"))
        .and(path(CHALLENGE))
        .respond_with(ResponseTemplate::new(200).set_body_string(TOKEN_REFUSED))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(auth_response(s(&v, "serversignature")))
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap();

    assert_eq!(requests_to(&server, "POST", CHALLENGE).await.len(), 2);
    assert_eq!(
        requests_to(&server, "GET", "/").await.len(),
        2,
        "web session refreshed"
    );
    assert_eq!(
        requests_to(&server, "GET", TOKEN).await.len(),
        2,
        "fresh token fetched"
    );
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 1);
}

#[tokio::test]
async fn challenge_token_refused_twice_is_rejected_without_proof() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_get(&server, STATE_LOGIN, fixture("state_login_logged_out.xml")).await;
    mount_token(&server).await;
    Mock::given(method("POST"))
        .and(path(CHALLENGE))
        .respond_with(ResponseTemplate::new(200).set_body_string(TOKEN_REFUSED))
        .mount(&server)
        .await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LoginRejected { code: 125003 });
    assert_eq!(
        requests_to(&server, "POST", CHALLENGE).await.len(),
        2,
        "one retry only"
    );
    assert_eq!(
        requests_to(&server, "POST", AUTH).await.len(),
        0,
        "no proof sent"
    );
}

fn identity_of(v: &Value) -> RouterIdentity {
    RouterIdentity::new(s(v, "salt"), v["iterations"].as_u64().unwrap() as u32)
}

#[tokio::test]
async fn login_refuses_too_few_iterations_before_any_proof() {
    let server = MockServer::start().await;
    let mut v = vector("ascii_password");
    v["iterations"] = serde_json::json!(999);
    mount_login_prelude(&server, &v).await;

    let session = HuaweiSession::new(&server.uri()).unwrap();
    let err = session
        .login_with_scram(&creds(&v), scram(&v))
        .await
        .unwrap_err();
    assert_eq!(err, RouterError::LoginAlgorithmMismatch);
    assert!(requests_to(&server, "POST", AUTH).await.is_empty());
}

#[tokio::test]
async fn login_gate_stops_before_the_proof() {
    let v = vector("ascii_password");
    let other = RouterIdentity::new("00ff", 1000);
    for (gate, first_time) in [(LoginGate::Expect(other), false), (LoginGate::Ask, true)] {
        let server = MockServer::start().await;
        mount_login_prelude(&server, &v).await;
        let session = HuaweiSession::new(&server.uri()).unwrap();
        let err = session
            .login_with_scram_gated(&creds(&v), scram(&v), &gate)
            .await
            .unwrap_err();
        assert_eq!(err, RouterError::UnrecognizedRouter { first_time });
        assert!(requests_to(&server, "POST", AUTH).await.is_empty());
    }
}

#[tokio::test]
async fn login_gate_lets_the_recorded_router_through_and_reports_it() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(auth_response(s(&v, "serversignature")))
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let seen = session
        .login_with_scram_gated(&creds(&v), scram(&v), &LoginGate::Expect(identity_of(&v)))
        .await
        .unwrap();
    assert_eq!(seen, Some(identity_of(&v)));
    assert_eq!(requests_to(&server, "POST", AUTH).await.len(), 1);
}

#[tokio::test]
async fn login_reports_no_identity_when_the_signature_fails() {
    let server = MockServer::start().await;
    let v = vector("ascii_password");
    mount_login_prelude(&server, &v).await;
    Mock::given(method("POST"))
        .and(path(AUTH))
        .respond_with(auth_response("00"))
        .mount(&server)
        .await;
    let session = HuaweiSession::new(&server.uri()).unwrap();
    let result = session
        .login_with_scram_gated(&creds(&v), scram(&v), &LoginGate::Open)
        .await;
    assert!(
        result.is_err(),
        "a device without the password never gets recorded"
    );
}

/// `forget` empties the cookie jar: the next request carries no `SessionID`, and no token.
#[tokio::test]
async fn forget_drops_the_session_cookie_and_tokens() {
    let server = MockServer::start().await;
    mount_token(&server).await;
    Mock::given(method("GET"))
        .and(path("/api/test/cookie"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(OK_BODY)
                .insert_header("Set-Cookie", "SessionID=demo-session; path=/; HttpOnly"),
        )
        .mount(&server)
        .await;
    mount_get(&server, "/api/test/after", OK_BODY.to_string()).await;
    let session = primed_session(&server).await;
    session.get("/api/test/cookie").await.unwrap();
    session.get("/api/test/after").await.unwrap();
    session.forget();
    assert_eq!(session.queued_tokens(), 0);
    session.get("/api/test/after").await.unwrap();

    let after = requests_to(&server, "GET", "/api/test/after").await;
    assert_eq!(after.len(), 2);
    assert!(header(&after[0], "cookie").is_some_and(|c| c.contains("SessionID=demo-session")));
    assert_eq!(header(&after[1], "cookie"), None, "the jar was emptied");
}
