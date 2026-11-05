//! The bus layer: a [`Bus`] trait hiding whether messages go to memory, a live
//! broker or JavaScript, the reference [`MemoryBus`], and a [`Session`] that
//! pumps an engine's messages onto a bus and its commands back in.

use gridlock_core::engine::Refusal;
use gridlock_core::message::topics;
pub use gridlock_core::message::{Message, Qos};
use gridlock_core::{Action, Engine, EngineError, LoadError, Plan};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[cfg(feature = "mqtt")]
pub mod mqtt;

pub trait Bus {
    fn publish(&mut self, topic: &str, payload: &[u8], qos: Qos, retain: bool);
    fn subscribe(&mut self, filter: &str);
    /// Drain inbound messages; never blocks.
    fn poll(&mut self) -> Vec<Message>;
    /// Last will and testament. No-op on [`MemoryBus`].
    fn set_will(&mut self, topic: &str, payload: &[u8]);
}

/// MQTT topic-filter matching with `+` (one level) and `#` (the rest, including the parent).
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let mut f = filter.split('/');
    let mut t = topic.split('/');
    loop {
        match (f.next(), t.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(a), Some(b)) if a == b => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// In-memory reference implementation of the topic contract: a loopback client
/// that keeps retained messages and delivers matching publishes to its own
/// subscriptions in publish order.
#[derive(Debug, Default)]
pub struct MemoryBus {
    retained: BTreeMap<String, Message>,
    subs: Vec<String>,
    inbox: VecDeque<Message>,
    wills: BTreeMap<String, Vec<u8>>,
    published: usize,
}

impl MemoryBus {
    pub fn new() -> Self {
        Self::default()
    }

    /// The retained tree, as a late subscriber would see it.
    pub fn retained(&self) -> &BTreeMap<String, Message> {
        &self.retained
    }

    pub fn retained_json(&self, topic: &str) -> Option<serde_json::Value> {
        self.retained
            .get(topic)
            .and_then(|m| serde_json::from_slice(&m.payload).ok())
    }

    pub fn wills(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.wills
    }

    pub fn published_count(&self) -> usize {
        self.published
    }
}

impl Bus for MemoryBus {
    fn publish(&mut self, topic: &str, payload: &[u8], qos: Qos, retain: bool) {
        self.published += 1;
        let m = Message {
            topic: topic.to_string(),
            payload: payload.to_vec(),
            qos,
            retain,
        };
        if retain {
            if payload.is_empty() {
                self.retained.remove(topic);
            } else {
                self.retained.insert(topic.to_string(), m.clone());
            }
        }
        if self.subs.iter().any(|f| topic_matches(f, topic)) {
            self.inbox.push_back(m);
        }
    }

    fn subscribe(&mut self, filter: &str) {
        if !self.subs.iter().any(|f| f == filter) {
            self.subs.push(filter.to_string());
        }
        for (topic, m) in &self.retained {
            if topic_matches(filter, topic) {
                self.inbox.push_back(m.clone());
            }
        }
    }

    fn poll(&mut self) -> Vec<Message> {
        self.inbox.drain(..).collect()
    }

    fn set_will(&mut self, topic: &str, payload: &[u8]) {
        self.wills.insert(topic.to_string(), payload.to_vec());
    }
}

/// Parse an inbound command message (topic without prefix).
pub fn parse_command(topic: &str, payload: &[u8]) -> Option<Action> {
    let v: serde_json::Value = serde_json::from_slice(payload).unwrap_or(serde_json::Value::Null);
    match topic {
        topics::GAME_ACTION => {
            let action = v
                .get("action")
                .and_then(|a| a.as_str())
                .unwrap_or("malformed");
            Some(Action::from_parts(
                action,
                v.get("arg").and_then(|a| a.as_str()),
            ))
        }
        topics::GAME_BOOST => match v.get("arm").and_then(|a| a.as_bool()) {
            Some(true) => Some(Action::Boost),
            _ => Some(Action::Unknown("malformed".into())),
        },
        _ => None,
    }
}

/// An engine wired to a bus. The engine never sees the bus; the session moves
/// messages between them after each tick.
pub struct Session<B: Bus> {
    engine: Engine,
    bus: B,
    prefix: String,
    retained_topics: BTreeSet<String>,
}

impl<B: Bus> Session<B> {
    pub fn new(engine: Engine, bus: B, prefix: &str) -> Self {
        let mut s = Self {
            engine,
            bus,
            prefix: prefix.to_string(),
            retained_topics: BTreeSet::new(),
        };
        s.bus
            .subscribe(&format!("{}{}", s.prefix, topics::GAME_ACTION));
        s.bus
            .subscribe(&format!("{}{}", s.prefix, topics::GAME_BOOST));
        s.flush();
        s
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }
    pub fn bus(&self) -> &B {
        &self.bus
    }
    pub fn bus_mut(&mut self) -> &mut B {
        &mut self.bus
    }

    /// Start a new level: clear every retained topic this session published, then
    /// swap in the new engine.
    pub fn new_level(&mut self, engine: Engine) {
        for t in std::mem::take(&mut self.retained_topics) {
            self.bus.publish(&t, &[], Qos::AtLeastOnce, true);
        }
        self.engine = engine;
        self.flush();
    }

    fn flush(&mut self) {
        for m in self.engine.drain_outbox() {
            let topic = format!("{}{}", self.prefix, m.topic);
            if m.retain {
                self.retained_topics.insert(topic.clone());
            }
            self.bus.publish(&topic, &m.payload, m.qos, m.retain);
        }
    }

    pub fn submit_plan(&mut self, plan: &Plan, name: &str) -> Result<(), LoadError> {
        self.engine.submit_plan(plan, name)?;
        if let Some(vp) = self.engine.plan() {
            let ids: Vec<String> = vp.assets.iter().map(|a| a.id.clone()).collect();
            for id in ids {
                let topic = format!("{}{}", self.prefix, topics::plant_state(&id));
                self.bus.set_will(&topic, br#"{"status":"offline"}"#);
            }
        }
        self.flush();
        Ok(())
    }

    /// Play one tick and publish its state.
    pub fn step(&mut self) -> Result<u32, EngineError> {
        let t = self.engine.step()?;
        self.flush();
        Ok(t)
    }

    /// Publish a command the way an external client would.
    pub fn send(&mut self, action: &Action) {
        let topic = format!("{}{}", self.prefix, topics::GAME_ACTION);
        self.bus.publish(
            &topic,
            action.to_json().to_string().as_bytes(),
            Qos::AtLeastOnce,
            false,
        );
    }

    /// Poll the bus for commands and apply them. Returns each with its outcome.
    pub fn pump(&mut self) -> Vec<(Action, Result<(), Refusal>)> {
        let mut out = Vec::new();
        for m in self.bus.poll() {
            let Some(topic) = m.topic.strip_prefix(&self.prefix) else {
                continue;
            };
            if let Some(a) = parse_command(topic, &m.payload) {
                let r = self.engine.act(&a);
                out.push((a, r));
            }
        }
        self.flush();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plus_and_hash_matching() {
        assert!(topic_matches("plant/+/state", "plant/bat-1/state"));
        assert!(!topic_matches("plant/+/state", "plant/bat-1/cmd"));
        assert!(!topic_matches("plant/+", "plant/bat-1/state"));
        assert!(topic_matches("plant/#", "plant/bat-1/state"));
        assert!(topic_matches("plant/#", "plant"));
        assert!(topic_matches("#", "anything/at/all"));
        assert!(topic_matches("a/b", "a/b"));
        assert!(!topic_matches("a/b", "a/b/c"));
        assert!(!topic_matches("a/b/c", "a/b"));
    }

    #[test]
    fn retained_delivered_to_late_subscriber() {
        let mut b = MemoryBus::new();
        b.publish("score/live", b"{\"stars\":3}", Qos::AtMostOnce, true);
        b.publish("grid/demand", b"{}", Qos::AtMostOnce, false);
        assert!(b.poll().is_empty());
        b.subscribe("score/#");
        let got = b.poll();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].topic, "score/live");
        b.subscribe("grid/+");
        assert!(
            b.poll().is_empty(),
            "non-retained messages are not replayed"
        );
    }

    #[test]
    fn empty_retained_publish_clears() {
        let mut b = MemoryBus::new();
        b.publish("a/b", b"1", Qos::AtLeastOnce, true);
        assert!(b.retained().contains_key("a/b"));
        b.publish("a/b", b"", Qos::AtLeastOnce, true);
        assert!(!b.retained().contains_key("a/b"));
    }

    #[test]
    fn poll_returns_in_publish_order() {
        let mut b = MemoryBus::new();
        b.subscribe("x/#");
        for i in 0..5u8 {
            b.publish(&format!("x/{i}"), &[i], Qos::AtMostOnce, false);
        }
        let got: Vec<u8> = b.poll().into_iter().map(|m| m.payload[0]).collect();
        assert_eq!(got, vec![0, 1, 2, 3, 4]);
        assert!(b.poll().is_empty());
    }

    #[test]
    fn parses_commands() {
        assert_eq!(
            parse_command("game/action", br#"{"action":"boost"}"#),
            Some(Action::Boost)
        );
        assert_eq!(
            parse_command("game/action", br#"{"action":"upgrade","arg":"mtls"}"#),
            Some(Action::Upgrade("mtls".into()))
        );
        assert_eq!(
            parse_command("game/boost", br#"{"arm":true}"#),
            Some(Action::Boost)
        );
        assert_eq!(
            parse_command("game/action", b"nope"),
            Some(Action::Unknown("malformed".into()))
        );
        assert_eq!(parse_command("grid/demand", b"{}"), None);
    }
}
