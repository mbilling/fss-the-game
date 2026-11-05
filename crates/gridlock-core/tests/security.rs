mod common;
use common::*;
use gridlock_core::engine::Status;
use gridlock_core::message::topics;
use gridlock_core::{Action, Engine, Refusal};

const BAT: &str = r#"
[[assets]]
id = "pv-1"
catalog = "pv"
capacity_kw = 100

[[assets]]
id = "bat-1"
catalog = "battery"
capacity_kwh = 1000
max_kw = 100
reserve = 0.1

[dispatch]
charge_priority = ["bat-1"]
discharge_priority = ["bat-1"]
"#;

fn attack(vector: &str, at: u32, ticks: u32) -> String {
    let target = if vector == "credential_theft" {
        "target_catalog = \"battery\"\n"
    } else {
        ""
    };
    format!("[[actual.events]]\nat = {at}\nkind = \"attack\"\nvector = \"{vector}\"\nticks = {ticks}\n{target}")
}

fn with_upgrades(ups: &[&str]) -> gridlock_core::Plan {
    let list: Vec<String> = ups.iter().map(|u| format!("\"{u}\"")).collect();
    plan(&format!(
        "{BAT}\n[security]\nupgrades = [{}]\n",
        list.join(", ")
    ))
}

fn run_attack(vector: &str, ups: &[&str]) -> Engine {
    let sc = synth(&attack(vector, 5, 3), &flat(50.0), &flat(0.5), &flat(0.0)).unwrap();
    let mut e = start(sc, &with_upgrades(ups));
    play_out(&mut e);
    e
}

#[test]
fn each_vector_blocked_by_each_upgrade_that_claims_it() {
    for (vector, up) in [
        ("credential_theft", "mtls"),
        ("rogue_client", "mtls"),
        ("rogue_client", "acl"),
    ] {
        let e = run_attack(vector, &[up]);
        let a = &e.attacks()[0];
        assert_eq!(a.blocked_by.as_deref(), Some(up), "{vector} vs {up}");
        assert!(!a.landed);
        assert!(e
            .records()
            .iter()
            .all(|r| r.assets.iter().all(|x| x.status == Status::Ok)));
    }
}

#[test]
fn undefended_attacks_land() {
    let e = run_attack("credential_theft", &["acl"]);
    assert!(e.attacks()[0].landed);
    assert_eq!(e.records()[5].assets[1].status, Status::Rogue);
    assert!(
        e.records()[5].curtailed_kw > 0.0,
        "a rogue battery dumps into curtailment"
    );
    assert_eq!(e.records()[8].assets[1].status, Status::Ok);

    let e = run_attack("rogue_client", &["ids"]);
    assert_eq!(e.records()[6].assets[1].status, Status::Idle);
    assert_eq!(
        e.records()[6].assets[0].status,
        Status::Ok,
        "sources keep producing"
    );
}

#[test]
fn cluster_shortens_broker_outage_and_skips_the_direct_star() {
    let sc = synth(
        &attack("broker_outage", 5, 6),
        &flat(50.0),
        &flat(0.5),
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc.clone(), &with_upgrades(&["cluster"]));
    play_out(&mut e);
    let a = &e.attacks()[0];
    assert_eq!(
        (a.ticks, a.mitigated_by.as_deref(), a.landed),
        (1, Some("cluster"), true)
    );
    assert_eq!(e.records()[5].assets[1].status, Status::Idle);
    assert_eq!(e.records()[6].assets[1].status, Status::Ok);
    assert_eq!(e.stars(), 3);

    let mut e = start(sc, &with_upgrades(&[]));
    play_out(&mut e);
    assert_eq!(e.attacks()[0].ticks, 6);
    assert_eq!(e.records()[10].assets[1].status, Status::Idle);
    assert_eq!(e.stars(), 2, "a full broker outage costs a star by itself");
}

#[test]
fn upgrade_in_play_activates_exactly_setup_ticks_later() {
    // mtls setup is 3 ticks: started after tick 1, active from tick 4.
    for (at, blocked) in [(3u32, false), (4u32, true)] {
        let sc = synth(
            &attack("credential_theft", at, 1),
            &flat(50.0),
            &flat(0.5),
            &flat(0.0),
        )
        .unwrap();
        let mut e = start(sc, &with_upgrades(&[]));
        e.step().unwrap();
        e.step().unwrap();
        assert_eq!(e.act(&Action::Upgrade("mtls".into())), Ok(()));
        assert!(!e.upgrade_active("mtls", 3));
        assert!(e.upgrade_active("mtls", 4));
        play_out(&mut e);
        assert_eq!(
            e.attacks()[0].blocked_by.is_some(),
            blocked,
            "attack at {at}"
        );
    }
}

#[test]
fn upgrade_refused_too_late_against_an_attack_in_progress() {
    let sc = synth(
        &attack("rogue_client", 2, 4),
        &flat(50.0),
        &flat(0.5),
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc, &with_upgrades(&[]));
    for _ in 0..3 {
        e.step().unwrap();
    }
    assert_eq!(e.act(&Action::Upgrade("acl".into())), Err(Refusal::TooLate));
    assert_eq!(
        e.act(&Action::Upgrade("mtls".into())),
        Err(Refusal::TooLate)
    );
    // The attack covers ticks 2..=5; after tick 5 it is over.
    for _ in 0..3 {
        e.step().unwrap();
    }
    assert_eq!(
        e.act(&Action::Upgrade("acl".into())),
        Err(Refusal::TooLate),
        "tick 5 is still under attack"
    );
    e.step().unwrap();
    assert_eq!(
        e.act(&Action::Upgrade("acl".into())),
        Ok(()),
        "still covers later attacks"
    );
}

#[test]
fn every_interaction_refused_while_busy() {
    let sc = synth("", &flat(50.0), &flat(0.5), &flat(0.0)).unwrap();
    let mut e = start(sc, &with_upgrades(&[]));
    e.step().unwrap();
    assert_eq!(e.act(&Action::Upgrade("mtls".into())), Ok(()));
    assert_eq!(e.busy_until(), 3);
    for _ in 0..2 {
        e.step().unwrap();
        assert_eq!(e.act(&Action::Boost), Err(Refusal::Busy));
        assert_eq!(e.act(&Action::Upgrade("acl".into())), Err(Refusal::Busy));
    }
    e.step().unwrap();
    assert_eq!(e.act(&Action::Boost), Ok(()));
}

#[test]
fn upgrade_refused_without_budget() {
    let sc = synth_r(
        &[("budget = 1000", "budget = 15")],
        "",
        &flat(50.0),
        &flat(0.5),
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc, &with_upgrades(&["acl"]));
    e.step().unwrap();
    assert_eq!(
        e.act(&Action::Upgrade("mtls".into())),
        Err(Refusal::NoBudget)
    );
    assert_eq!(
        e.act(&Action::Upgrade("acl".into())),
        Err(Refusal::Unknown),
        "already owned"
    );
    assert_eq!(
        e.act(&Action::Upgrade("nope".into())),
        Err(Refusal::Unknown)
    );
}

#[test]
fn broker_outage_refuses_interactions() {
    let sc = synth(
        &attack("broker_outage", 1, 3),
        &flat(50.0),
        &flat(0.5),
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc, &with_upgrades(&[]));
    e.step().unwrap();
    e.step().unwrap();
    assert_eq!(e.act(&Action::Boost), Err(Refusal::BrokerDown));
}

#[test]
fn ids_alerts_exactly_warn_ticks_before() {
    let sc = synth(
        &attack("credential_theft", 10, 2),
        &flat(50.0),
        &flat(0.5),
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc.clone(), &with_upgrades(&["ids"]));
    e.drain_outbox();
    let mut alert_ticks = Vec::new();
    while e.phase() == gridlock_core::Phase::Play {
        let t = e.step().unwrap();
        if e.drain_outbox()
            .iter()
            .any(|m| m.topic == topics::SECURITY_ALERT)
        {
            alert_ticks.push(t);
        }
    }
    assert_eq!(alert_ticks, vec![8]);
    assert_eq!(e.attacks()[0].alert_t, Some(8));

    let mut e = start(sc, &with_upgrades(&[]));
    play_out(&mut e);
    assert!(
        !e.drain_outbox()
            .iter()
            .any(|m| m.topic == topics::SECURITY_ALERT),
        "no ids, no alert"
    );
}
