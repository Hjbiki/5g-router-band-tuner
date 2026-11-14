//! The read-only privacy check on the Router page: what the provider can reach on the router and
//! the devices it remembers (`router_core::privacy`), and which DNS server answers this PC.
//!
//! Both run only when the user presses their button. The router reads are GETs through the
//! driver and change nothing. The DNS check is one ordinary name lookup through Windows, the only
//! step that leaves the local network. Nothing either reads is stored: the event log gets the
//! router reads' outcomes, codes and element names, and nothing at all about the DNS check.

use std::net::IpAddr;
use std::time::Duration;

use router_core::privacy::PrivacyReport;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::commands::{AppState, CmdError};

/// Akamai's name that answers with the address of the DNS server that asked for it.
const WHOAMI_HOST: &str = "whoami.akamai.net";
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// What the demo driver shows instead of a real lookup (a documentation address).
const DEMO_RESOLVER: &str = "192.0.2.53";

/// Which DNS server answers this PC's lookups. For the open window only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DnsCheck {
    /// The server's address as the lookup returned it; `None` when it gave nothing usable.
    pub resolver: Option<String>,
}

/// The address to show from a lookup's answers: the first public IPv4 one, else the first public
/// IPv6 one. A private, loopback, link-local or reserved answer isn't a DNS server's address (a
/// captive portal or a filter answered instead), so it counts as no answer. An IPv4 address
/// written the IPv6 way (`::ffff:a.b.c.d`) is judged and shown as the IPv4 address it is.
fn pick_resolver(addrs: &[IpAddr]) -> Option<IpAddr> {
    let addrs: Vec<IpAddr> = addrs.iter().map(IpAddr::to_canonical).collect();
    let public = |v4: bool| {
        addrs
            .iter()
            .copied()
            .find(|ip| ip.is_ipv4() == v4 && is_public(ip))
    };
    public(true).or_else(|| public(false))
}

fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            // 0.x is "this network" and 240.x and up is reserved, the broadcast address included.
            let first = v4.octets()[0];
            !(first == 0
                || first >= 240
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_multicast())
                && router_core::wan_kind(*v4) == router_core::WanKind::Public
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            // fc00::/7 is private (unique local) and fe80::/10 is link-local.
            !(v6.is_unspecified()
                || v6.is_loopback()
                || v6.is_multicast()
                || first & 0xfe00 == 0xfc00
                || first & 0xffc0 == 0xfe80)
        }
    }
}

/// Reads remote management and the device list from the router, when the user asks. Refused as
/// a whole unless logged in with nothing holding the router; after that each read ends in its
/// own outcome, so a refusal by the router is shown beside that read and never treated as a
/// lost session. Logs `privacy.checked`.
#[tauri::command]
pub async fn privacy_check(
    state: State<'_, AppState>,
    scan: State<'_, crate::autoscan::ScanState>,
    st: State<'_, crate::speedtest::SpeedTestState>,
) -> Result<PrivacyReport, CmdError> {
    let (driver, snapshot) = state.driver_and_snapshot();
    if let Some(why) = crate::commands::router_busy_refusal(
        &snapshot,
        scan.is_running(),
        st.is_running(),
        state.page_scripts_running(),
    ) {
        return Err(why.to_string().into());
    }
    let (report, detail) = router_core::privacy::check(driver.as_ref()).await;
    let dir = state.data_dir().to_path_buf();
    // A failed append only loses the log line.
    let _ = tokio::task::spawn_blocking(move || {
        crate::history::log_event(
            &dir,
            router_core::event_text::kinds::PRIVACY_CHECKED,
            detail,
        )
    })
    .await;
    Ok(report)
}

/// Looks up `WHOAMI_HOST` through Windows, so the answer is the DNS server this PC really uses
/// (encrypted DNS included). The demo driver never contacts anything and shows a made-up address.
#[tauri::command]
pub async fn dns_check(state: State<'_, AppState>) -> Result<DnsCheck, CmdError> {
    if state.is_demo() {
        return Ok(DnsCheck {
            resolver: Some(DEMO_RESOLVER.to_string()),
        });
    }
    let lookup = tokio::task::spawn_blocking(|| {
        use std::net::ToSocketAddrs;
        (WHOAMI_HOST, 0)
            .to_socket_addrs()
            .map(|found| found.map(|a| a.ip()).collect::<Vec<_>>())
            .unwrap_or_default()
    });
    let addrs = match tokio::time::timeout(LOOKUP_TIMEOUT, lookup).await {
        Ok(Ok(addrs)) => addrs,
        _ => Vec::new(),
    };
    Ok(DnsCheck {
        resolver: pick_resolver(&addrs).map(|ip| ip.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ips(list: &[&str]) -> Vec<IpAddr> {
        list.iter().map(|s| s.parse().unwrap()).collect()
    }

    #[test]
    fn the_first_public_ipv4_answer_is_the_resolver() {
        let cases: [(&[&str], Option<&str>); 9] = [
            (&["203.0.113.5"], Some("203.0.113.5")),
            (&["2001:db8::5", "203.0.113.5"], Some("203.0.113.5")),
            (&["203.0.113.5", "198.51.100.9"], Some("203.0.113.5")),
            (&["2001:db8::5"], Some("2001:db8::5")),
            (&[], None),
            // A filter or captive portal answered with a local address: not a DNS server.
            (&["192.168.8.1"], None),
            (&["10.0.0.1", "127.0.0.1", "169.254.1.1", "0.0.0.0"], None),
            (&["100.64.0.1"], None),
            (&["::1", "fe80::1", "fd00::1", "::"], None),
        ];
        for (answers, want) in cases {
            assert_eq!(
                pick_resolver(&ips(answers))
                    .map(|ip| ip.to_string())
                    .as_deref(),
                want,
                "{answers:?}"
            );
        }
    }

    /// Addresses no DNS server can have, and a local address written as IPv6.
    #[test]
    fn reserved_and_mapped_local_answers_are_not_a_resolver() {
        for answer in [
            "0.1.2.3",
            "240.0.0.1",
            "255.255.255.255",
            "::ffff:192.168.8.1",
            "::ffff:10.0.0.1",
            "::ffff:127.0.0.1",
        ] {
            assert_eq!(pick_resolver(&ips(&[answer])), None, "{answer}");
        }
        // A public IPv4 address written the IPv6 way is shown as the IPv4 address it is.
        assert_eq!(
            pick_resolver(&ips(&["::ffff:203.0.113.5"])).map(|ip| ip.to_string()),
            Some("203.0.113.5".to_string())
        );
    }

    #[test]
    fn a_local_answer_is_skipped_for_a_public_one_after_it() {
        assert_eq!(
            pick_resolver(&ips(&["192.168.8.1", "203.0.113.5"])).map(|ip| ip.to_string()),
            Some("203.0.113.5".to_string())
        );
    }
}
