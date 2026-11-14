use router_core::drivers::huawei::xml;
use router_core::drivers::huawei::{
    HuaweiH122Driver, DIAGNOSTIC_PATHS, PATH_PLMN_LIST, SEARCH_TIMEOUT,
};
use router_core::{
    DriverInfo, IpMethod, LteBandSet, NetConfig, NetworkMode, NrOption, RouterDriver, RouterError,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn fx(n: &str) -> String {
    let p = format!(
        "{}/tests/fixtures/huawei_h122/{n}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(p).unwrap()
}

async fn mount_get(server: &MockServer, p: &str, body: String) {
    Mock::given(method("GET"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(server)
        .await;
}

async fn mount_post(server: &MockServer, p: &str, body: &str) {
    Mock::given(method("POST"))
        .and(path(p))
        .respond_with(ResponseTemplate::new(200).set_body_string(body.to_string()))
        .mount(server)
        .await;
}

const OK: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>OK</response>";

async fn setup() -> (MockServer, HuaweiH122Driver) {
    let server = MockServer::start().await;
    mount_get(&server, "/api/webserver/token", fx("webserver_token.xml")).await;
    let d = HuaweiH122Driver::new(&server.uri()).unwrap();
    (server, d)
}

async fn reqs(server: &MockServer, m: &str, p: &str) -> Vec<Request> {
    server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .filter(|r| r.method.as_str() == m && r.url.path() == p)
        .collect()
}

fn cfg(opt: Option<NrOption>) -> NetConfig {
    NetConfig {
        network_mode: NetworkMode::LteOnly,
        lte_bands: LteBandSet::from_bands(&[1, 3, 20]).unwrap(),
        legacy_band_raw: "0".to_string(),
        nr_option: opt,
    }
}

#[test]
fn info_and_new() {
    let d = HuaweiH122Driver::new("192.168.8.1").unwrap();
    assert_eq!(
        d.info(),
        DriverInfo {
            id: "huawei_h122",
            name: "Huawei H122-373"
        }
    );
    assert!(HuaweiH122Driver::new("http://192.168.8.1").is_ok());
    assert!(HuaweiH122Driver::new("192.168.8.1/api").is_err());
    assert!(HuaweiH122Driver::new("http://192.168.8.1/x").is_err());
    assert!(HuaweiH122Driver::new("").is_err());
}

#[tokio::test]
async fn apply_body_exact() {
    let (server, d) = setup().await;
    mount_post(&server, "/api/net/net-mode", OK).await;
    let c = cfg(Some(NrOption::Nsa));
    let preview = d.apply_preview(&c);
    assert_eq!(
        preview.body,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><NetworkMode>03</NetworkMode><NetworkBand>0</NetworkBand><LTEBand>80005</LTEBand><networkOption>0</networkOption></request>"
    );
    assert_eq!(preview.fields.len(), 4);
    d.apply_net_config(&c).await.unwrap();
    let got = reqs(&server, "POST", "/api/net/net-mode").await;
    assert_eq!(got.len(), 1);
    assert_eq!(
        String::from_utf8(got[0].body.clone()).unwrap(),
        preview.body
    );
}

#[tokio::test]
async fn apply_omits_option_when_none() {
    let (_s, d) = setup().await;
    let p = d.apply_preview(&cfg(None));
    assert!(!p.body.contains("networkOption"));
    assert_eq!(p.fields.len(), 3);
}

#[tokio::test]
async fn apply_never_sends_undefined() {
    for (opt, code) in [
        (NrOption::Nsa, "0"),
        (NrOption::Sa, "1"),
        (NrOption::SaNsa, "2"),
    ] {
        let (server, d) = setup().await;
        mount_post(&server, "/api/net/net-mode", OK).await;
        d.apply_net_config(&cfg(Some(opt))).await.unwrap();
        let got = reqs(&server, "POST", "/api/net/net-mode").await;
        let b = String::from_utf8(got[0].body.clone()).unwrap();
        assert!(!b.contains("undefined"));
        assert!(b.contains(&format!("<networkOption>{code}</networkOption>")));
    }
}

#[tokio::test]
async fn apply_error_surfaces() {
    let (server, d) = setup().await;
    mount_post(
        &server,
        "/api/net/net-mode",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100005</code><message></message></error>",
    )
    .await;
    let r = d.apply_net_config(&cfg(Some(NrOption::Nsa))).await;
    assert!(
        matches!(r, Err(RouterError::Api { code: 100005, .. })),
        "{r:?}"
    );
}

#[tokio::test]
async fn reboot_sends_control_1() {
    let (server, d) = setup().await;
    mount_post(&server, "/api/device/control", OK).await;
    d.reboot().await.unwrap();
    let got = reqs(&server, "POST", "/api/device/control").await;
    let b = String::from_utf8(got[0].body.clone()).unwrap();
    assert!(b.contains("<Control>1</Control>"));
}

/// Confirmed on this model by the user's bookmarklet: one POST of `Control` = 2, nothing else.
#[tokio::test]
async fn factory_reset_sends_control_2_once() {
    let (server, d) = setup().await;
    mount_post(&server, "/api/device/control", OK).await;
    d.factory_reset().await.unwrap();
    let got = reqs(&server, "POST", "/api/device/control").await;
    assert_eq!(got.len(), 1);
    let b = String::from_utf8(got[0].body.clone()).unwrap();
    assert!(b.contains("<Control>2</Control>"));

    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    mount_get(&server, "/api/net/net-mode-list", fx("net_mode_list.xml")).await;
    assert!(d.capabilities().await.unwrap().factory_reset_verified);
}

#[tokio::test]
async fn capabilities_from_list() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    mount_get(&server, "/api/net/net-mode-list", fx("net_mode_list.xml")).await;
    let got = d.capabilities().await.unwrap();
    let cfg = xml::parse_net_mode(&fx("net_mode.xml")).unwrap();
    let mut want = xml::parse_capabilities(&fx("net_mode_list.xml"), &cfg);
    // The H122 refreshes its IP with a network search only: its data-switch "off" was refused.
    want.ip_methods = vec![IpMethod::NetworkSearch];
    assert_eq!(got, want);
    assert!(got.band_edit_allowed);
}

/// The search is one GET of plmn-list with the session cookie; only the count is kept.
#[tokio::test]
async fn network_search_gets_plmn_list_and_counts_networks() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/net/plmn-list", fx("plmn_list.xml")).await;
    let got = d.network_search().await.unwrap();
    assert_eq!(got.networks, Some(3));
    let all = server.received_requests().await.unwrap();
    assert_eq!(all.len(), 1, "one request, no token fetch");
    assert_eq!(all[0].method.as_str(), "GET");
    assert_eq!(all[0].url.path(), PATH_PLMN_LIST);
}

#[tokio::test]
async fn network_search_keeps_the_router_code() {
    let (server, d) = setup().await;
    let refusal = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100003</code><message></message></error>";
    mount_get(&server, "/api/net/plmn-list", refusal.to_string()).await;
    assert_eq!(
        d.network_search().await,
        Err(RouterError::SessionLost { code: Some(100003) })
    );

    let (server, d) = setup().await;
    mount_get(&server, "/api/net/plmn-list", "not xml at all".to_string()).await;
    assert_eq!(d.network_search().await.unwrap().networks, None);
    drop(server);
}

/// plmn-list drops the data session: it must never run as a diagnostics read.
#[test]
fn diagnostics_never_search() {
    assert!(!DIAGNOSTIC_PATHS.contains(&PATH_PLMN_LIST));
    assert!(DIAGNOSTIC_PATHS.iter().all(|p| !p.contains("plmn-list")));
    assert!(SEARCH_TIMEOUT >= std::time::Duration::from_secs(120));
}

#[tokio::test]
async fn status_includes_operator() {
    let (server, d) = setup().await;
    mount_get(
        &server,
        "/api/monitoring/status",
        fx("monitoring_status.xml"),
    )
    .await;
    mount_get(&server, "/api/net/current-plmn", fx("current_plmn.xml")).await;
    let s = d.status().await.unwrap();
    assert_eq!(s.operator.as_ref().unwrap().mcc_mnc, "42004");
    let s2 = d.status().await.unwrap();
    assert_eq!(s2.operator, s.operator);
    assert_eq!(reqs(&server, "GET", "/api/net/current-plmn").await.len(), 1);
    assert_eq!(
        reqs(&server, "GET", "/api/monitoring/status").await.len(),
        2
    );
}

#[tokio::test]
async fn simple_endpoints() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/user/heartbeat", fx("heartbeat.xml")).await;
    mount_get(
        &server,
        "/api/user/state-login",
        fx("state_login_logged_in.xml"),
    )
    .await;
    mount_get(&server, "/api/device/signal", fx("signal.xml")).await;
    mount_get(
        &server,
        "/api/device/information",
        fx("device_information.xml"),
    )
    .await;
    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    d.heartbeat().await.unwrap();
    assert_eq!(
        d.session_state().await.unwrap(),
        router_core::SessionState::LoggedIn
    );
    d.signal().await.unwrap();
    assert!(!d.device_info().await.unwrap().model.is_empty());
    let wan = d.wan_address().await.unwrap();
    assert_eq!(wan.ipv4, Some(std::net::Ipv4Addr::new(192, 0, 2, 17)));
    assert!(wan.ipv6_present);
    assert_eq!(
        d.net_config().await.unwrap(),
        xml::parse_net_mode(&fx("net_mode.xml")).unwrap()
    );
}

fn collect_rs(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            collect_rs(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Shutdown and config backup are never sent, and the reset call (`Control` = 2) lives only in
/// the Huawei driver's `factory_reset`, once.
#[test]
fn no_shutdown_and_one_reset_call() {
    let mut files = Vec::new();
    collect_rs(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut files,
    );
    assert!(!files.is_empty());
    let mut resets = 0;
    for f in files {
        let text = std::fs::read_to_string(&f).unwrap();
        for bad in ["<Control>3", "<Control>4"] {
            assert!(!text.contains(bad), "{} contains {bad}", f.display());
        }
        let n = text.matches("<Control>2").count();
        if n > 0 {
            assert!(
                f.ends_with("drivers/huawei/driver.rs") || f.ends_with(r"drivers\huawei\driver.rs"),
                "{} contains <Control>2",
                f.display()
            );
            resets += n;
        }
    }
    assert_eq!(resets, 1);
}

#[tokio::test]
async fn diagnostic_reads_only_get_and_redact_cleanly() {
    let (server, d) = setup().await;
    mount_get(
        &server,
        "/api/device/information",
        fx("device_information.xml"),
    )
    .await;
    mount_get(
        &server,
        "/api/monitoring/status",
        fx("monitoring_status.xml"),
    )
    .await;
    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    mount_get(
        &server,
        "/api/dialup/mobile-dataswitch",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><dataswitch>1</dataswitch></response>"
            .to_string(),
    )
    .await;
    // Every other path answers like a router that lacks it.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100002</code><message></message></error>",
        ))
        .mount(&server)
        .await;

    let reads = d.diagnostic_reads().await;
    assert!(reads.len() >= 8);
    assert!(reads.iter().any(|r| r.result.is_ok()));
    assert!(reads.iter().any(|r| r.result.is_err()));
    let received = server.received_requests().await.unwrap();
    assert!(received
        .iter()
        .all(|r| r.method == wiremock::http::Method::GET));

    let switch = reads
        .iter()
        .find(|r| r.path == "/api/dialup/mobile-dataswitch")
        .expect("the data switch is read");
    assert!(switch
        .result
        .as_ref()
        .unwrap()
        .contains("<dataswitch>1</dataswitch>"));

    let report =
        router_core::diagnostics::build_report("0.0.0", "huawei_h122", "Huawei", 0, &reads, &[]);
    assert!(report.is_ok(), "{:?}", report.err());
    assert!(report.unwrap().contains("<dataswitch>1</dataswitch>"));
}

#[tokio::test]
async fn logout_body_exact() {
    let (server, d) = setup().await;
    mount_post(&server, "/api/user/logout", OK).await;
    d.logout().await.unwrap();
    let posts = reqs(&server, "POST", "/api/user/logout").await;
    assert_eq!(posts.len(), 1);
    assert_eq!(
        String::from_utf8(posts[0].body.clone()).unwrap(),
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><request><Logout>1</Logout></request>"
    );
}

/// A router that already dropped the session has nothing to end: that counts as logged out.
#[tokio::test]
async fn logout_without_a_session_counts_as_done() {
    for code in [100003, 125002] {
        let (server, d) = setup().await;
        let err = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>{code}</code><message></message></error>"
        );
        mount_post(&server, "/api/user/logout", &err).await;
        assert_eq!(d.logout().await, Ok(()), "code {code}");
    }
    let (server, d) = setup().await;
    mount_post(
        &server,
        "/api/user/logout",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><error><code>100002</code><message></message></error>",
    )
    .await;
    assert!(matches!(
        d.logout().await,
        Err(RouterError::Api { code: 100002, .. })
    ));
}

#[tokio::test]
async fn data_switch_read_and_posts() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    mount_get(&server, "/api/net/net-mode-list", fx("net_mode_list.xml")).await;
    // No answer from the switch endpoint: Refresh IP stays hidden.
    assert!(!d.capabilities().await.unwrap().data_switch);

    mount_get(
        &server,
        "/api/dialup/mobile-dataswitch",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><dataswitch>1</dataswitch></response>"
            .to_string(),
    )
    .await;
    assert!(d.capabilities().await.unwrap().data_switch);
    assert!(d.data_switch().await.unwrap());

    mount_post(&server, "/api/dialup/mobile-dataswitch", OK).await;
    d.set_data_switch(false).await.unwrap();
    d.set_data_switch(true).await.unwrap();
    let posts = reqs(&server, "POST", "/api/dialup/mobile-dataswitch").await;
    assert_eq!(posts.len(), 2);
    let body = |i: usize| String::from_utf8(posts[i].body.clone()).unwrap();
    assert!(
        body(0).contains("<dataswitch>0</dataswitch>"),
        "{}",
        body(0)
    );
    assert!(
        body(1).contains("<dataswitch>1</dataswitch>"),
        "{}",
        body(1)
    );
}

#[tokio::test]
async fn apn_profiles_are_two_gets_and_nothing_else() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    mount_get(&server, "/api/net/net-mode-list", fx("net_mode_list.xml")).await;
    let caps = d.capabilities().await.unwrap();
    assert!(caps.apn.read);
    assert!(caps.apn.switch && caps.apn.edit && !caps.apn.confirmed);

    mount_get(&server, "/api/dialup/profiles", fx("dialup_profiles.xml")).await;
    // No Auto APN answer: the list still reads, Auto APN stays unknown.
    let p = d.apn_profiles().await.unwrap();
    assert_eq!(p.current, Some(1));
    assert_eq!(p.auto_apn, None);
    mount_get(
        &server,
        "/api/dialup/auto-apn",
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><AutoAPN>1</AutoAPN></response>"
            .to_string(),
    )
    .await;
    assert_eq!(d.apn_profiles().await.unwrap().auto_apn, Some(true));
    let all = server.received_requests().await.unwrap();
    assert!(all.iter().all(|r| r.method.as_str() == "GET"), "reads only");
    // The profiles hold saved passwords: never part of the diagnostics export.
    let reads = d.diagnostic_reads().await;
    assert!(reads
        .iter()
        .all(|r| !r.path.contains("profiles") && !r.path.contains("apn")));
}

#[tokio::test]
async fn set_default_apn_reads_first_and_posts_only_the_default() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/net/net-mode", fx("net_mode.xml")).await;
    mount_get(&server, "/api/net/net-mode-list", fx("net_mode_list.xml")).await;
    assert!(d.capabilities().await.unwrap().apn.switch);
    mount_get(&server, "/api/dialup/profiles", fx("dialup_profiles.xml")).await;
    mount_post(&server, "/api/dialup/profiles", OK).await;
    let profiles = d.apn_profiles().await.unwrap();
    let work = profiles.get(2).unwrap();
    let target = router_core::ApnTarget {
        index: 2,
        fingerprint: work.fingerprint.clone(),
    };

    // A changed fingerprint, a missing index or an incomplete profile: nothing is posted.
    for bad in [
        router_core::ApnTarget {
            index: 2,
            fingerprint: "0000000000000000".into(),
        },
        router_core::ApnTarget {
            index: 9,
            fingerprint: work.fingerprint.clone(),
        },
        router_core::ApnTarget {
            index: 3,
            fingerprint: profiles.get(3).unwrap().fingerprint.clone(),
        },
    ] {
        assert!(matches!(
            d.set_default_apn(&bad).await,
            Err(RouterError::Validation { .. })
        ));
    }
    assert!(reqs(&server, "POST", "/api/dialup/profiles")
        .await
        .is_empty());

    d.set_default_apn(&target).await.unwrap();
    let posts = reqs(&server, "POST", "/api/dialup/profiles").await;
    assert_eq!(posts.len(), 1);
    let body = String::from_utf8(posts[0].body.clone()).unwrap();
    assert!(
        body.contains("<Delete>0</Delete><SetDefault>2</SetDefault><Modify>0</Modify>"),
        "{body}"
    );
    assert!(
        !body.contains("Password") && !body.contains("work.example"),
        "{body}"
    );
    let preview = d.apn_preview(&target);
    assert_eq!(preview.body, body);
    assert_eq!(
        preview.fields[1],
        ("SetDefault".to_string(), "2".to_string())
    );
}

const ONE_LAN_HOST: &str = "<response><Hosts><Host><ID>1</ID>\
<MacAddress>00:00:5E:00:53:21</MacAddress><IpAddress>192.168.8.50</IpAddress>\
<HostName>desk</HostName><Active>1</Active></Host></Hosts></response>";
const ONE_WIFI_HOST: &str = "<response><Hosts><Host><ID>1</ID>\
<MacAddress>00:00:5E:00:53:22</MacAddress><IpAddress>192.168.8.51</IpAddress>\
<HostName>tv</HostName><AssociatedSsid>home</AssociatedSsid></Host></Hosts></response>";

fn refusal(code: u32) -> String {
    format!("<error><code>{code}</code><message></message></error>")
}

#[tokio::test]
async fn remote_management_is_one_get_of_the_cwmp_page() {
    let (server, d) = setup().await;
    mount_get(
        &server,
        "/api/cwmp/basic-info",
        "<response><AcsUrl>https://acs.example.net:7547/x</AcsUrl></response>".to_string(),
    )
    .await;
    let seen = d.remote_management().await.unwrap();
    assert_eq!(seen.value.server_host.as_deref(), Some("acs.example.net"));
    let all = server.received_requests().await.unwrap();
    assert_eq!(all.len(), 1, "the check sends one request and nothing else");
    assert_eq!(all[0].method.as_str(), "GET");
    assert_eq!(all[0].url.path(), "/api/cwmp/basic-info");
}

#[tokio::test]
async fn a_refused_remote_management_read_keeps_the_routers_code() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/cwmp/basic-info", refusal(100003)).await;
    assert_eq!(
        d.remote_management().await.unwrap_err(),
        RouterError::SessionLost { code: Some(100003) }
    );
    assert!(reqs(&server, "POST", "/api/cwmp/basic-info")
        .await
        .is_empty());
}

#[tokio::test]
async fn each_device_list_is_one_get_of_its_own_page() {
    let (server, d) = setup().await;
    mount_get(&server, "/api/lan/HostInfo", ONE_LAN_HOST.to_string()).await;
    mount_get(&server, "/api/wlan/host-list", ONE_WIFI_HOST.to_string()).await;
    let lan = d.lan_devices().await.unwrap();
    assert_eq!(lan.value.len(), 1);
    assert_eq!(lan.value[0].mac, "00:00:5E:00:53:21");
    assert_eq!(lan.value[0].active, Some(true));
    assert_eq!(reqs(&server, "GET", "/api/lan/HostInfo").await.len(), 1);
    assert!(reqs(&server, "GET", "/api/wlan/host-list").await.is_empty());

    let wifi = d.wifi_devices().await.unwrap();
    assert_eq!(wifi.value.len(), 1);
    assert_eq!(wifi.value[0].mac, "00:00:5E:00:53:22");
    assert_eq!(wifi.value[0].active, None);
    assert_eq!(reqs(&server, "GET", "/api/wlan/host-list").await.len(), 1);
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

/// The whole check against a router that refuses the LAN list: three GETs, no more, and the
/// Wi-Fi list is what gets shown.
#[tokio::test]
async fn a_check_sends_three_gets_at_most_and_falls_back_to_the_wifi_list() {
    use router_core::privacy::{self, DeviceSource, PrivacyOutcome};
    let (server, d) = setup().await;
    mount_get(
        &server,
        "/api/cwmp/basic-info",
        "<response><AcsUrl>https://acs.example.net:7547/x</AcsUrl></response>".to_string(),
    )
    .await;
    mount_get(&server, "/api/lan/HostInfo", refusal(100003)).await;
    mount_get(&server, "/api/wlan/host-list", ONE_WIFI_HOST.to_string()).await;
    let (report, detail) = privacy::check(&d).await;
    assert_eq!(report.remote_management.outcome, PrivacyOutcome::Ok);
    assert_eq!(report.devices_read.outcome, PrivacyOutcome::Ok);
    assert_eq!(report.devices_source, Some(DeviceSource::Wifi));
    assert_eq!(report.devices[0].mac, "00:00:5E:00:53:22");
    assert_eq!(detail["devices"]["lan"]["code"], 100003);
    let all = server.received_requests().await.unwrap();
    assert_eq!(all.len(), 3);
    assert!(all.iter().all(|r| r.method.as_str() == "GET"));
}

/// A router without these pages most likely answers with an HTTP error, not an XML one. That
/// must read as "no page for this" with the status, not as an answer the app couldn't read.
#[tokio::test]
async fn a_missing_page_is_not_offered_and_keeps_the_http_status() {
    use router_core::privacy::{self, PrivacyOutcome, PrivacyRead};
    let (server, d) = setup().await;
    // Nothing is mounted for the three paths: the server answers 404.
    let (report, detail) = privacy::check(&d).await;
    let missing = PrivacyRead {
        outcome: PrivacyOutcome::NotOffered,
        code: Some(404),
    };
    assert_eq!(report.remote_management, missing);
    assert_eq!(report.devices_read, missing);
    assert_eq!(detail["remoteManagement"]["code"], 404);
    assert_eq!(detail["devices"]["wifi"]["code"], 404);
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
}

#[tokio::test]
async fn another_http_error_reads_as_refused_with_its_status() {
    use router_core::privacy::{PrivacyOutcome, PrivacyRead};
    let (server, d) = setup().await;
    Mock::given(method("GET"))
        .and(path("/api/cwmp/basic-info"))
        .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
        .mount(&server)
        .await;
    let error = d.remote_management().await.unwrap_err();
    assert_eq!(
        PrivacyRead::from_error(&error),
        PrivacyRead {
            outcome: PrivacyOutcome::Refused,
            code: Some(403)
        }
    );
}

/// The answers hold MAC addresses and the provider's account for the router, so they are never
/// part of the diagnostics export, redacted or not.
#[tokio::test]
async fn the_diagnostics_export_never_asks_for_the_privacy_reads() {
    let (server, d) = setup().await;
    let _ = d.diagnostic_reads().await;
    for request in server.received_requests().await.unwrap() {
        let p = request.url.path().to_string();
        assert!(
            !p.contains("cwmp") && !p.contains("HostInfo") && !p.contains("host-list"),
            "{p}"
        );
    }
}
