use std::net::Ipv4Addr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::bands::{lte::LteBandSet, nr::NrBandInfo};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum NetworkMode {
    Auto,
    LteOnly,
    NrOnly,
    Raw { value: String },
}

impl NetworkMode {
    pub fn code(&self) -> &str {
        match self {
            Self::Auto => "00",
            Self::LteOnly => "03",
            Self::NrOnly => "08",
            Self::Raw { value } => value,
        }
    }

    pub fn from_code(code: &str) -> Self {
        match code {
            "00" => Self::Auto,
            "03" => Self::LteOnly,
            "08" => Self::NrOnly,
            other => Self::Raw {
                value: other.to_string(),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum NrOption {
    Nsa,
    Sa,
    SaNsa,
}

impl NrOption {
    pub fn code(self) -> u8 {
        match self {
            Self::Nsa => 0,
            Self::Sa => 1,
            Self::SaNsa => 2,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Nsa),
            1 => Some(Self::Sa),
            2 => Some(Self::SaNsa),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NetConfig {
    pub network_mode: NetworkMode,
    pub lte_bands: LteBandSet,
    pub legacy_band_raw: String,
    pub nr_option: Option<NrOption>,
}

/// How a driver gets the router a new internet address (Refresh IP).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum IpMethod {
    /// Mobile data off and back on (`set_data_switch`), with the data-switch journal.
    DataSwitch,
    /// A manual network search (`network_search`): the router drops its data session while it
    /// lists the operators around it, then reconnects by itself. Nothing is switched off.
    NetworkSearch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Capabilities {
    pub supported_lte: LteBandSet,
    pub modes: Vec<NetworkMode>,
    pub nr_options: Vec<NrOption>,
    pub band_edit_allowed: bool,
    /// The driver has a factory-reset call confirmed for this model. While false the button stays
    /// disabled and the actor refuses without calling the driver.
    #[serde(default)]
    pub factory_reset_verified: bool,
    /// The driver can turn the mobile data switch off and on (Refresh IP). False until a driver
    /// confirms the switch exists on its model.
    #[serde(default)]
    pub data_switch: bool,
    /// The ways this driver can refresh the IP, in the order to try them. Empty hides Refresh
    /// IP. `data_switch` above stays the read of the switch (the "mobile data is off" notice).
    #[serde(default)]
    pub ip_methods: Vec<IpMethod>,
    /// What the driver can do with APN profiles.
    #[serde(default)]
    pub apn: crate::apn::ApnSupport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Rat {
    Gsm,
    Umts,
    Lte,
    Nr,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LteSignal {
    pub band: Option<u16>,
    pub earfcn_dl: Option<u32>,
    pub earfcn_ul: Option<u32>,
    pub pci: Option<u16>,
    #[ts(type = "number | null")]
    pub cell_id: Option<u64>,
    pub enb_id: Option<u32>,
    pub sector: Option<u8>,
    pub rsrp: Option<f32>,
    pub rsrq: Option<f32>,
    pub sinr: Option<f32>,
    pub rssi: Option<f32>,
    pub tx_power: Option<f32>,
    pub dl_bw_mhz: Option<f32>,
    pub ul_bw_mhz: Option<f32>,
    pub dl_mcs: [Option<u8>; 2],
    pub ul_mcs: Option<u8>,
    pub cqi: [Option<u8>; 2],
    /// Some means carrier aggregation is active.
    pub scc_pci: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NrSignal {
    pub arfcn_dl: Option<u32>,
    pub arfcn_ul: Option<u32>,
    pub band: Option<NrBandInfo>,
    pub pci: Option<u16>,
    pub rsrp: Option<f32>,
    pub rsrq: Option<f32>,
    pub sinr: Option<f32>,
    pub tx_power: Option<f32>,
    pub dl_bw_mhz: Option<f32>,
    pub ul_bw_mhz: Option<f32>,
    pub dl_mcs: [Option<u8>; 2],
    pub ul_mcs: Option<u8>,
    pub cqi: [Option<u8>; 2],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Signal {
    pub serving: Option<Rat>,
    pub lte: Option<LteSignal>,
    pub nr: Option<NrSignal>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum ConnState {
    Connecting,
    Connected,
    Disconnected,
    Failed,
    NoService,
    Other { code: u32 },
}

impl ConnState {
    pub fn from_code(code: u32) -> Self {
        match code {
            900 => Self::Connecting,
            901 => Self::Connected,
            902 | 903 => Self::Disconnected,
            904 => Self::Failed,
            905 => Self::NoService,
            // 906 is the router's "Connection Error" code: a failure, not no-service.
            906 => Self::Failed,
            other => Self::Other { code: other },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Operator {
    pub name: String,
    pub mcc_mnc: String,
    /// The name stored on the SIM, when the router reports one.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub spn: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Status {
    pub connection: ConnState,
    pub network_type: u32,
    pub operator: Option<Operator>,
    pub bars: Option<u8>,
    pub nr_bars: Option<u8>,
    /// `EndcStatus`: the 4G cell has 5G dual connectivity on. `None` if the router doesn't say.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub endc: Option<bool>,
    /// `endcRestrictedStatus`: the network restricts 5G on this connection.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub endc_restricted: Option<bool>,
}

/// Only these three fields are ever parsed from device information.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeviceInfo {
    pub model: String,
    pub firmware: String,
    pub web_ui: String,
    /// The SIM's own network: MCC and MNC, the first digits of its IMSI. The rest of the IMSI
    /// identifies a person, so it never gets past the parser.
    #[serde(default)]
    #[ts(optional = nullable)]
    pub home_plmn: Option<String>,
}

pub fn in_service(status: &Status, signal: &Signal) -> bool {
    status.connection == ConnState::Connected && signal.serving.is_some()
}

/// What kind of address the router got from the operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WanKind {
    /// 100.64.0.0/10: the operator's carrier-grade NAT.
    Cgnat,
    /// 10/8, 172.16/12 or 192.168/16: behind the operator's NAT too.
    Private,
    /// Anything else: reachable from the internet, unless a firewall says otherwise.
    Public,
}

/// The router's internet (WAN) address, for the live snapshot only.
///
/// `ipv4` is the full address and never leaves memory: everything stored or logged uses
/// `range`, the first two numbers ("a.b"). Nothing about IPv6 is kept but whether there is one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WanView {
    pub ipv4: Option<String>,
    pub range: Option<String>,
    pub kind: Option<WanKind>,
    pub ipv6: bool,
}

impl WanView {
    pub fn new(ipv4: Option<Ipv4Addr>, ipv6: bool) -> Self {
        Self {
            ipv4: ipv4.map(|ip| ip.to_string()),
            range: ipv4.map(ipv4_range),
            kind: ipv4.map(wan_kind),
            ipv6,
        }
    }
}

/// The address's first two numbers, "a.b": the only part of a WAN address that is ever stored.
pub fn ipv4_range(ip: Ipv4Addr) -> String {
    let [a, b, _, _] = ip.octets();
    format!("{a}.{b}")
}

pub fn wan_kind(ip: Ipv4Addr) -> WanKind {
    let [a, b, _, _] = ip.octets();
    match (a, b) {
        (100, 64..=127) => WanKind::Cgnat,
        (10, _) | (172, 16..=31) | (192, 168) => WanKind::Private,
        _ => WanKind::Public,
    }
}
