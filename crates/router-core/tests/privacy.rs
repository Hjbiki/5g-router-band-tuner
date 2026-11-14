//! The read-only privacy check: the Huawei parsers, the outcome of a failed read, and what a
//! check leaves in the event log. Every address and name here is made up (documentation ranges).

use router_core::drivers::huawei::xml;
use router_core::drivers::mock::{MockDriver, MockScenario};
use router_core::privacy::{
    self, clean_name, is_private_mac, normalize_mac, server_host, DeviceLink, DeviceSource,
    LanDevice, PrivacyOutcome, PrivacyRead,
};
use router_core::{ApiErrorKind, Credentials, RouterDriver, RouterError};

const CWMP: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>\
<Enable>1</Enable>\
<AcsUrl>https://cpe-login:s3cret-pass@acs.example.net:7547/cwmp/path?x=1</AcsUrl>\
<AcsUserName>cpe-account</AcsUserName>\
<AcsUserPassword>hunter2-secret</AcsUserPassword>\
<ConnReqUrl>http://198.51.100.7:7547/abcdef</ConnReqUrl>\
<PeriodicInformInterval>86400</PeriodicInformInterval>\
</response>";

#[test]
fn cwmp_gives_the_server_name_and_nothing_else() {
    let seen = xml::parse_cwmp(CWMP).unwrap();
    assert_eq!(seen.value.server_host.as_deref(), Some("acs.example.net"));
    assert_eq!(seen.value.server_set, Some(true));
    // Neither the provider's account for this router nor the router's own address is kept.
    let kept = format!("{:?}", seen.value);
    for private in [
        "cpe-login",
        "s3cret-pass",
        "cpe-account",
        "hunter2-secret",
        "198.51.100.7",
        "7547",
        "abcdef",
    ] {
        assert!(!kept.contains(private), "{private} was kept: {kept}");
    }
}

#[test]
fn cwmp_lists_the_element_names_in_order() {
    let seen = xml::parse_cwmp(CWMP).unwrap();
    assert_eq!(
        seen.tags,
        [
            "Enable",
            "AcsUrl",
            "AcsUserName",
            "AcsUserPassword",
            "ConnReqUrl",
            "PeriodicInformInterval"
        ]
    );
}

#[test]
fn cwmp_with_an_empty_server_field_reads_as_not_set() {
    let seen = xml::parse_cwmp("<response><AcsUrl></AcsUrl><Enable>0</Enable></response>").unwrap();
    assert_eq!(seen.value.server_host, None);
    assert_eq!(seen.value.server_set, Some(false));
    let seen = xml::parse_cwmp("<response><AcsUrl/></response>").unwrap();
    assert_eq!(seen.value.server_set, Some(false));
}

#[test]
fn cwmp_without_a_server_field_reads_as_unknown() {
    let seen = xml::parse_cwmp("<response><Enable>1</Enable></response>").unwrap();
    assert_eq!(seen.value.server_host, None);
    assert_eq!(seen.value.server_set, None);
}

/// The connection-request address is the router's own (its WAN address): it must never be shown
/// as the provider's server, and neither may an account field that happens to name the ACS.
#[test]
fn cwmp_never_takes_the_routers_own_address_or_an_account_field() {
    for body in [
        "<response><ConnReqUrl>http://198.51.100.7:7547/x</ConnReqUrl></response>",
        "<response><ConnectionRequestURL>http://198.51.100.7:7547/x</ConnectionRequestURL></response>",
        "<response><AcsUserName>http://not-a-server.example</AcsUserName></response>",
        "<response><AcsPassword>http://not-a-server.example</AcsPassword></response>",
        "<response><StunServerUrl>http://stun.example.net</StunServerUrl></response>",
    ] {
        let seen = xml::parse_cwmp(body).unwrap();
        assert_eq!(seen.value.server_host, None, "{body}");
        assert_eq!(seen.value.server_set, None, "{body}");
    }
}

#[test]
fn cwmp_prefers_the_acs_field_over_another_address() {
    let seen = xml::parse_cwmp(
        "<response><PortalUrl>http://portal.example.org</PortalUrl>\
         <ACSURL>http://acs.example.net/</ACSURL></response>",
    )
    .unwrap();
    assert_eq!(seen.value.server_host.as_deref(), Some("acs.example.net"));
}

/// "ACS" in a field's name isn't enough: a switch, a port or an account named for it must never
/// read as "a server is set", and must not hide the address after it.
#[test]
fn a_field_named_for_the_acs_only_counts_when_it_holds_an_address() {
    let cases: [(&str, Option<&str>, Option<bool>); 6] = [
        (
            "<response><AcsEnable>0</AcsEnable><AcsUrl></AcsUrl></response>",
            None,
            Some(false),
        ),
        (
            "<response><AcsPort>7547</AcsPort><AcsUrl>http://acs.example.net/</AcsUrl></response>",
            Some("acs.example.net"),
            Some(true),
        ),
        ("<response><AcsEnable>1</AcsEnable></response>", None, None),
        (
            "<response><Acs>https://acs.example.net:7547/</Acs></response>",
            Some("acs.example.net"),
            Some(true),
        ),
        (
            "<response><AcsAddress>acs.example.net</AcsAddress></response>",
            None,
            None,
        ),
        (
            "<response><AcsMode>https</AcsMode><Url>http://acs.example.net</Url></response>",
            Some("acs.example.net"),
            Some(true),
        ),
    ];
    for (body, host, set) in cases {
        let seen = xml::parse_cwmp(body).unwrap();
        assert_eq!(seen.value.server_host.as_deref(), host, "{body}");
        assert_eq!(seen.value.server_set, set, "{body}");
    }
}

/// The H122's own element names, as its first real answer listed them (2026-11-13). The values
/// here are made up.
const CWMP_H122: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>\
<enable>1</enable><inform>1</inform><interval>86400</interval>\
<acsurl>https://acs.example.net:7547/cwmp</acsurl>\
<acsname>cpe-account</acsname><acspwd>hunter2-secret</acspwd>\
<conname>conn-account</conname><conpwd>conn-secret</conpwd><conport>7547</conport>\
<cert>0</cert><authmode>0</authmode><informrangemin>60</informrangemin>\
<informrangemax>86400</informrangemax><iscwmpcertexisted>0</iscwmpcertexisted>\
<virtualserver_used_wanport>0</virtualserver_used_wanport></response>";

#[test]
fn cwmp_on_the_h122_says_whether_it_is_switched_on() {
    let seen = xml::parse_cwmp(CWMP_H122).unwrap();
    assert_eq!(seen.value.enabled, Some(true));
    assert_eq!(seen.value.server_host.as_deref(), Some("acs.example.net"));
    assert_eq!(seen.value.server_set, Some(true));
    let kept = format!("{:?}", seen.value);
    for private in [
        "cpe-account",
        "hunter2-secret",
        "conn-account",
        "conn-secret",
        "7547",
    ] {
        assert!(!kept.contains(private), "{private} was kept: {kept}");
    }
    let off = CWMP_H122.replace("<enable>1</enable>", "<enable>0</enable>");
    assert_eq!(xml::parse_cwmp(&off).unwrap().value.enabled, Some(false));
}

/// Only the field named exactly `enable` is the switch. A longer name that contains the word
/// (the periodic report's own switch, for one) must never be read as "remote management is on".
#[test]
fn the_switch_is_never_guessed_from_another_field() {
    for body in [
        "<response><acsurl>http://acs.example.net</acsurl></response>",
        "<response><enable>maybe</enable></response>",
        "<response><enable></enable></response>",
        "<response><PeriodicInformEnable>1</PeriodicInformEnable></response>",
        "<response><AcsEnable>1</AcsEnable><inform>1</inform></response>",
    ] {
        assert_eq!(xml::parse_cwmp(body).unwrap().value.enabled, None, "{body}");
    }
    // The name is matched without case, as the router's other answers vary in it.
    let seen = xml::parse_cwmp("<response><Enable>0</Enable></response>").unwrap();
    assert_eq!(seen.value.enabled, Some(false));
}

#[test]
fn cwmp_with_an_unreadable_address_says_one_is_set() {
    let seen = xml::parse_cwmp("<response><AcsUrl>acs example</AcsUrl></response>").unwrap();
    assert_eq!(seen.value.server_host, None);
    assert_eq!(seen.value.server_set, Some(true));
}

#[test]
fn cwmp_refuses_an_answer_that_is_not_the_routers() {
    for body in ["", "not xml <<", "<html><body>login</body></html>"] {
        assert!(
            matches!(xml::parse_cwmp(body), Err(RouterError::Parse { .. })),
            "{body:?}"
        );
    }
}

#[test]
fn server_host_keeps_only_the_host() {
    let cases: [(&str, Option<&str>); 12] = [
        ("http://acs.example.net", Some("acs.example.net")),
        ("HTTPS://ACS.Example.NET:8443/x/y", Some("acs.example.net")),
        ("https://user:pw@acs.example.net/", Some("acs.example.net")),
        ("http://203.0.113.9:7547/", Some("203.0.113.9")),
        ("  http://acs.example.net?x=1  ", Some("acs.example.net")),
        ("http://acs.example.net#frag", Some("acs.example.net")),
        ("acs.example.net", None),
        ("ftp://acs.example.net", None),
        ("http://", None),
        ("http://bad host/", None),
        ("http://[2001:db8::1]:7547/", None),
        ("http://evil<script>.example/", None),
    ];
    for (url, want) in cases {
        assert_eq!(server_host(url).as_deref(), want, "{url:?}");
    }
    assert_eq!(
        server_host(&format!("http://{}.example", "a".repeat(260))),
        None
    );
}

const HOSTS: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response><Hosts>\
<Host><ID>1</ID><MacAddress>00:00:5E:00:53:01</MacAddress><IpAddress>192.168.8.100</IpAddress>\
<HostName>desk-pc</HostName><ActualName>Office PC</ActualName><Active>1</Active>\
<AssociatedSsid></AssociatedSsid><InterfaceType>Ethernet</InterfaceType></Host>\
<Host><ID>2</ID><MacAddress>da-a1-19-00-53-02</MacAddress><IpAddress>192.168.8.101</IpAddress>\
<HostName>phone</HostName><ActualName></ActualName><Active>0</Active></Host>\
<Host><ID>3</ID><MacAddress>not-a-mac</MacAddress><HostName>ghost</HostName></Host>\
</Hosts></response>";

#[test]
fn hosts_are_read_with_name_address_and_state() {
    let seen = xml::parse_hosts(HOSTS).unwrap();
    assert_eq!(
        seen.value,
        [
            LanDevice {
                name: Some("Office PC".to_string()),
                mac: "00:00:5E:00:53:01".to_string(),
                ip: Some("192.168.8.100".to_string()),
                active: Some(true),
                private_mac: false,
                link: Some(DeviceLink::Cable),
                this_device: false,
            },
            LanDevice {
                name: Some("phone".to_string()),
                mac: "DA:A1:19:00:53:02".to_string(),
                ip: Some("192.168.8.101".to_string()),
                active: Some(false),
                private_mac: true,
                link: None,
                this_device: false,
            },
        ]
    );
}

#[test]
fn hosts_list_the_element_names_once_each() {
    let seen = xml::parse_hosts(HOSTS).unwrap();
    assert_eq!(
        seen.tags,
        [
            "Hosts",
            "Host",
            "ID",
            "MacAddress",
            "IpAddress",
            "HostName",
            "ActualName",
            "Active",
            "AssociatedSsid",
            "InterfaceType"
        ]
    );
}

/// `wlan/host-list` has no `Active` and no `ActualName`: online stays unknown, never "offline".
#[test]
fn hosts_without_an_active_field_leave_online_unknown() {
    let seen = xml::parse_hosts(
        "<response><Hosts><Host><ID>1</ID><MacAddress>00:00:5E:00:53:09</MacAddress>\
         <IpAddress>192.168.8.7;fe80::1</IpAddress><HostName>tv</HostName>\
         <AssociatedSsid>home</AssociatedSsid></Host></Hosts></response>",
    )
    .unwrap();
    assert_eq!(seen.value.len(), 1);
    assert_eq!(seen.value[0].active, None);
    assert_eq!(seen.value[0].name.as_deref(), Some("tv"));
    assert_eq!(seen.value[0].ip.as_deref(), Some("192.168.8.7"));
}

/// How a device is connected, and which one is this PC, from the H122's own fields.
#[test]
fn hosts_say_how_each_device_is_connected_and_which_is_this_pc() {
    let host = |mac: &str, extra: &str| {
        format!("<Host><MacAddress>00:00:5E:00:53:{mac}</MacAddress>{extra}</Host>")
    };
    let body = format!(
        "<response><Hosts>{}{}{}{}{}{}{}</Hosts></response>",
        host(
            "31",
            "<InterfaceType>Wireless</InterfaceType><AssociatedSsid>home</AssociatedSsid>\
             <isLocalDevice>0</isLocalDevice>"
        ),
        host(
            "32",
            "<InterfaceType>Ethernet</InterfaceType><AssociatedSsid></AssociatedSsid>\
             <isLocalDevice>1</isLocalDevice>"
        ),
        host("33", "<AssociatedSsid>home</AssociatedSsid>"),
        host("34", "<InterfaceType>WLAN 5G</InterfaceType>"),
        host("35", "<InterfaceType>Bridge</InterfaceType>"),
        host("36", ""),
        host(
            "37",
            "<InterfaceType>2.4G</InterfaceType><AssociatedSsid>home</AssociatedSsid>"
        ),
    );
    let seen = xml::parse_hosts(&body).unwrap();
    let got: Vec<(Option<DeviceLink>, bool)> =
        seen.value.iter().map(|d| (d.link, d.this_device)).collect();
    assert_eq!(
        got,
        [
            (Some(DeviceLink::Wifi), false),
            (Some(DeviceLink::Cable), true),
            // No type, but it is on a Wi-Fi network.
            (Some(DeviceLink::Wifi), false),
            (Some(DeviceLink::Wifi), false),
            // A word the app doesn't know says nothing by itself.
            (None, false),
            (None, false),
            // But a device on a Wi-Fi network is on Wi-Fi, whatever the word.
            (Some(DeviceLink::Wifi), false),
        ]
    );
}

#[test]
fn hosts_with_no_devices_are_an_empty_list() {
    for body in [
        "<response><Hosts></Hosts></response>",
        "<response><Hosts/></response>",
        "<response></response>",
    ] {
        assert!(xml::parse_hosts(body).unwrap().value.is_empty(), "{body}");
    }
}

#[test]
fn hosts_refuse_an_answer_that_is_not_the_routers() {
    for body in ["", "not xml <<", "<html><body>login</body></html>"] {
        assert!(
            matches!(xml::parse_hosts(body), Err(RouterError::Parse { .. })),
            "{body:?}"
        );
    }
}

/// Anyone on the Wi-Fi picks their own device name, so it is treated as hostile text.
#[test]
fn a_device_name_is_cleaned_and_capped() {
    assert_eq!(clean_name("  Office PC \t").as_deref(), Some("Office PC"));
    assert_eq!(
        clean_name("ev\u{202E}il\u{7}\u{0}\r\nname\u{2066}").as_deref(),
        Some("evilname")
    );
    assert_eq!(clean_name("").as_deref(), None);
    assert_eq!(clean_name(" \u{200F}\u{7} ").as_deref(), None);
    let long = clean_name(&"\u{0645}".repeat(200)).unwrap();
    assert_eq!(long.chars().count(), 64);
}

#[test]
fn a_hostile_name_in_the_routers_answer_is_cleaned_too() {
    let body = format!(
        "<response><Hosts><Host><MacAddress>00:00:5E:00:53:0A</MacAddress>\
         <HostName>ev&#x202E;il&#9;{}</HostName></Host></Hosts></response>",
        "x".repeat(200)
    );
    let seen = xml::parse_hosts(&body).unwrap();
    let name = seen.value[0].name.clone().unwrap();
    assert!(name.starts_with("evil"), "{name:?}");
    assert_eq!(name.chars().count(), 64);
    assert!(!name.chars().any(char::is_control));
}

#[test]
fn the_device_list_is_capped() {
    let mut body = String::from("<response><Hosts>");
    for i in 0..300u32 {
        body.push_str(&format!(
            "<Host><MacAddress>00:00:5E:00:{:02X}:{:02X}</MacAddress></Host>",
            i / 256,
            i % 256
        ));
    }
    body.push_str("</Hosts></response>");
    assert_eq!(xml::parse_hosts(&body).unwrap().value.len(), 256);
}

#[test]
fn mac_addresses_are_normalised_or_refused() {
    let cases: [(&str, Option<&str>); 8] = [
        ("00:00:5e:00:53:01", Some("00:00:5E:00:53:01")),
        ("00-00-5E-00-53-01", Some("00:00:5E:00:53:01")),
        (" 00:00:5E:00:53:01 ", Some("00:00:5E:00:53:01")),
        ("00005E005301", Some("00:00:5E:00:53:01")),
        ("00:00:5E:00:53", None),
        ("00:00:5E:00:53:01:02", None),
        ("zz:00:5E:00:53:01", None),
        ("", None),
    ];
    for (raw, want) in cases {
        assert_eq!(normalize_mac(raw).as_deref(), want, "{raw:?}");
    }
}

/// A private (randomised) Wi-Fi address has the "locally administered" bit: the second hex digit
/// is 2, 6, A or E.
#[test]
fn private_wifi_addresses_are_recognised() {
    for mac in [
        "02:00:5E:00:53:01",
        "06:00:5E:00:53:01",
        "DA:A1:19:00:53:02",
        "FE:00:5E:00:53:01",
    ] {
        assert!(is_private_mac(mac), "{mac}");
    }
    for mac in [
        "00:00:5E:00:53:01",
        "3C:5A:B4:00:53:01",
        "F4:00:5E:00:53:01",
        "garbage",
    ] {
        assert!(!is_private_mac(mac), "{mac}");
    }
}

#[test]
fn a_failed_read_is_named_by_how_it_failed() {
    use PrivacyOutcome::*;
    let api = |code, kind| RouterError::Api { code, kind };
    let cases = [
        (
            RouterError::SessionLost { code: Some(100003) },
            Refused,
            Some(100003),
        ),
        (
            RouterError::SessionLost { code: Some(125002) },
            Refused,
            Some(125002),
        ),
        (RouterError::SessionLost { code: None }, Refused, None),
        (
            api(100002, ApiErrorKind::NotSupported),
            NotOffered,
            Some(100002),
        ),
        (api(100005, ApiErrorKind::BadParam), Refused, Some(100005)),
        (api(100004, ApiErrorKind::Busy), Refused, Some(100004)),
        (
            RouterError::Validation {
                message: privacy::DEVICES_NOT_SUPPORTED.to_string(),
            },
            NotSupported,
            None,
        ),
        (RouterError::Unsupported, NotSupported, None),
        (
            RouterError::Parse {
                message: "x".to_string(),
            },
            Unreadable,
            None,
        ),
        (RouterError::Timeout, NoAnswer, None),
        (RouterError::Unreachable, NoAnswer, None),
    ];
    for (error, outcome, code) in cases {
        assert_eq!(
            PrivacyRead::from_error(&error),
            PrivacyRead { outcome, code },
            "{error:?}"
        );
    }
}

async fn mock() -> MockDriver {
    let m = MockDriver::new(MockScenario::zain_like());
    m.login(&Credentials::new("admin", "mock")).await.unwrap();
    m
}

#[tokio::test]
async fn a_check_reads_remote_management_and_the_devices() {
    let m = mock().await;
    let (report, _) = privacy::check(&m).await;
    assert_eq!(report.remote_management.outcome, PrivacyOutcome::Ok);
    assert_eq!(
        report.management.unwrap().server_host.as_deref(),
        Some("acs.example.net")
    );
    assert_eq!(report.devices_read.outcome, PrivacyOutcome::Ok);
    assert_eq!(report.devices_source, Some(DeviceSource::Lan));
    assert!(report.devices.len() >= 3);
    assert!(report.devices.iter().any(|d| d.private_mac));
    assert!(report.devices.iter().any(|d| d.active == Some(false)));
    // The LAN list answered, so the Wi-Fi list is never asked for.
    assert_eq!(m.wifi_device_reads(), 0);
}

/// Which list answered decides what the list means (every remembered device, or only the ones on
/// Wi-Fi now), and the log must show what the router did with each path.
#[tokio::test]
async fn the_wifi_list_is_used_when_the_lan_list_is_refused() {
    let m = mock().await;
    m.set_devices_error(Some(RouterError::SessionLost { code: Some(100003) }));
    let (report, detail) = privacy::check(&m).await;
    assert_eq!(report.devices_read, PrivacyRead::ok());
    assert_eq!(report.devices_source, Some(DeviceSource::Wifi));
    assert!(!report.devices.is_empty());
    assert!(report.devices.iter().all(|d| d.active.is_none()));
    assert_eq!(detail["devices"]["result"], "ok");
    assert_eq!(detail["devices"]["source"], "wifi");
    assert_eq!(detail["devices"]["lan"]["result"], "refused");
    assert_eq!(detail["devices"]["lan"]["code"], 100003);
}

#[tokio::test]
async fn both_lists_failing_reports_the_first_and_logs_both() {
    let m = mock().await;
    m.set_devices_error(Some(RouterError::Api {
        code: 100002,
        kind: ApiErrorKind::NotSupported,
    }));
    m.set_wifi_devices_error(Some(RouterError::SessionLost { code: Some(100003) }));
    let (report, detail) = privacy::check(&m).await;
    assert_eq!(
        report.devices_read,
        PrivacyRead {
            outcome: PrivacyOutcome::NotOffered,
            code: Some(100002)
        }
    );
    assert_eq!(report.devices_source, None);
    assert!(report.devices.is_empty());
    assert_eq!(detail["devices"]["result"], "notOffered");
    assert_eq!(detail["devices"]["code"], 100002);
    assert_eq!(detail["devices"]["wifi"]["result"], "refused");
    assert_eq!(detail["devices"]["wifi"]["code"], 100003);
    assert!(detail["devices"].get("source").is_none());
}

#[tokio::test]
async fn a_router_that_does_not_answer_is_not_asked_twice() {
    let m = mock().await;
    m.set_devices_error(Some(RouterError::Timeout));
    let (report, detail) = privacy::check(&m).await;
    assert_eq!(report.devices_read.outcome, PrivacyOutcome::NoAnswer);
    assert_eq!(m.wifi_device_reads(), 0);
    assert!(detail["devices"].get("wifi").is_none());
}

/// The promise to the user: a check leaves how each read ended, the router's code and the
/// element names in the log, and none of what it read.
#[tokio::test]
async fn a_check_logs_outcomes_and_names_but_no_values() {
    let m = mock().await;
    let (report, detail) = privacy::check(&m).await;
    assert_eq!(detail["remoteManagement"]["result"], "ok");
    assert_eq!(detail["devices"]["result"], "ok");
    assert_eq!(detail["devices"]["source"], "lan");
    assert!(detail["devices"].get("lan").is_none());
    assert!(detail["remoteManagement"]["tags"]
        .as_array()
        .is_some_and(|t| !t.is_empty()));
    assert!(detail["devices"]["tags"]
        .as_array()
        .is_some_and(|t| !t.is_empty()));
    let logged = detail.to_string();
    let host = report.management.unwrap().server_host.unwrap();
    assert!(!logged.contains(&host), "server name logged: {logged}");
    for d in &report.devices {
        assert!(!logged.contains(&d.mac), "MAC logged: {logged}");
        for private in [d.name.as_deref(), d.ip.as_deref()].into_iter().flatten() {
            assert!(!logged.contains(private), "{private} logged: {logged}");
        }
    }
    // No count either: the log must not become a record of who was home.
    assert!(
        !logged.contains(&format!(":{}", report.devices.len())),
        "{logged}"
    );
    assert!(detail["devices"].get("count").is_none());
}

#[tokio::test]
async fn a_refused_read_is_reported_and_the_other_still_runs() {
    let m = mock().await;
    m.set_remote_management_error(Some(RouterError::SessionLost { code: Some(100003) }));
    let (report, detail) = privacy::check(&m).await;
    assert_eq!(
        report.remote_management,
        PrivacyRead {
            outcome: PrivacyOutcome::Refused,
            code: Some(100003)
        }
    );
    assert_eq!(report.management, None);
    assert_eq!(report.devices_read.outcome, PrivacyOutcome::Ok);
    assert!(!report.devices.is_empty());
    assert_eq!(detail["remoteManagement"]["result"], "refused");
    assert_eq!(detail["remoteManagement"]["code"], 100003);
    assert!(detail["remoteManagement"].get("tags").is_none());
}

/// A driver that never learned these reads refuses without a request: "not supported".
#[tokio::test]
async fn a_driver_without_the_reads_says_not_supported() {
    struct Bare(MockDriver);
    #[async_trait::async_trait]
    impl RouterDriver for Bare {
        fn info(&self) -> router_core::DriverInfo {
            self.0.info()
        }
        fn apply_preview(&self, cfg: &router_core::NetConfig) -> router_core::ApplyPreview {
            self.0.apply_preview(cfg)
        }
        async fn login(&self, creds: &Credentials) -> Result<(), RouterError> {
            self.0.login(creds).await
        }
        async fn session_state(&self) -> Result<router_core::SessionState, RouterError> {
            self.0.session_state().await
        }
        async fn heartbeat(&self) -> Result<(), RouterError> {
            self.0.heartbeat().await
        }
        async fn capabilities(&self) -> Result<router_core::Capabilities, RouterError> {
            self.0.capabilities().await
        }
        async fn device_info(&self) -> Result<router_core::DeviceInfo, RouterError> {
            self.0.device_info().await
        }
        async fn status(&self) -> Result<router_core::Status, RouterError> {
            self.0.status().await
        }
        async fn signal(&self) -> Result<router_core::Signal, RouterError> {
            self.0.signal().await
        }
        async fn net_config(&self) -> Result<router_core::NetConfig, RouterError> {
            self.0.net_config().await
        }
        async fn apply_net_config(&self, cfg: &router_core::NetConfig) -> Result<(), RouterError> {
            self.0.apply_net_config(cfg).await
        }
        async fn reboot(&self) -> Result<(), RouterError> {
            self.0.reboot().await
        }
    }
    let (report, _) = privacy::check(&Bare(mock().await)).await;
    assert_eq!(
        report.remote_management,
        PrivacyRead {
            outcome: PrivacyOutcome::NotSupported,
            code: None
        }
    );
    assert_eq!(report.devices_read.outcome, PrivacyOutcome::NotSupported);
}

/// A check's log line carries element names such as `MacAddress` and `AcsUserPassword`. They are
/// names, not values, and must not stop the diagnostics export or be taken out of it.
#[tokio::test]
async fn the_diagnostics_export_accepts_a_check_in_the_log() {
    let m = mock().await;
    m.set_devices_error(Some(RouterError::SessionLost { code: Some(100003) }));
    let (_, detail) = privacy::check(&m).await;
    let events = [router_core::EventRecord {
        at_ms: 1,
        kind: router_core::event_text::kinds::PRIVACY_CHECKED.to_string(),
        detail,
    }];
    let text = router_core::diagnostics::build_report("1.0.0", "mock", "Mock", 0, &[], &events)
        .expect("a privacy check in the log stopped the export");
    let report: serde_json::Value = serde_json::from_str(&text).unwrap();
    let logged = report.to_string();
    assert!(logged.contains("privacy.checked"), "{logged}");
    assert!(logged.contains("MacAddress"), "{logged}");
    assert!(logged.contains("100003"), "{logged}");
}

/// An account written with a `/` in it must never be read as the host. A stray `@` after the
/// host leaves it unknown, which is the safe way to be wrong.
#[test]
fn server_host_is_unknown_when_an_account_could_hide_before_it() {
    for url in [
        "https://user:pa/ss@acs.example.net/",
        "https://secret/x@host/",
        "http://acs.example.net/cwmp?to=a@b",
        "http://acs.example.net#a@b",
    ] {
        assert_eq!(server_host(url), None, "{url:?}");
    }
}

/// The element names are the router's choice, so one shaped like an address is left out of the
/// log: the diagnostics export reads the event log, and its leak scan would refuse to run.
#[test]
fn an_element_named_like_an_address_is_not_logged() {
    let body = "<?xml version=\"1.0\" encoding=\"UTF-8\"?><response>\
<Enable>1</Enable>\
<AABBCCDDEEFF>1</AABBCCDDEEFF>\
<AcsUrl>http://acs.example.net</AcsUrl>\
</response>";
    let seen = xml::parse_cwmp(body).unwrap();
    assert_eq!(seen.tags, ["Enable", "AcsUrl"]);
    assert_eq!(seen.value.server_host.as_deref(), Some("acs.example.net"));
    assert!(router_core::diagnostics::identifier_hits(&seen.tags.join(" ")).is_empty());
}

/// A name made only of characters nobody can see reads as no name. Inside a name they stay:
/// Arabic-script names and emoji are written with joiners.
#[test]
fn a_name_with_nothing_visible_is_no_name() {
    for blank in [
        "\u{200B}\u{200B}",
        "\u{200D}",
        " \u{2060}\u{00AD} ",
        "\u{E0041}\u{E0042}",
        "\u{FE0F}",
    ] {
        assert_eq!(clean_name(blank), None, "{blank:?}");
    }
    assert_eq!(
        clean_name("\u{0645}\u{200C}\u{0646}").as_deref(),
        Some("\u{0645}\u{200C}\u{0646}")
    );
    assert_eq!(clean_name("a\u{200D}b").as_deref(), Some("a\u{200D}b"));
}
