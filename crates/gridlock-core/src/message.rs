//! MQTT-shaped messages and the topic contract.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Qos {
    AtMostOnce = 0,
    AtLeastOnce = 1,
    ExactlyOnce = 2,
}

/// One publish. `payload` is UTF-8 JSON for every engine topic.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: Qos,
    pub retain: bool,
}

impl Message {
    pub fn json(
        topic: impl Into<String>,
        value: &serde_json::Value,
        qos: Qos,
        retain: bool,
    ) -> Self {
        Self {
            topic: topic.into(),
            payload: value.to_string().into_bytes(),
            qos,
            retain,
        }
    }

    pub fn payload_str(&self) -> &str {
        std::str::from_utf8(&self.payload).unwrap_or("")
    }
}

/// Topic names, without the configurable root prefix.
pub mod topics {
    pub const GAME_PHASE: &str = "game/phase";
    pub const GAME_PLAN: &str = "game/plan";
    pub const GAME_ACTION: &str = "game/action";
    pub const GAME_BOOST: &str = "game/boost";
    pub const SIM_TICK: &str = "sim/tick";
    pub const FORECAST_DEMAND: &str = "forecast/demand";
    pub const FORECAST_PV: &str = "forecast/pv";
    pub const FORECAST_WIND: &str = "forecast/wind";
    pub const FORECAST_WINDOW: &str = "forecast/actual_window";
    pub const GRID_DEMAND: &str = "grid/demand";
    pub const GRID_BALANCE: &str = "grid/balance";
    pub const GRID_EVENT: &str = "grid/event";
    pub const SCORE_LIVE: &str = "score/live";
    pub const SCORE_ROUND: &str = "score/round";
    pub const SECURITY_STATUS: &str = "security/status";
    pub const SECURITY_ALERT: &str = "security/alert";
    pub const SECURITY_ATTACK: &str = "security/attack";

    pub fn plant_state(id: &str) -> String {
        format!("plant/{id}/state")
    }
    pub fn plant_cmd(id: &str) -> String {
        format!("plant/{id}/cmd")
    }
    pub fn debrief(n: usize) -> String {
        format!("debrief/{n}")
    }
}
