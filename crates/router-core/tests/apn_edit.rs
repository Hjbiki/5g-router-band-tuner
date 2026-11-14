//! APN profile add, edit and delete: the draft's checks, the password that never shows, and the
//! Huawei driver's requests and guards.

use router_core::apn::{check_after_write, check_write, ApnDraft, PasswordChange, Secret};
use router_core::drivers::huawei::xml;
use router_core::drivers::huawei::HuaweiH122Driver;
use router_core::{
    ApnAuth, ApnIpType, ApnTarget, RouterDriver, RouterError, APN_CURRENT_MOVED,
    APN_NEEDS_ENCRYPTION, APN_PASSWORD_HIDDEN, APN_PROFILE_CHANGED, APN_PROFILE_IN_USE,
    APN_PROFILE_READ_ONLY, APN_ROUTER_SWITCHED,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TYPED: &str = "typed-secret-5678";
const SAVED: &str = "synthetic-secret-1234";

fn fx(n: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/huawei_h122/{n}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

fn draft(password: PasswordChange) -> ApnDraft {
    ApnDraft {
        name: "  Travel  ".into(),
        apn: "travel.example".into(),
        username: "demo".into(),
        password,
        auth: ApnAuth::Chap,
        ip_type: ApnIpType::Ipv4,
    }
}

fn set(p: &str) -> PasswordChange {
    PasswordChange::Set {
        value: Secret::new(p),
    }
}

fn refusal(m: &str) -> RouterError {
    RouterError::Validation {
        message: m.to_string(),
    }
}

fn is_refusal(r: &Result<impl std::fmt::Debug, RouterError>, m: &str) -> bool {
    matches!(r, Err(RouterError::Validation { message }) if message == m)
}

#[test]
fn a_secret_is_never_shown_or_serialised() {
    let d = draft(set(TYPED));
    let debug = format!("{d:?} {:?}", Secret::new(TYPED));
    assert!(!debug.contains(TYPED), "{debug}");
    assert!(debug.contains("***"));
    // The draft comes from the UI as JSON, with the password as a tagged union.
    let json = format!(
        r#"{{"name":"A","apn":"","username":"","auth":"pap","ipType":"ipv4v6","password":{{"kind":"set","value":"{TYPED}"}}}}"#
    );
    let parsed: ApnDraft = serde_json::from_str(&json).unwrap();
    assert!(matches!(&parsed.password, PasswordChange::Set { value } if value.expose() == TYPED));
    let keep: PasswordChange = serde_json::from_str(r#"{"kind":"keep"}"#).unwrap();
    assert_eq!(keep, PasswordChange::Keep);
    // Neither type implements Serialize; this would not compile:
    // serde_json::to_string(&parsed)
}

#[test]
fn the_draft_rules() {
    let ok = draft(set(TYPED)).checked(false).unwrap();
    assert_eq!(ok.name, "Travel", "trimmed");
    let bad = |f: &dyn Fn(&mut ApnDraft)| {
        let mut d = draft(PasswordChange::Clear);
        f(&mut d);
        d.checked(false).unwrap_err()
    };
    let name = refusal(router_core::apn::APN_NAME_INVALID);
    assert_eq!(bad(&|d| d.name = "   ".into()), name);
    assert_eq!(bad(&|d| d.name = "x".repeat(65)), name);
    assert_eq!(bad(&|d| d.name = "a\u{7}b".into()), name);
    let apn = refusal(router_core::apn::APN_APN_INVALID);
    assert_eq!(bad(&|d| d.apn = "has space".into()), apn);
    assert_eq!(bad(&|d| d.apn = "a<b".into()), apn);
    assert_eq!(bad(&|d| d.apn = "a".repeat(101)), apn);
    assert_eq!(
        bad(&|d| d.username = "u".repeat(65)),
        refusal(router_core::apn::APN_USERNAME_INVALID)
    );
    assert_eq!(
        bad(&|d| d.password = set(&"p".repeat(65))),
        refusal(router_core::apn::APN_PASSWORD_INVALID)
    );
    assert_eq!(
        bad(&|d| d.password = set("a\nb")),
        refusal(router_core::apn::APN_PASSWORD_INVALID)
    );
    assert_eq!(
        bad(&|d| d.auth = ApnAuth::Other),
        refusal(router_core::apn::APN_KIND_INVALID)
    );
    assert_eq!(
        draft(PasswordChange::Keep).checked(true).unwrap_err(),
        refusal(router_core::apn::APN_NOTHING_TO_KEEP)
    );
    // The network chooses the APN when it is empty; 64 and 100 characters are fine.
    let mut d = draft(PasswordChange::Clear);
    d.apn = String::new();
    d.name = "n".repeat(64);
    assert!(d.checked(true).is_ok());
    d.apn = "a".repeat(100);
    assert!(d.checked(true).is_ok());
}

#[test]
fn keep_needs_a_password_the_router_shows() {
    let d = draft(PasswordChange::Keep);
    assert_eq!(d.password_text(Some(SAVED)).unwrap(), SAVED);
    for hidden in ["", "  ", "******"] {
        assert_eq!(
            d.password_text(Some(hidden)).unwrap_err(),
            refusal(APN_PASSWORD_HIDDEN),
            "{hidden:?}"
        );
    }
    assert_eq!(
        draft(PasswordChange::Clear)
            .password_text(Some(SAVED))
            .unwrap(),
        ""
    );
}

#[test]
fn the_write_guard() {
    let p = xml::parse_apn_profiles(&fx("dialup_profiles.xml")).unwrap();
    let current = p.current_target().unwrap();
    let t = |i: u32| ApnTarget {
        index: i,
        fingerprint: p.get(i).unwrap().fingerprint.clone(),
    };
    assert!(check_write(&p, &current, None).is_ok());
    assert!(check_write(&p, &current, Some(&t(2))).is_ok());
    assert_eq!(
        check_write(&p, &t(2), None).unwrap_err(),
        refusal(APN_CURRENT_MOVED)
    );
    // Profile 1 is in use (and the operator's): in use is said first.
    assert_eq!(
        check_write(&p, &current, Some(&t(1))).unwrap_err(),
        refusal(APN_PROFILE_IN_USE)
    );
    let stale = ApnTarget {
        index: 2,
        fingerprint: "0000000000000000".into(),
    };
    assert_eq!(
        check_write(&p, &current, Some(&stale)).unwrap_err(),
        refusal(APN_PROFILE_CHANGED)
    );
    let mut ro = p.clone();
    ro.profiles[1].read_only = true;
    assert_eq!(
        check_write(&ro, &current, Some(&t(2))).unwrap_err(),
        refusal(APN_PROFILE_READ_ONLY)
    );
    let mut moved = p.clone();
    moved.current = Some(2);
    assert_eq!(
        check_after_write(&moved, &current).unwrap_err(),
        refusal(APN_ROUTER_SWITCHED)
    );
}

#[test]
fn profile_tags_come_back_in_order_with_unknown_ones() {
    let body = fx("dialup_profiles.xml");
    let tags = xml::parse_profile_tags(&body, 2).unwrap().unwrap();
    let names: Vec<&str> = tags.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names[..3], ["Index", "IsValid", "Name"]);
    assert!(names.contains(&"DialupNum") && names.contains(&"PrimaryDns"));
    assert!(tags.contains(&("Name".to_string(), "Work & travel".to_string())));
    assert!(tags.contains(&("Password".to_string(), SAVED.to_string())));
    assert_eq!(xml::parse_profile_tags(&body, 9).unwrap(), None);
    let nested = "<response><Profiles><Profile><Index>2</Index><X><Y>1</Y></X></Profile></Profiles></response>";
    assert!(xml::parse_profile_tags(nested, 2).is_err());
    assert!(xml::parse_profile_tags("<error><code>1</code></error>", 2).is_err());
}

// --- Huawei driver ------------------------------------------------------------------------

const OK: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";

async fn setup() -> (MockServer, HuaweiH122Driver) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/webserver/token"))
        .respond_with(ResponseTemplate::new(200).set_body_string(fx("webserver_token.xml")))
        .mount(&server)
        .await;
    let d = HuaweiH122Driver::new(&server.uri()).unwrap();
    (server, d)
}

/// The profile list answers `first` once, then `then` for every later read.
async fn mount_profiles(server: &MockServer, first: String, then: String) {
    Mock::given(method("GET"))
        .and(path("/api/dialup/profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_string(first))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/dialup/profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_string(then))
        .with_priority(5)
        .mount(server)
        .await;
}

async fn mount_post(server: &MockServer, body: &str) {
    Mock::given(method("POST"))
        .and(path("/api/dialup/profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body.to_string()))
        .mount(server)
        .await;
}

async fn posts(server: &MockServer) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == "POST" && r.url.path() == "/api/dialup/profiles")
        .collect()
}

fn body(r: &Request) -> String {
    String::from_utf8(r.body.clone()).unwrap()
}

fn targets() -> (ApnTarget, ApnTarget) {
    let p = xml::parse_apn_profiles(&fx("dialup_profiles.xml")).unwrap();
    let t = |i: u32| ApnTarget {
        index: i,
        fingerprint: p.get(i).unwrap().fingerprint.clone(),
    };
    (t(1), t(2))
}

/// The fixture with profile 2 rewritten to the draft and `current` in use.
fn after_edit(current: u32) -> String {
    fx("dialup_profiles.xml")
        .replacen(
            "<CurrentProfile>1<",
            &format!("<CurrentProfile>{current}<"),
            1,
        )
        .replace("<Name>Work &amp; travel</Name>", "<Name>Travel</Name>")
        .replace("work.example", "travel.example")
        .replace(
            "<Username>demo-user</Username>",
            "<Username>demo</Username>",
        )
}

#[tokio::test]
async fn edit_sends_every_tag_back_and_keeps_the_saved_password() {
    let (server, d) = setup().await;
    mount_profiles(&server, fx("dialup_profiles.xml"), after_edit(1)).await;
    mount_post(&server, OK).await;
    let (current, work) = targets();
    d.edit_apn_profile(&current, &work, &draft(PasswordChange::Keep))
        .await
        .unwrap();
    let p = posts(&server).await;
    assert_eq!(p.len(), 1);
    let b = body(&p[0]);
    assert!(
        b.contains("<Delete>0</Delete><SetDefault>1</SetDefault><Modify>2</Modify><Profile><Index>2</Index>"),
        "{b}"
    );
    assert!(b.contains("<Name>Travel</Name>"), "{b}");
    assert!(b.contains("<ApnName>travel.example</ApnName>"), "{b}");
    assert!(
        b.contains("<DialupNum>*99#</DialupNum>"),
        "passed through: {b}"
    );
    assert!(
        b.contains(&format!("<Password>{SAVED}</Password>")),
        "kept: {b}"
    );
    assert!(b.contains("<iptype>0</iptype>"), "{b}");
}

#[tokio::test]
async fn a_typed_password_is_escaped_and_sent_once() {
    let (server, d) = setup().await;
    mount_profiles(&server, fx("dialup_profiles.xml"), after_edit(1)).await;
    mount_post(&server, OK).await;
    let (current, work) = targets();
    d.edit_apn_profile(&current, &work, &draft(set("a<&b")))
        .await
        .unwrap();
    let b = body(&posts(&server).await[0]);
    assert!(b.contains("<Password>a&lt;&amp;b</Password>"), "{b}");
    assert!(!b.contains(SAVED), "{b}");
}

#[tokio::test]
async fn a_hidden_password_cant_be_kept_and_nothing_is_posted() {
    let (server, d) = setup().await;
    let masked = fx("dialup_profiles.xml").replace(SAVED, "******");
    mount_profiles(&server, masked.clone(), masked).await;
    mount_post(&server, OK).await;
    let (current, work) = targets();
    let r = d
        .edit_apn_profile(&current, &work, &draft(PasswordChange::Keep))
        .await;
    assert!(is_refusal(&r, APN_PASSWORD_HIDDEN), "{r:?}");
    assert!(posts(&server).await.is_empty());
}

#[tokio::test]
async fn the_guard_refuses_before_any_post() {
    let (server, d) = setup().await;
    mount_profiles(
        &server,
        fx("dialup_profiles.xml"),
        fx("dialup_profiles.xml"),
    )
    .await;
    mount_post(&server, OK).await;
    let (current, work) = targets();
    let r = d.edit_apn_profile(&work, &work, &draft(set(TYPED))).await;
    assert!(is_refusal(&r, APN_CURRENT_MOVED), "{r:?}");
    let r = d.delete_apn_profile(&current, &current).await;
    assert!(is_refusal(&r, APN_PROFILE_IN_USE), "{r:?}");
    let mut bad = draft(PasswordChange::Clear);
    bad.name = String::new();
    let r = d.add_apn_profile(&current, &bad).await;
    assert!(matches!(r, Err(RouterError::Validation { .. })), "{r:?}");
    assert!(posts(&server).await.is_empty());
}

#[tokio::test]
async fn a_moved_profile_in_use_is_reported_after_the_write() {
    let (server, d) = setup().await;
    let gone = fx("dialup_profiles.xml").replacen("<CurrentProfile>1<", "<CurrentProfile>3<", 1);
    mount_profiles(&server, fx("dialup_profiles.xml"), gone).await;
    mount_post(&server, OK).await;
    let (current, work) = targets();
    let r = d.delete_apn_profile(&current, &work).await;
    assert!(is_refusal(&r, APN_ROUTER_SWITCHED), "{r:?}");
    let b = body(&posts(&server).await[0]);
    assert!(
        b.contains(
            "<request><Delete>2</Delete><SetDefault>1</SetDefault><Modify>0</Modify></request>"
        ),
        "{b}"
    );
}

#[tokio::test]
async fn a_refused_plain_request_says_it_may_need_encryption() {
    let (server, d) = setup().await;
    mount_profiles(
        &server,
        fx("dialup_profiles.xml"),
        fx("dialup_profiles.xml"),
    )
    .await;
    mount_post(
        &server,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100005</code><message></message></error>",
    )
    .await;
    let (current, _) = targets();
    let r = d.add_apn_profile(&current, &draft(set(TYPED))).await;
    assert!(is_refusal(&r, APN_NEEDS_ENCRYPTION), "{r:?}");
}

#[tokio::test]
async fn add_returns_the_new_index() {
    let (server, d) = setup().await;
    let new_profile = "<Profile>\n<Index>4</Index>\n<IsValid>1</IsValid>\n<Name>Travel</Name>\n<ApnIsStatic>1</ApnIsStatic>\n<ApnName>travel.example</ApnName>\n<Username>demo</Username>\n<Password></Password>\n<AuthMode>2</AuthMode>\n<ReadOnly>0</ReadOnly>\n<iptype>0</iptype>\n</Profile>\n</Profiles>";
    let after = fx("dialup_profiles.xml").replacen("</Profiles>", new_profile, 1);
    mount_profiles(&server, fx("dialup_profiles.xml"), after).await;
    mount_post(&server, OK).await;
    let (current, _) = targets();
    let index = d
        .add_apn_profile(&current, &draft(set(TYPED)))
        .await
        .unwrap();
    assert_eq!(index, 4);
    let b = body(&posts(&server).await[0]);
    assert!(
        b.contains("<Delete>0</Delete><SetDefault>1</SetDefault><Modify>1</Modify><Profile><Index></Index><IsValid>1</IsValid><Name>Travel</Name>"),
        "{b}"
    );
    assert!(b.contains(&format!("<Password>{TYPED}</Password>")), "{b}");
    assert!(b.contains("<ReadOnly>0</ReadOnly>"), "{b}");
    // Saved as something else: the router didn't take the draft as sent.
    let (server, d) = setup().await;
    mount_profiles(
        &server,
        fx("dialup_profiles.xml"),
        fx("dialup_profiles.xml"),
    )
    .await;
    mount_post(&server, OK).await;
    let r = d.add_apn_profile(&current, &draft(set(TYPED))).await;
    assert!(is_refusal(&r, router_core::APN_SAVE_MISMATCH), "{r:?}");
}
