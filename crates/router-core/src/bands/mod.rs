use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod lte;
pub mod nr;
pub mod quality;

/// Duplex scheme of a band, shared by LTE and NR.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Duplex {
    Fdd,
    Tdd,
    SdlOnly,
}
