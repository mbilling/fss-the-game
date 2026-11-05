//! Player plans and interaction logs.

use crate::error::LoadError;
use crate::scenario::{CatalogKind, CurveId, ForecastTier, Scenario, Vector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub plan: PlanHeader,
    pub assets: Vec<AssetDecl>,
    pub dispatch: Dispatch,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security: Option<PlanSecurity>,
    /// Zones are M1; parsed and ignored with a warning.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zones: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanHeader {
    pub scenario: String,
    #[serde(default)]
    pub forecast_tier: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDecl {
    pub id: String,
    pub catalog: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_kw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capacity_kwh: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_kw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tank_kg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub export_kw: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub import_kw: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dispatch {
    #[serde(default)]
    pub charge_priority: Vec<String>,
    #[serde(default)]
    pub discharge_priority: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanSecurity {
    #[serde(default)]
    pub upgrades: Vec<String>,
}

/// A plan asset with its catalog parameters resolved.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetSpec {
    Source {
        curve: CurveId,
        capacity_kw: f64,
    },
    Storage {
        capacity_kwh: f64,
        max_kw: f64,
        reserve: f64,
        eta_in: f64,
        eta_out: f64,
        soc_init: f64,
    },
    Electrolyser {
        max_kw: f64,
        tank_kg: f64,
        eta: f64,
        kwh_per_kg: f64,
    },
    FuelCell {
        max_kw: f64,
        eta: f64,
        kwh_per_kg: f64,
    },
    Interconnector {
        export_kw: f64,
        import_kw: f64,
        eta: f64,
        co2_per_kwh_import: f64,
        export_credit_ratio: f64,
        credits_init: f64,
    },
}

impl AssetSpec {
    /// Kind name used in the WASM manifest.
    pub fn kind(&self) -> &'static str {
        match self {
            AssetSpec::Source { .. } => "source",
            AssetSpec::Storage { .. } => "storage",
            AssetSpec::Electrolyser { .. } => "electrolyser",
            AssetSpec::FuelCell { .. } => "fuel_cell",
            AssetSpec::Interconnector { .. } => "interconnector",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub id: String,
    pub catalog: String,
    pub spec: AssetSpec,
    pub cost: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CostLine {
    pub item: String,
    pub cost: f64,
}

/// A plan checked against a scenario, ready for Play.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidPlan {
    pub plan: Plan,
    pub assets: Vec<Asset>,
    pub charge: Vec<usize>,
    pub discharge: Vec<usize>,
    pub upgrades: Vec<String>,
    pub tier: ForecastTier,
    pub cost_lines: Vec<CostLine>,
    pub cost: f64,
    pub warnings: Vec<String>,
}

impl ValidPlan {
    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.assets.iter().position(|a| a.id == id)
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl Plan {
    pub fn parse(name: &str, toml_text: &str) -> Result<Self, LoadError> {
        toml::from_str(toml_text).map_err(|e| LoadError::new(name, "", e.message().to_string()))
    }

    /// Validate against a scenario and compute the cost. `name` labels errors.
    pub fn validate(&self, sc: &Scenario, name: &str) -> Result<ValidPlan, LoadError> {
        let err = |key: String, msg: String| LoadError::new(name, key, msg);
        let mut warnings = Vec::new();
        if self.plan.scenario != sc.round.id {
            return Err(err(
                "plan.scenario".into(),
                format!(
                    "plan is for '{}', scenario is '{}'",
                    self.plan.scenario, sc.round.id
                ),
            ));
        }
        let tier = sc.tier(self.plan.forecast_tier).cloned().ok_or_else(|| {
            err(
                "plan.forecast_tier".into(),
                format!("unknown tier {}", self.plan.forecast_tier),
            )
        })?;

        let mut ids = BTreeSet::new();
        let mut assets = Vec::new();
        let mut cost_lines = Vec::new();
        for (i, a) in self.assets.iter().enumerate() {
            let key = format!("assets[{i}]");
            if !valid_id(&a.id) {
                return Err(err(
                    format!("{key}.id"),
                    format!("'{}' must match [a-z0-9-]+", a.id),
                ));
            }
            if !ids.insert(a.id.clone()) {
                return Err(err(
                    format!("{key}.id"),
                    format!("duplicate asset id '{}'", a.id),
                ));
            }
            let entry = sc.catalog_entry(&a.catalog).ok_or_else(|| {
                err(
                    format!("{key}.catalog"),
                    format!("unknown catalog id '{}'", a.catalog),
                )
            })?;
            let need = |v: Option<f64>, field: &str| -> Result<f64, LoadError> {
                let x = v.ok_or_else(|| {
                    err(
                        format!("{key}.{field}"),
                        format!("required for a {} asset", entry.id),
                    )
                })?;
                if !x.is_finite() || x < 0.0 {
                    return Err(err(
                        format!("{key}.{field}"),
                        format!("{x} must be a finite number >= 0"),
                    ));
                }
                Ok(x)
            };
            let mut used: Vec<&str> = Vec::new();
            let (spec, cost) = match &entry.kind {
                CatalogKind::Source {
                    curve,
                    cost_per_kw,
                    max_capacity_kw,
                } => {
                    used.push("capacity_kw");
                    let cap = need(a.capacity_kw, "capacity_kw")?;
                    if cap > *max_capacity_kw {
                        return Err(err(
                            format!("{key}.capacity_kw"),
                            format!("{cap} exceeds the catalog maximum {max_capacity_kw}"),
                        ));
                    }
                    (
                        AssetSpec::Source {
                            curve: *curve,
                            capacity_kw: cap,
                        },
                        cap * cost_per_kw,
                    )
                }
                CatalogKind::Storage {
                    eta_in,
                    eta_out,
                    soc_init,
                    cost_per_kwh,
                    cost_per_kw,
                } => {
                    used.extend(["capacity_kwh", "max_kw", "reserve"]);
                    let cap = need(a.capacity_kwh, "capacity_kwh")?;
                    let max_kw = need(a.max_kw, "max_kw")?;
                    let reserve = need(a.reserve, "reserve")?;
                    if reserve > 1.0 {
                        return Err(err(
                            format!("{key}.reserve"),
                            format!("{reserve} is outside 0 to 1"),
                        ));
                    }
                    (
                        AssetSpec::Storage {
                            capacity_kwh: cap,
                            max_kw,
                            reserve,
                            eta_in: *eta_in,
                            eta_out: *eta_out,
                            soc_init: *soc_init,
                        },
                        cap * cost_per_kwh + max_kw * cost_per_kw,
                    )
                }
                CatalogKind::Electrolyser {
                    eta,
                    kwh_per_kg,
                    cost_per_kw,
                    tank_cost_per_kg,
                } => {
                    used.extend(["max_kw", "tank_kg"]);
                    let max_kw = need(a.max_kw, "max_kw")?;
                    let tank = need(a.tank_kg, "tank_kg")?;
                    (
                        AssetSpec::Electrolyser {
                            max_kw,
                            tank_kg: tank,
                            eta: *eta,
                            kwh_per_kg: *kwh_per_kg,
                        },
                        max_kw * cost_per_kw + tank * tank_cost_per_kg,
                    )
                }
                CatalogKind::FuelCell {
                    eta,
                    kwh_per_kg,
                    cost_per_kw,
                } => {
                    used.push("max_kw");
                    let max_kw = need(a.max_kw, "max_kw")?;
                    (
                        AssetSpec::FuelCell {
                            max_kw,
                            eta: *eta,
                            kwh_per_kg: *kwh_per_kg,
                        },
                        max_kw * cost_per_kw,
                    )
                }
                CatalogKind::Interconnector {
                    eta,
                    co2_per_kwh_import,
                    export_credit_ratio,
                    cost_per_kw,
                    max_kw,
                    credits_init,
                } => {
                    used.extend(["export_kw", "import_kw"]);
                    let ex = need(a.export_kw, "export_kw")?;
                    let im = need(a.import_kw, "import_kw")?;
                    if ex > *max_kw || im > *max_kw {
                        return Err(err(
                            key.clone(),
                            format!(
                                "export_kw and import_kw are capped at {max_kw} by the catalog"
                            ),
                        ));
                    }
                    (
                        AssetSpec::Interconnector {
                            export_kw: ex,
                            import_kw: im,
                            eta: *eta,
                            co2_per_kwh_import: *co2_per_kwh_import,
                            export_credit_ratio: *export_credit_ratio,
                            credits_init: *credits_init,
                        },
                        ex.max(im) * cost_per_kw,
                    )
                }
                CatalogKind::Security(_) => {
                    return Err(err(
                        format!("{key}.catalog"),
                        format!(
                            "'{}' is a security upgrade; list it under [security] upgrades",
                            a.catalog
                        ),
                    ))
                }
            };
            let given = [
                ("capacity_kw", a.capacity_kw),
                ("capacity_kwh", a.capacity_kwh),
                ("max_kw", a.max_kw),
                ("reserve", a.reserve),
                ("tank_kg", a.tank_kg),
                ("export_kw", a.export_kw),
                ("import_kw", a.import_kw),
            ];
            for (field, v) in given {
                if v.is_some() && !used.contains(&field) {
                    warnings.push(format!(
                        "{name}: {key}.{field} is ignored for a {} asset",
                        entry.id
                    ));
                }
            }
            cost_lines.push(CostLine {
                item: a.id.clone(),
                cost,
            });
            assets.push(Asset {
                id: a.id.clone(),
                catalog: a.catalog.clone(),
                spec,
                cost,
            });
        }
        if !assets
            .iter()
            .any(|a| matches!(a.spec, AssetSpec::Source { .. }))
        {
            return Err(err(
                "assets".into(),
                "the plan needs at least one source".into(),
            ));
        }

        let resolve = |list: &[String],
                       field: &str,
                       ok: fn(&AssetSpec) -> bool|
         -> Result<Vec<usize>, LoadError> {
            let mut out = Vec::new();
            for (j, id) in list.iter().enumerate() {
                let key = format!("dispatch.{field}[{j}]");
                let i = assets
                    .iter()
                    .position(|a| &a.id == id)
                    .ok_or_else(|| err(key.clone(), format!("unknown asset id '{id}'")))?;
                if !ok(&assets[i].spec) {
                    return Err(err(
                        key,
                        format!(
                            "a {} asset cannot take part in {field}",
                            assets[i].spec.kind()
                        ),
                    ));
                }
                if out.contains(&i) {
                    return Err(err(key, format!("'{id}' is listed twice")));
                }
                out.push(i);
            }
            Ok(out)
        };
        let charge = resolve(&self.dispatch.charge_priority, "charge_priority", |s| {
            matches!(
                s,
                AssetSpec::Storage { .. }
                    | AssetSpec::Electrolyser { .. }
                    | AssetSpec::Interconnector { .. }
            )
        })?;
        let discharge = resolve(
            &self.dispatch.discharge_priority,
            "discharge_priority",
            |s| {
                matches!(
                    s,
                    AssetSpec::Storage { .. }
                        | AssetSpec::FuelCell { .. }
                        | AssetSpec::Interconnector { .. }
                )
            },
        )?;
        for (i, a) in assets.iter().enumerate() {
            if matches!(a.spec, AssetSpec::Storage { .. })
                && !(charge.contains(&i) && discharge.contains(&i))
            {
                return Err(err(
                    "dispatch".into(),
                    format!(
                        "storage '{}' must appear in both charge_priority and discharge_priority",
                        a.id
                    ),
                ));
            }
        }

        let mut upgrades = Vec::new();
        if let Some(sec) = &self.security {
            for (j, u) in sec.upgrades.iter().enumerate() {
                let key = format!("security.upgrades[{j}]");
                let up = sc
                    .upgrade(u)
                    .ok_or_else(|| err(key.clone(), format!("unknown security upgrade '{u}'")))?;
                if upgrades.contains(u) {
                    return Err(err(key, format!("'{u}' is listed twice")));
                }
                cost_lines.push(CostLine {
                    item: format!("upgrade {u}"),
                    cost: up.cost,
                });
                upgrades.push(u.clone());
            }
        }
        if !self.zones.is_empty() {
            warnings.push(format!(
                "{name}: zones are an M1 feature and are ignored in v0"
            ));
        }
        cost_lines.push(CostLine {
            item: format!("forecast tier {}", tier.tier),
            cost: tier.cost,
        });

        let cost: f64 = cost_lines.iter().map(|c| c.cost).sum();
        if cost > sc.round.budget + 1e-9 {
            return Err(err(
                "plan".into(),
                format!("cost {cost:.1} exceeds the budget of {}", sc.round.budget),
            ));
        }

        Ok(ValidPlan {
            plan: self.clone(),
            assets,
            charge,
            discharge,
            upgrades,
            tier,
            cost_lines,
            cost,
            warnings,
        })
    }
}

/// One exposure-list line per attack vector, for the Plan screen.
pub fn exposure(sc: &Scenario, upgrades: &[String]) -> Vec<(Vector, String)> {
    let owned = |pred: &dyn Fn(&crate::scenario::SecurityUpgrade) -> bool| -> Vec<&str> {
        upgrades
            .iter()
            .filter(|u| sc.upgrade(u).is_some_and(pred))
            .map(String::as_str)
            .collect()
    };
    Vector::ALL
        .into_iter()
        .map(|v| {
            let text = if !sc.has_attacks() {
                "not present".to_string()
            } else if let blocked @ [_, ..] = owned(&|s| s.defeats.contains(&v)).as_slice() {
                format!("covered by {}", blocked.join(", "))
            } else if let [first, ..] = owned(&|s| s.covers(v)).as_slice() {
                format!("shortened by {first}")
            } else {
                "open".to_string()
            };
            (v, text)
        })
        .collect()
}

/// What the player did during Play. Replayed with the plan, it reproduces a session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionLog {
    #[serde(default)]
    pub interactions: Vec<InteractionEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionEntry {
    /// Issued after tick `t` was played; applies from `t + 1`.
    pub t: u32,
    pub action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg: Option<String>,
}

impl InteractionLog {
    pub fn parse(name: &str, toml_text: &str) -> Result<Self, LoadError> {
        toml::from_str(toml_text).map_err(|e| LoadError::new(name, "", e.message().to_string()))
    }
}
