mod common;
use common::*;

#[test]
fn wrong_curve_row_count() {
    let e = synth("", &[1.0; 23], &flat(0.0), &flat(0.0)).unwrap_err();
    assert_eq!(e.file, "d.csv");
    assert!(e.msg.contains("23 data rows, expected 24"), "{e}");
}

#[test]
fn curve_value_out_of_range() {
    let mut pv = flat(0.5);
    pv[3] = 1.5;
    let e = synth("", &flat(1.0), &pv, &flat(0.0)).unwrap_err();
    assert_eq!((e.file.as_str(), e.key.as_str()), ("pv.csv", "row 5"));
    assert!(e.msg.contains("outside 0 to 1"));
}

#[test]
fn unknown_attack_vector() {
    let e = synth(
        "[[actual.events]]\nat = 1\nkind = \"attack\"\nvector = \"laser\"\nticks = 1\n",
        &flat(1.0),
        &flat(0.0),
        &flat(0.0),
    )
    .unwrap_err();
    assert_eq!(
        (e.file.as_str(), e.key.as_str()),
        ("synth.toml", "actual.events[0].vector")
    );
    assert!(e.msg.contains("laser"));
}

#[test]
fn event_past_the_end_of_the_day() {
    let e = synth(
        "[[actual.events]]\nat = 20\nkind = \"outage\"\nasset_catalog = \"pv\"\nticks = 5\n",
        &flat(1.0),
        &flat(0.0),
        &flat(0.0),
    )
    .unwrap_err();
    assert_eq!(e.key, "actual.events[0]");
}

#[test]
fn unknown_catalog_id_in_plan() {
    let sc = synth("", &flat(1.0), &flat(0.0), &flat(0.0)).unwrap();
    let e = plan("[[assets]]\nid = \"x-1\"\ncatalog = \"fusion\"\ncapacity_kw = 1\n[dispatch]\n")
        .validate(&sc, "p.toml")
        .unwrap_err();
    assert_eq!(
        (e.file.as_str(), e.key.as_str()),
        ("p.toml", "assets[0].catalog")
    );
    assert!(e.msg.contains("fusion"));
}

#[test]
fn plan_over_budget() {
    let sc = synth("", &flat(1.0), &flat(0.0), &flat(0.0)).unwrap();
    let p = plan(&format!("{PV_ONLY}\n[security]\nupgrades = [\"mtls\"]\n"));
    let sc_cheap = gridlock_core::Scenario {
        round: gridlock_core::scenario::RoundSpec {
            budget: 5.0,
            ..sc.round.clone()
        },
        ..sc
    };
    let e = p.validate(&sc_cheap, "p.toml").unwrap_err();
    assert_eq!(e.key, "plan");
    assert!(e.msg.contains("exceeds the budget"), "{e}");
}

#[test]
fn storage_must_be_in_both_priority_lists() {
    let sc = synth("", &flat(1.0), &flat(0.0), &flat(0.0)).unwrap();
    let p = plan(&PV_ONLY.replace(
        "[dispatch]",
        "[[assets]]\nid = \"bat-1\"\ncatalog = \"battery\"\ncapacity_kwh = 1\nmax_kw = 1\nreserve = 0\n[dispatch]\ncharge_priority = [\"bat-1\"]",
    ));
    let e = p.validate(&sc, "p.toml").unwrap_err();
    assert_eq!(e.key, "dispatch");
}

#[test]
fn reference_plans_cost_what_the_spec_says() {
    let sc = cloudy();
    let bal = plan_file("balanced.toml")
        .validate(&sc, "balanced.toml")
        .unwrap();
    let headroom = (sc.round.budget - bal.cost) / sc.round.budget;
    assert!(
        (0.05..=0.15).contains(&headroom),
        "balanced headroom {headroom}"
    );
    let uns = plan_file("unsecured.toml")
        .validate(&sc, "unsecured.toml")
        .unwrap();
    assert_eq!(
        uns.assets, bal.assets,
        "unsecured has the same assets as balanced"
    );
    assert!(uns.upgrades.is_empty());
}
