//! Live broker transport. Stubbed in M0; implemented on rumqttc in M1.
//!
//! mqttd is secure by default: a local test broker needs
//! `MQTTD_PLAINTEXT_BIND=127.0.0.1:1883 MQTTD_ALLOW_ANONYMOUS=1`.
//! Connection settings come from `GRIDLOCK_MQTT_URL` (default
//! `mqtt://127.0.0.1:1883`), `GRIDLOCK_MQTT_USER` and `GRIDLOCK_MQTT_PASS`.

use std::fmt;

#[derive(Debug, Clone)]
pub struct MqttConfig {
    pub url: String,
    pub user: Option<String>,
    pub pass: Option<String>,
}

impl MqttConfig {
    pub fn from_env() -> Self {
        Self {
            url: std::env::var("GRIDLOCK_MQTT_URL")
                .unwrap_or_else(|_| "mqtt://127.0.0.1:1883".into()),
            user: std::env::var("GRIDLOCK_MQTT_USER").ok(),
            pass: std::env::var("GRIDLOCK_MQTT_PASS").ok(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BusError(pub String);

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for BusError {}

/// Placeholder for the rumqttc-backed bus.
pub struct MqttBus;

impl MqttBus {
    pub fn connect(cfg: &MqttConfig) -> Result<Self, BusError> {
        Err(BusError(format!(
            "MqttBus is not implemented until M1 (would connect to {})",
            cfg.url
        )))
    }
}
