#!/usr/bin/env python3
"""Regenerate the cloudy_tuesday curve CSVs.

Deterministic: smooth shapes plus fixed "weather" bumps, no randomness. The CSVs
in scenarios/cloudy_tuesday/curves are the source of truth; this script only
documents how they were made and makes re-tuning repeatable.

usage: python3 tools/gen_curves.py [out_dir]
"""
import math
import os
import sys

TICKS = 96  # 15-minute ticks


def hour(t):
    return t * 0.25 + 0.125  # middle of the tick


def bump(h, centre, width, height):
    return height * math.exp(-(((h - centre) / width) ** 2))


def demand(t, actual):
    h = hour(t)
    base = 420
    morning = bump(h, 8.0, 1.6, 260)
    day = bump(h, 13.0, 4.0, 160)
    evening = bump(h, 19.0, 1.9, 470 if actual else 400)
    if actual:
        # A colder-than-forecast evening and a busy lunch hour.
        evening += bump(h, 20.25, 0.8, 60)
        day += bump(h, 12.25, 0.6, 50)
    return base + morning + day + evening


def pv(t, actual):
    h = hour(t)
    if h < 5.75 or h > 20.75:
        return 0.0
    x = math.sin(math.pi * (h - 5.75) / 15.0)
    clear = 0.78 * x ** 1.6
    if not actual:
        return clear * 0.92
    # Cloud bank around 10:00-12:00 and a thinner one mid-afternoon.
    cloud = 1.0 - bump(h, 10.9, 0.9, 0.55) - bump(h, 15.5, 0.6, 0.30)
    return max(0.0, clear * cloud)


def wind(t, actual):
    h = hour(t)
    base = 0.30 + 0.06 * math.sin(2 * math.pi * (h - 3) / 24)
    front = bump(h, 19.5, 2.6, 0.22)
    if not actual:
        return min(1.0, base + front * 0.8)
    # The front arrives earlier and stronger than forecast; a lull before it.
    lull = bump(h, 16.0, 1.2, 0.12)
    return max(0.0, min(1.0, base + bump(h, 19.0, 2.0, 0.45) - lull))


def write(path, header, values):
    with open(path, "w") as f:
        f.write(header + "\n")
        for v in values:
            f.write(f"{v:.4f}\n" if header == "pu" else f"{v:.1f}\n")


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
        os.path.dirname(__file__), "..", "scenarios", "cloudy_tuesday", "curves")
    os.makedirs(out, exist_ok=True)
    for kind, actual in (("forecast", False), ("actual", True)):
        write(os.path.join(out, f"tue_demand_{kind}.csv"), "kw", [demand(t, actual) for t in range(TICKS)])
        write(os.path.join(out, f"tue_pv_{kind}.csv"), "pu", [pv(t, actual) for t in range(TICKS)])
        write(os.path.join(out, f"tue_wind_{kind}.csv"), "pu", [wind(t, actual) for t in range(TICKS)])


if __name__ == "__main__":
    main()
