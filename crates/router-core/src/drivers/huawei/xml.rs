//! Huawei response parsing. Pure functions over response bodies.

use std::collections::HashMap;

use quick_xml::events::Event;
use quick_xml::Reader;

use super::errors;
use crate::bands::nr::resolve_nr_band;
use crate::{
    Capabilities, ConnState, DeviceInfo, LteBandSet, LteSignal, NetConfig, NetworkMode, NrOption,
    NrSignal, Operator, Rat, RouterError, SessionState, Signal, Status,
};

fn parse_err(e: impl std::fmt::Display) -> RouterError {
    RouterError::Parse {
        message: e.to_string(),
    }
}

/// Text carried by one event. Since quick-xml 0.38 a character or predefined entity reference
/// (`&#40;`, `&amp;`) is its own `GeneralRef` event between `Text` events, so an element's text
/// is the concatenation of both kinds. The H122 sends `&#40;`/`&#41;` in its version strings.
fn event_text(ev: &Event<'_>) -> Result<Option<String>, RouterError> {
    match ev {
        Event::Text(t) => Ok(Some(t.decode().map_err(parse_err)?.into_owned())),
        Event::GeneralRef(r) => {
            if let Some(c) = r.resolve_char_ref().map_err(parse_err)? {
                return Ok(Some(c.to_string()));
            }
            let name = r.decode().map_err(parse_err)?;
            quick_xml::escape::resolve_predefined_entity(&name)
                .map(|s| Some(s.to_string()))
                .ok_or_else(|| parse_err(format!("unknown entity &{name};")))
        }
        _ => Ok(None),
    }
}

/// Appends `ev`'s text to the innermost open element's buffer.
fn collect_text(ev: &Event<'_>, texts: &mut [Option<String>]) -> Result<(), RouterError> {
    if let (Some(text), Some(buf)) = (event_text(ev)?, texts.last_mut()) {
        buf.get_or_insert_with(String::new).push_str(&text);
    }
    Ok(())
}

/// Flattens a response into leaf-element-name -> decoded text (first non-empty occurrence wins).
/// With a non-empty `only`, other elements are never stored.
pub(crate) fn flat(
    body: &str,
    only: &[&str],
) -> Result<(String, HashMap<String, String>), RouterError> {
    let mut reader = Reader::from_str(body);
    let mut root = String::new();
    let mut stack: Vec<String> = Vec::new();
    // Text of each open element, parallel to `stack`.
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut map: HashMap<String, String> = HashMap::new();
    let wanted = |n: &str| only.is_empty() || only.contains(&n);
    loop {
        let ev = reader.read_event().map_err(parse_err)?;
        match &ev {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if root.is_empty() {
                    root = name.clone();
                }
                if wanted(&name) {
                    map.entry(name.clone()).or_default();
                }
                stack.push(name);
                texts.push(None);
            }
            Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if root.is_empty() {
                    root = name.clone();
                }
                if wanted(&name) {
                    map.entry(name).or_default();
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => collect_text(&ev, &mut texts)?,
            Event::End(_) => {
                let (name, text) = (stack.pop(), texts.pop().flatten());
                // The root's own text is never a value.
                if let (Some(name), Some(text)) = (name, text) {
                    if !stack.is_empty() && wanted(&name) {
                        if let Some(slot) = map.get_mut(&name) {
                            if slot.is_empty() && !text.trim().is_empty() {
                                *slot = text.trim().to_string();
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Ok((root, map))
}

pub(crate) fn get<'a>(m: &'a HashMap<String, String>, k: &str) -> &'a str {
    m.get(k).map(String::as_str).unwrap_or("")
}

/// Detects `<error><code>N</code></error>` inside an HTTP 200 body.
pub fn check_error(body: &str) -> Result<(), RouterError> {
    let (root, m) = flat(body, &["code"])?;
    if root == "error" {
        let code = get(&m, "code").parse::<u32>().unwrap_or(0);
        return Err(RouterError::Api {
            code,
            kind: errors::kind(code),
        });
    }
    Ok(())
}

/// First signed decimal in the string (`>=-44dBm` -> -44.0). No digits -> `None`.
pub fn parse_number(raw: &str) -> Option<f32> {
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let neg = b[i] == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_digit);
        if b[i].is_ascii_digit() || neg {
            let start = i;
            i += 1;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            return raw[start..i].parse::<f64>().ok().map(|v| v as f32);
        }
        i += 1;
    }
    None
}

/// Integer following `key` (for example `Code0:` in `mcsDownCarrier1Code0:14`).
fn kv_int(raw: &str, key: &str) -> Option<i64> {
    let rest = &raw[raw.find(key)? + key.len()..];
    parse_number(rest.split_whitespace().next()?).map(|v| v as i64)
}

fn bandwidth_mhz(raw: &str) -> Option<f32> {
    let n = f64::from(parse_number(raw)?);
    let lower = raw.to_ascii_lowercase();
    let mhz = if lower.contains("khz") { n / 1000.0 } else { n };
    Some(mhz as f32)
}

fn uint<T: TryFrom<i64>>(raw: &str) -> Option<T> {
    parse_number(raw).and_then(|v| T::try_from(v as i64).ok())
}

fn nonzero(v: Option<i64>) -> Option<u32> {
    v.filter(|n| *n > 0).and_then(|n| u32::try_from(n).ok())
}

fn mcs(v: Option<i64>) -> Option<u8> {
    v.and_then(|n| u8::try_from(n).ok())
}

fn cqi(raw: &str) -> Option<u8> {
    match parse_number(raw)? as i64 {
        32639 => None,
        n => u8::try_from(n).ok(),
    }
}

fn parse_cell(cell: &str, enodeb: &str) -> (Option<u64>, Option<u32>, Option<u8>) {
    let mut cell_id = None;
    let mut enb = None;
    let mut sector = None;
    let cell = cell.trim();
    if let Some((e, s)) = cell.split_once('-') {
        if let (Ok(e), Ok(s)) = (e.trim().parse::<u32>(), s.trim().parse::<u8>()) {
            enb = Some(e);
            sector = Some(s);
            cell_id = Some(u64::from(e) * 256 + u64::from(s));
        }
    } else if let Ok(c) = cell.parse::<u64>() {
        cell_id = Some(c);
        enb = u32::try_from(c >> 8).ok();
        sector = Some((c & 0xFF) as u8);
    }
    if let Ok(e) = enodeb.trim().parse::<u32>() {
        enb = Some(e);
    }
    (cell_id, enb, sector)
}

pub fn parse_signal(body: &str) -> Result<Signal, RouterError> {
    let (_, m) = flat(body, &[])?;
    let g = |k: &str| get(&m, k);

    let earfcn = g("earfcn");
    let (cell_id, enb_id, sector) = parse_cell(g("cell_id"), g("enodeb_id"));
    let lte = LteSignal {
        band: uint(g("band")),
        earfcn_dl: nonzero(kv_int(earfcn, "DL:")),
        earfcn_ul: nonzero(kv_int(earfcn, "UL:")),
        pci: uint(g("pci")),
        cell_id,
        enb_id,
        sector,
        rsrp: parse_number(g("rsrp")),
        rsrq: parse_number(g("rsrq")),
        sinr: parse_number(g("sinr")),
        rssi: parse_number(g("rssi")),
        tx_power: kv_int(g("txpower"), "PPusch:").map(|v| v as f32),
        dl_bw_mhz: bandwidth_mhz(g("dlbandwidth")),
        ul_bw_mhz: bandwidth_mhz(g("ulbandwidth")),
        dl_mcs: [
            mcs(kv_int(g("dl_mcs"), "Code0:")),
            mcs(kv_int(g("dl_mcs"), "Code1:")),
        ],
        ul_mcs: mcs(kv_int(g("ul_mcs"), "UpCarrier1:")),
        cqi: [cqi(g("cqi0")), cqi(g("cqi1"))],
        scc_pci: uint(g("scc_pci")),
    };
    let lte_present = g("mode").trim() == "7"
        || lte.rsrp.is_some()
        || lte.rsrq.is_some()
        || lte.sinr.is_some()
        || lte.rssi.is_some()
        || lte.pci.is_some()
        || lte.cell_id.is_some()
        || lte.earfcn_dl.is_some();

    let nr_earfcn = g("nrearfcn");
    let nr_dl = nonzero(kv_int(nr_earfcn, "DL:"));
    let nr_ul = nonzero(kv_int(nr_earfcn, "UL:"));
    let nr_present = nr_dl.is_some()
        || ["nrrsrp", "nrrsrq", "nrsinr"]
            .iter()
            .any(|k| parse_number(g(k)).is_some());
    let nr = nr_present.then(|| NrSignal {
        arfcn_dl: nr_dl,
        arfcn_ul: nr_ul,
        band: nr_dl.and_then(|dl| resolve_nr_band(dl, nr_ul)),
        pci: uint(g("nrpci")),
        rsrp: parse_number(g("nrrsrp")),
        rsrq: parse_number(g("nrrsrq")),
        sinr: parse_number(g("nrsinr")),
        tx_power: kv_int(g("nrtxpower"), "PPusch:").map(|v| v as f32),
        dl_bw_mhz: bandwidth_mhz(g("nrdlbandwidth")),
        ul_bw_mhz: bandwidth_mhz(g("nrulbandwidth")),
        dl_mcs: [
            mcs(kv_int(g("nrdlmcs"), "Code0:")),
            mcs(kv_int(g("nrdlmcs"), "Code1:")),
        ],
        ul_mcs: mcs(kv_int(g("nrulmcs"), "UpCarrier1:")),
        cqi: [cqi(g("nrcqi0")), cqi(g("nrcqi1"))],
    });

    let serving = match g("mode").trim() {
        "0" => Some(Rat::Gsm),
        "2" => Some(Rat::Umts),
        "7" => Some(Rat::Lte),
        _ if nr.is_some() => Some(Rat::Nr),
        _ => None,
    };
    Ok(Signal {
        serving,
        lte: lte_present.then_some(lte),
        nr,
    })
}

/// `1` or `true`, `0` or `false`. Anything else, or nothing, is unknown.
fn flag(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

pub fn parse_status(body: &str) -> Result<Status, RouterError> {
    let (_, m) = flat(body, &[])?;
    let conn: u32 = get(&m, "ConnectionStatus")
        .parse()
        .map_err(|_| parse_err("missing ConnectionStatus"))?;
    let net_type = match get(&m, "CurrentNetworkTypeEx") {
        "" => get(&m, "CurrentNetworkType"),
        v => v,
    };
    Ok(Status {
        connection: ConnState::from_code(conn),
        network_type: net_type.parse().unwrap_or(0),
        operator: None,
        bars: get(&m, "SignalIcon").parse().ok(),
        nr_bars: get(&m, "SignalIconNr").parse().ok(),
        endc: flag(get(&m, "EndcStatus")),
        endc_restricted: flag(get(&m, "endcRestrictedStatus")),
    })
}

/// The SIM's name, if it says anything. Bounded: it comes from the SIM and goes to the screen.
pub fn clean_spn(raw: &str) -> Option<String> {
    let s: String = raw
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(40)
        .collect();
    (!s.is_empty()).then_some(s)
}

/// MCC and MNC from an IMSI, and nothing else of it. Only for Saudi SIMs, where the MNC is always
/// 2 digits; elsewhere its length varies, so no guess is made.
pub fn home_plmn(imsi: &str) -> Option<String> {
    let imsi = imsi.trim();
    let plausible = (14..=15).contains(&imsi.len()) && imsi.bytes().all(|b| b.is_ascii_digit());
    (plausible && imsi.starts_with("420")).then(|| imsi[..5].to_string())
}

pub fn parse_operator(body: &str) -> Result<Option<Operator>, RouterError> {
    let (_, m) = flat(body, &["FullName", "ShortName", "Numeric", "Spn"])?;
    let name = match get(&m, "FullName") {
        "" => get(&m, "ShortName"),
        v => v,
    };
    let mcc_mnc = get(&m, "Numeric");
    if name.is_empty() && mcc_mnc.is_empty() {
        return Ok(None);
    }
    Ok(Some(Operator {
        name: name.to_string(),
        mcc_mnc: mcc_mnc.to_string(),
        spn: clean_spn(get(&m, "Spn")),
    }))
}

pub fn parse_net_mode(body: &str) -> Result<NetConfig, RouterError> {
    let (_, m) = flat(body, &[])?;
    let mode = get(&m, "NetworkMode");
    if mode.is_empty() {
        return Err(parse_err("net-mode: missing NetworkMode"));
    }
    Ok(NetConfig {
        network_mode: NetworkMode::from_code(mode),
        lte_bands: LteBandSet::from_huawei_hex(get(&m, "LTEBand"))?,
        legacy_band_raw: get(&m, "NetworkBand").to_string(),
        nr_option: get(&m, "networkOption")
            .parse::<u8>()
            .ok()
            .and_then(NrOption::from_code),
    })
}

type ModeList = (Vec<NetworkMode>, Option<LteBandSet>);

/// Walks `net-mode-list`; `None` on malformed XML.
fn scan_net_mode_list(body: &str) -> Option<ModeList> {
    let mut reader = Reader::from_str(body);
    let mut stack: Vec<String> = Vec::new();
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut modes = Vec::new();
    let mut supported: Option<LteBandSet> = None;
    let (mut name, mut value) = (String::new(), String::new());
    loop {
        let ev = reader.read_event().ok()?;
        match &ev {
            Event::Start(e) => {
                let n = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if n == "LTEBand" {
                    name.clear();
                    value.clear();
                }
                stack.push(n);
                texts.push(None);
            }
            Event::Text(_) | Event::GeneralRef(_) => collect_text(&ev, &mut texts).ok()?,
            Event::End(e) => {
                // An element's text is complete at its end tag; `stack` still holds it here.
                if let Some(text) = texts.pop().flatten() {
                    let text = text.trim().to_string();
                    match stack.last().map(String::as_str) {
                        Some("Access") if stack.iter().any(|s| s == "AccessList") => {
                            modes.push(NetworkMode::from_code(&text));
                        }
                        Some("Name") if stack.iter().any(|s| s == "LTEBand") => name = text,
                        Some("Value") if stack.iter().any(|s| s == "LTEBand") => value = text,
                        _ => {}
                    }
                }
                if e.name().as_ref() == b"LTEBand" && supported.is_none() && name != "All bands" {
                    supported = LteBandSet::from_huawei_hex(&value).ok();
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    Some((modes, supported))
}

pub fn parse_capabilities(body: &str, net_mode: &NetConfig) -> Capabilities {
    let (modes, supported) = scan_net_mode_list(body).unwrap_or_default();
    let nr_options = if net_mode.nr_option.is_some() {
        vec![NrOption::Nsa, NrOption::Sa, NrOption::SaNsa]
    } else {
        Vec::new()
    };
    Capabilities {
        band_edit_allowed: supported.is_some(),
        supported_lte: supported.unwrap_or(LteBandSet::AUTO),
        modes,
        nr_options,
        // Confirmed on the H122-373 (fw 11.0.2.21): the user's bookmarklet resets it with
        // `Control` = 2, and reboot (`Control` = 1, same endpoint) is device-tested. A future
        // Huawei model that shares this driver needs its own confirmation. See docs/API_NOTES.md §7.
        factory_reset_verified: true,
        data_switch: false,
        ip_methods: Vec::new(),
        // Reading, switching (with automatic undo), and editing profiles that aren't in use.
        // `confirmed` stays false until the calls are seen working on this firmware.
        apn: crate::apn::ApnSupport {
            read: true,
            switch: true,
            edit: true,
            confirmed: false,
        },
    }
}

pub fn parse_state_login(body: &str) -> Result<SessionState, RouterError> {
    let (_, m) = flat(body, &["State", "lockstatus", "remainwaittime"])?;
    if get(&m, "lockstatus") == "1" {
        return Ok(SessionState::Locked {
            wait_s: get(&m, "remainwaittime").parse().unwrap_or(0),
        });
    }
    Ok(if get(&m, "State") == "0" {
        SessionState::LoggedIn
    } else {
        SessionState::LoggedOut
    })
}

/// Reads ONLY DeviceName, SoftwareVersion and WebUIVersion.
pub fn parse_device_info(body: &str) -> Result<DeviceInfo, RouterError> {
    let (_, m) = flat(
        body,
        &["DeviceName", "SoftwareVersion", "WebUIVersion", "Imsi"],
    )?;
    if get(&m, "DeviceName").is_empty() {
        return Err(parse_err("device information: missing DeviceName"));
    }
    Ok(DeviceInfo {
        model: get(&m, "DeviceName").to_string(),
        firmware: get(&m, "SoftwareVersion").to_string(),
        web_ui: get(&m, "WebUIVersion").to_string(),
        home_plmn: home_plmn(get(&m, "Imsi")),
    })
}

/// Reads ONLY WanIPAddress and WanIPv6Address from device information. An empty, all-zero or
/// unparsable IPv4 address reads as none (no data connection); nothing of the IPv6 address is
/// kept but whether there is one.
pub fn parse_wan(body: &str) -> Result<crate::WanAddress, RouterError> {
    let (_, m) = flat(body, &["WanIPAddress", "WanIPv6Address"])?;
    let ipv4 = get(&m, "WanIPAddress")
        .trim()
        .parse::<std::net::Ipv4Addr>()
        .ok()
        .filter(|ip| !ip.is_unspecified());
    let v6 = get(&m, "WanIPv6Address").trim();
    let ipv6_present = v6
        .parse::<std::net::Ipv6Addr>()
        .is_ok_and(|ip| !ip.is_unspecified());
    Ok(crate::WanAddress { ipv4, ipv6_present })
}

/// `/api/dialup/profiles`: `CurrentProfile` and each `Profile`. A profile's `Password` text is
/// never kept, only whether it has any; the body itself is never logged. A profile without a
/// usable `Index` is skipped. `auto_apn` is left unknown (see `parse_auto_apn`).
pub fn parse_apn_profiles(body: &str) -> Result<crate::apn::ApnProfiles, RouterError> {
    use crate::apn::RawProfile;
    let mut reader = Reader::from_str(body);
    let mut root = String::new();
    let mut stack: Vec<String> = Vec::new();
    let mut texts: Vec<Option<String>> = Vec::new();
    let mut current: Option<u32> = None;
    let mut profiles = Vec::new();
    let mut raw: Option<RawProfile> = None;
    loop {
        let ev = reader.read_event().map_err(parse_err)?;
        match &ev {
            Event::Start(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if root.is_empty() {
                    root = name.clone();
                }
                if name == "Profile" {
                    raw = Some(RawProfile::default());
                }
                stack.push(name);
                texts.push(None);
            }
            Event::Empty(e) => {
                if root.is_empty() {
                    root = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => collect_text(&ev, &mut texts)?,
            Event::End(_) => {
                let name = stack.pop().unwrap_or_default();
                let text = texts.pop().flatten().unwrap_or_default();
                if name == "Profile" {
                    if let Some(p) = raw.take().and_then(RawProfile::build) {
                        profiles.push(p);
                    }
                    continue;
                }
                if let Some(p) = raw.as_mut() {
                    match name.as_str() {
                        "Index" => p.index = text,
                        "IsValid" => p.is_valid = text,
                        "Name" => p.name = text,
                        "ApnIsStatic" => p.apn_is_static = text,
                        "ApnName" => p.apn_name = text,
                        "Username" => p.username = text,
                        // Only whether there is one: the text is dropped right here.
                        "Password" => p.has_password = !text.trim().is_empty(),
                        "AuthMode" => p.auth_mode = text,
                        "ReadOnly" => p.read_only = text,
                        "iptype" => p.ip_type = text,
                        _ => {}
                    }
                } else if name == "CurrentProfile" {
                    current = text.trim().parse().ok();
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if root == "error" {
        return Err(parse_err("dialup profiles: error response"));
    }
    if root != "response" {
        return Err(parse_err("dialup profiles: unexpected response"));
    }
    Ok(crate::apn::ApnProfiles {
        current,
        profiles,
        auto_apn: None,
    })
}

/// The child elements of the `<Profile>` whose `Index` is `index`, in order, as (tag, decoded
/// text); `None` when there is no such profile. For an edit, which sends back every tag it
/// doesn't change. The result holds the saved password: it is used inside one driver call and
/// never logged. A profile with nested elements or an odd tag name can't be sent back
/// faithfully, so it is a parse error.
pub fn parse_profile_tags(
    body: &str,
    index: u32,
) -> Result<Option<Vec<(String, String)>>, RouterError> {
    let mut reader = Reader::from_str(body);
    let mut depth = 0usize;
    let mut profile_depth: Option<usize> = None;
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut text: Option<String> = None;
    let mut root_ok = None;
    loop {
        let ev = reader.read_event().map_err(parse_err)?;
        match &ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if root_ok.is_none() {
                    root_ok = Some(name == "response");
                }
                let empty = matches!(ev, Event::Empty(_));
                match profile_depth {
                    Some(pd) if depth == pd + 1 => {
                        return Err(parse_err("dialup profiles: nested profile element"));
                    }
                    Some(pd) if depth == pd => {
                        if !plain_tag(&name) {
                            return Err(parse_err("dialup profiles: unexpected tag name"));
                        }
                        tags.push((name, String::new()));
                        if !empty {
                            text = Some(String::new());
                        }
                    }
                    None if name == "Profile" && !empty => {
                        profile_depth = Some(depth + 1);
                        tags.clear();
                    }
                    _ => {}
                }
                if !empty {
                    depth += 1;
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => {
                if let (Some(buf), Some(t)) = (text.as_mut(), event_text(&ev)?) {
                    buf.push_str(&t);
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                match profile_depth {
                    Some(pd) if depth == pd => {
                        if let (Some(t), Some(last)) = (text.take(), tags.last_mut()) {
                            last.1 = t;
                        }
                    }
                    Some(pd) if depth + 1 == pd => {
                        profile_depth = None;
                        let found = tags
                            .iter()
                            .any(|(k, v)| k == "Index" && v.trim().parse() == Ok(index));
                        if found {
                            return Ok(Some(std::mem::take(&mut tags)));
                        }
                        tags.clear();
                    }
                    _ => {}
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if root_ok != Some(true) {
        return Err(parse_err("dialup profiles: unexpected response"));
    }
    Ok(None)
}

/// A tag the app may write back: ASCII letters, digits and underscores, starting with a letter.
fn plain_tag(name: &str) -> bool {
    name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `/api/dialup/auto-apn`: `<AutoAPN>` 1 is on, 0 is off.
pub fn parse_auto_apn(body: &str) -> Result<bool, RouterError> {
    let (_, m) = flat(body, &["AutoAPN"])?;
    flag(get(&m, "AutoAPN")).ok_or_else(|| parse_err("auto-apn: missing AutoAPN"))
}

/// The mobile data switch: `<dataswitch>` 1 is on, 0 is off. Anything else is a parse error.
pub fn parse_dataswitch(body: &str) -> Result<bool, RouterError> {
    let (_, m) = flat(body, &["dataswitch"])?;
    match get(&m, "dataswitch").trim() {
        "1" => Ok(true),
        "0" => Ok(false),
        other => Err(parse_err(format!("dataswitch: unexpected value {other:?}"))),
    }
}

/// One element below the root, in document order.
struct Node {
    name: String,
    /// 1 for the root's own children.
    depth: usize,
    /// The trimmed text of an element without child elements; `None` for one that has them.
    text: Option<String>,
}

/// The root's name and every element below it, for an answer whose element names aren't known
/// in advance.
fn walk(body: &str) -> Result<(String, Vec<Node>), RouterError> {
    let mut reader = Reader::from_str(body);
    let mut root: Option<String> = None;
    let mut nodes: Vec<Node> = Vec::new();
    // The open elements below the root, as indexes into `nodes`.
    let mut open: Vec<usize> = Vec::new();
    loop {
        let ev = reader.read_event().map_err(parse_err)?;
        match &ev {
            Event::Start(e) | Event::Empty(e) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if root.is_none() {
                    root = Some(name);
                    continue;
                }
                if let Some(&parent) = open.last() {
                    nodes[parent].text = None;
                }
                nodes.push(Node {
                    name,
                    depth: open.len() + 1,
                    text: Some(String::new()),
                });
                if matches!(ev, Event::Start(_)) {
                    open.push(nodes.len() - 1);
                }
            }
            Event::Text(_) | Event::GeneralRef(_) => {
                if let (Some(text), Some(&at)) = (event_text(&ev)?, open.last()) {
                    if let Some(buf) = nodes[at].text.as_mut() {
                        buf.push_str(&text);
                    }
                }
            }
            Event::End(_) => {
                open.pop();
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let root = root.ok_or_else(|| parse_err("the answer is empty"))?;
    for node in &mut nodes {
        if let Some(text) = node.text.as_mut() {
            *text = text.trim().to_string();
        }
    }
    Ok((root, nodes))
}

/// The element names of an answer, each once and in order, for the event log: plain names only,
/// `MAX_TAGS` at most. Never a value. The names are still the router's choice, so one shaped
/// like an address or a long number is left out: the diagnostics export reads the event log, and
/// its leak scan would refuse to run on it.
fn tag_names(nodes: &[Node]) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for node in nodes {
        if names.len() == crate::privacy::MAX_TAGS {
            break;
        }
        if plain_tag(&node.name)
            && node.name.len() <= 40
            && !names.contains(&node.name)
            && crate::diagnostics::identifier_hits(&node.name).is_empty()
        {
            names.push(node.name.clone());
        }
    }
    names
}

/// The element that says whether remote management is switched on (1) or off (0). Seen in the
/// H122's own answer on 2026-11-13; matched as a whole name, never as part of a longer one.
const CWMP_SWITCH: &str = "enable";

/// How a device is connected, from the router's `InterfaceType` word. The H122's words aren't
/// known yet, so a device that is on a Wi-Fi network (`AssociatedSsid`) is on Wi-Fi unless the
/// word says cable; an unknown word alone says nothing.
fn device_link(interface: &str, ssid: &str) -> Option<crate::privacy::DeviceLink> {
    use crate::privacy::DeviceLink;
    let word = interface.to_ascii_lowercase();
    let has = |parts: &[&str]| parts.iter().any(|p| word.contains(p));
    if has(&["wireless", "wifi", "wi-fi", "wlan", "802.11"]) {
        Some(DeviceLink::Wifi)
    } else if has(&["ethernet", "lan", "wired", "cable"]) {
        Some(DeviceLink::Cable)
    } else if !ssid.is_empty() {
        Some(DeviceLink::Wifi)
    } else {
        None
    }
}

/// A field that is never the management server's address, whatever it holds: the
/// connection-request address (the router's own), a STUN server or an account field.
fn never_server_field(name: &str) -> bool {
    ["req", "stun", "user", "pass", "pwd"]
        .iter()
        .any(|other| name.contains(other))
}

/// `/api/cwmp/basic-info`: the provider's remote management (TR-069). The path is
/// huawei-lte-api's `cwmp.basic_info`; the H122's element names aren't confirmed, so only a
/// server address is looked for. A field counts when its name says it is an address (`url`), or
/// when its value is an `http://` or `https://` address: a switch, a port or a mode named for
/// the ACS never reads as "a server is set". A field named for the ACS wins over any other.
/// Account names, passwords and the connection-request address are never kept, and whether the
/// feature is switched on isn't guessed.
pub fn parse_cwmp(
    body: &str,
) -> Result<crate::privacy::Seen<crate::privacy::RemoteManagement>, RouterError> {
    let (root, nodes) = walk(body)?;
    if root != "response" {
        return Err(parse_err("remote management: not a response"));
    }
    // (named for the ACS, the field's text, the host when the text is an address)
    let mut fields: Vec<(bool, &str, Option<String>)> = Vec::new();
    for node in &nodes {
        let (Some(text), name) = (node.text.as_deref(), node.name.to_ascii_lowercase()) else {
            continue;
        };
        if never_server_field(&name) {
            continue;
        }
        let host = crate::privacy::server_host(text);
        if name.contains("url") || host.is_some() {
            fields.push((name.contains("acs"), text, host));
        }
    }
    if fields.iter().any(|(acs, _, _)| *acs) {
        fields.retain(|(acs, _, _)| *acs);
    }
    let (server_host, server_set) = match fields.iter().find(|(_, text, _)| !text.is_empty()) {
        Some((_, _, host)) => (host.clone(), Some(true)),
        None if fields.is_empty() => (None, None),
        None => (None, Some(false)),
    };
    Ok(crate::privacy::Seen {
        value: crate::privacy::RemoteManagement {
            server_host,
            server_set,
            enabled: nodes
                .iter()
                .find(|n| n.name.eq_ignore_ascii_case(CWMP_SWITCH))
                .and_then(|n| n.text.as_deref())
                .and_then(flag),
        },
        tags: tag_names(&nodes),
    })
}

/// `/api/lan/HostInfo` or `/api/wlan/host-list`: the devices the router lists, one per `Host`.
/// The element names are the ones in the H122's own `HostInfo` answer (2026-11-13), compared
/// without case. A host without a readable `MacAddress` is skipped; `Active` is only in
/// `HostInfo`, so without it online stays unknown.
pub fn parse_hosts(
    body: &str,
) -> Result<crate::privacy::Seen<Vec<crate::privacy::LanDevice>>, RouterError> {
    use crate::privacy::{clean_name, is_private_mac, normalize_mac, LanDevice, MAX_DEVICES};
    let (root, nodes) = walk(body)?;
    if root != "response" {
        return Err(parse_err("devices: not a response"));
    }
    let mut devices: Vec<LanDevice> = Vec::new();
    let mut at = 0;
    while at < nodes.len() && devices.len() < MAX_DEVICES {
        let host = &nodes[at];
        at += 1;
        if !host.name.eq_ignore_ascii_case("host") {
            continue;
        }
        let mut fields: HashMap<String, &str> = HashMap::new();
        while at < nodes.len() && nodes[at].depth > host.depth {
            if let Some(text) = nodes[at].text.as_deref() {
                fields
                    .entry(nodes[at].name.to_ascii_lowercase())
                    .or_insert(text);
            }
            at += 1;
        }
        let field = |name: &str| fields.get(name).copied().unwrap_or("");
        let Some(mac) = normalize_mac(field("macaddress")) else {
            continue;
        };
        let ip = field("ipaddress")
            .split([';', ',', ' '])
            .find_map(|part| part.trim().parse::<std::net::Ipv4Addr>().ok())
            .filter(|ip| !ip.is_unspecified())
            .map(|ip| ip.to_string());
        devices.push(LanDevice {
            name: clean_name(field("actualname")).or_else(|| clean_name(field("hostname"))),
            private_mac: is_private_mac(&mac),
            mac,
            ip,
            active: flag(field("active")),
            link: device_link(field("interfacetype"), field("associatedssid")),
            this_device: flag(field("islocaldevice")) == Some(true),
        });
    }
    Ok(crate::privacy::Seen {
        value: devices,
        tags: tag_names(&nodes),
    })
}

/// `/api/net/plmn-list`: how many `<Network>` entries the search listed. Nothing else is read,
/// so the operators' names and codes never leave this function. An answer that isn't XML gives
/// `None`, not an error: the search ran either way.
pub fn count_plmn_networks(body: &str) -> Option<u8> {
    let mut reader = Reader::from_str(body);
    let mut count: u32 = 0;
    let mut root = false;
    loop {
        match reader.read_event().ok()? {
            Event::Start(e) | Event::Empty(e) => {
                let name = e.name();
                if !root {
                    root = true;
                    if name.as_ref() != b"response" {
                        return None;
                    }
                } else if name.as_ref() == b"Network" {
                    count += 1;
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    root.then(|| u8::try_from(count).unwrap_or(u8::MAX))
}
