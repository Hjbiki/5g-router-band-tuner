use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Signal quality bucket, shown as text as well as colour in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum Quality {
    Excellent,
    Good,
    Fair,
    Poor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Rsrp,
    Rsrq,
    Sinr,
}

/// Classify a measurement. Thresholds are inclusive lower bounds (spec section 4).
pub fn classify(metric: Metric, value: f32) -> Quality {
    let [excellent, good, fair] = match metric {
        Metric::Rsrp => [-80.0, -90.0, -100.0],
        Metric::Rsrq => [-10.0, -15.0, -20.0],
        Metric::Sinr => [20.0, 13.0, 0.0],
    };
    if value >= excellent {
        Quality::Excellent
    } else if value >= good {
        Quality::Good
    } else if value >= fair {
        Quality::Fair
    } else {
        Quality::Poor
    }
}
