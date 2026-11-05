mod common;
use common::*;
use gridlock_core::{Action, Phase, Refusal};

fn pv_plan(cap: f64) -> gridlock_core::Plan {
    plan(&PV_ONLY.replace("capacity_kw = 100", &format!("capacity_kw = {cap}")))
}

#[test]
fn outage_tolerance_boundary() {
    // demand 100, tolerance 2%: 2 kW short is fine, 2.1 kW short is an outage.
    let sc = synth("", &flat(100.0), &flat(1.0), &flat(0.0)).unwrap();
    let mut e = start(sc.clone(), &pv_plan(98.0));
    e.step().unwrap();
    assert!(!e.records()[0].outage);
    assert_eq!(e.stars(), 3);

    let mut e = start(sc, &pv_plan(97.9));
    e.step().unwrap();
    assert!(e.records()[0].outage);
    assert_eq!(e.stars(), 2);
}

#[test]
fn zero_demand_is_never_an_outage() {
    let sc = synth("", &flat(0.0), &flat(0.0), &flat(0.0)).unwrap();
    let mut e = start(sc, &pv_plan(1.0));
    play_out(&mut e);
    assert_eq!(e.stars(), 3);
}

#[test]
fn zero_stars_stops_play_at_once() {
    let sc = synth("", &flat(100.0), &flat(0.0), &flat(0.0)).unwrap();
    let mut e = start(sc, &pv_plan(10.0));
    play_out(&mut e);
    assert_eq!(e.phase(), Phase::Debrief);
    assert_eq!(e.records().len(), 3, "the third outage tick ends the day");
    let s = e.score().unwrap();
    assert!(s.ended_early && !s.pass);
    assert_eq!(s.reasons[0], "stars");
    assert_eq!(
        e.step().unwrap_err().to_string(),
        "engine is in phase debrief, expected play"
    );
}

const ELY: &str = r#"
[[assets]]
id = "pv-1"
catalog = "pv"
capacity_kw = 100

[[assets]]
id = "ely-1"
catalog = "electrolyser"
max_kw = 100
tank_kg = 100

[dispatch]
charge_priority = ["ely-1"]
"#;

#[test]
fn contract_missed_at_by_tick_is_not_recovered() {
    // No surplus until tick 6; the contract is due at tick 5.
    let mut pv = flat(0.5);
    for x in pv.iter_mut().skip(6) {
        *x = 1.0;
    }
    let sc = synth(
        "[[contracts]]\nproduct = \"h2\"\nkg = 1\nby_tick = 5\n",
        &flat(50.0),
        &pv,
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc, &plan(ELY));
    play_out(&mut e);
    let s = e.score().unwrap();
    assert_eq!(s.contracts_met, 0);
    assert!(s
        .star_losses
        .iter()
        .any(|l| l.t == 5 && l.cause.contains("contract")));
    assert!(s.reasons.contains(&"contract".to_string()));
    assert!(
        e.records().last().unwrap().assets[1].tank_kg.unwrap() > 1.0,
        "later H2 does not count"
    );
}

#[test]
fn contract_delivered_takes_h2_from_the_tank() {
    // 50 kW surplus into a 100% electrolyser at 10 kWh/kg: 5 kg per hour.
    let sc = synth(
        "[[contracts]]\nproduct = \"h2\"\nkg = 9\nby_tick = 1\n",
        &flat(50.0),
        &flat(1.0),
        &flat(0.0),
    )
    .unwrap();
    let mut e = start(sc, &plan(ELY));
    e.step().unwrap();
    e.step().unwrap();
    assert!((e.records()[1].assets[1].tank_kg.unwrap() - 1.0).abs() < 1e-9);
    play_out(&mut e);
    assert_eq!(e.score().unwrap().contracts_met, 1);
}

#[test]
fn boost_refused_without_charge() {
    let sc = synth("", &flat(150.0), &flat(1.0), &flat(0.0)).unwrap();
    let mut e = start(sc, &pv_plan(100.0));
    assert_eq!(
        e.act(&Action::Boost),
        Err(Refusal::Unknown),
        "nothing to arm before the first tick"
    );
    e.step().unwrap();
    assert_eq!(e.act(&Action::Boost), Ok(()));
    assert_eq!(
        e.act(&Action::Boost),
        Err(Refusal::Unknown),
        "already armed"
    );
    e.step().unwrap();
    assert_eq!(
        e.records()[1].boost_kw,
        50.0,
        "boost covers only the deficit"
    );
    assert_eq!(e.boost_charges(), 0);
    assert_eq!(e.act(&Action::Boost), Err(Refusal::NoCharge));
    let log = e.interactions();
    assert_eq!(log.len(), 4);
    assert_eq!(log[3].reason, Some(Refusal::NoCharge));
}

#[test]
fn boost_credit_earned_at_credit_kwh() {
    let sc = synth_r(
        &[("credit_kwh = 0", "credit_kwh = 100")],
        "",
        &flat(100.0),
        &flat(1.0),
        &flat(0.0),
    )
    .unwrap();
    let p = plan(
        r#"
[[assets]]
id = "pv-1"
catalog = "pv"
capacity_kw = 199.99

[[assets]]
id = "ic-1"
catalog = "interconnector"
export_kw = 500
import_kw = 0

[dispatch]
charge_priority = ["ic-1"]
"#,
    );
    let mut e = start(sc, &p);
    e.step().unwrap();
    assert_eq!(e.boost_charges(), 1, "99.99 kWh exported: not yet");
    e.step().unwrap();
    assert_eq!(e.boost_charges(), 2, "199.98 kWh exported: one credit");
}

#[test]
fn efficiency_counts_export_and_losses_as_waste() {
    // 200 kW harvested, 100 demand, 100 exported: half wasted.
    let sc = synth("", &flat(100.0), &flat(1.0), &flat(0.0)).unwrap();
    let p = plan(
        r#"
[[assets]]
id = "pv-1"
catalog = "pv"
capacity_kw = 200

[[assets]]
id = "ic-1"
catalog = "interconnector"
export_kw = 500
import_kw = 0

[dispatch]
charge_priority = ["ic-1"]
"#,
    );
    let mut e = start(sc, &p);
    play_out(&mut e);
    assert!((e.score().unwrap().efficiency - 0.5).abs() < 1e-9);
}

#[test]
fn leftover_storage_is_credited_at_day_end() {
    // 50 kW surplus every hour into a lossless battery, 1200 kWh gained.
    let sc = synth("", &flat(50.0), &flat(1.0), &flat(0.0)).unwrap();
    let p = plan(
        r#"
[[assets]]
id = "pv-1"
catalog = "pv"
capacity_kw = 100

[[assets]]
id = "bat-1"
catalog = "battery"
capacity_kwh = 10000
max_kw = 100
reserve = 0.0

[dispatch]
charge_priority = ["bat-1"]
discharge_priority = ["bat-1"]
"#,
    );
    let mut e = start(sc, &p);
    play_out(&mut e);
    let s = e.score().unwrap();
    assert!((s.leftover_kwh - 1200.0).abs() < 1e-6);
    assert_eq!(s.efficiency, 1.0, "clamped at 100%");
}
