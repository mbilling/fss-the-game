//! Gridlock simulation core.
//!
//! A pure, deterministic Power-to-X simulation: no IO, no clock, no RNG. Scenarios
//! and curves arrive as `&str`; the engine emits MQTT-shaped [`message::Message`]s
//! that a shell (CLI, WASM) hands to a bus.

pub mod debrief;
pub mod engine;
pub mod error;
pub mod message;
pub mod plan;
pub mod scenario;

pub use engine::{Action, Engine, EngineError, Phase, Refusal, RunOutput};
pub use error::LoadError;
pub use plan::{InteractionLog, Plan, ValidPlan};
pub use scenario::{Scenario, ScenarioFile};

/// Format a tick index as a wall-clock time `HH:MM`.
pub fn clock(t: u32, tick_minutes: u32) -> String {
    let m = t * tick_minutes;
    format!("{:02}:{:02}", m / 60, m % 60)
}
