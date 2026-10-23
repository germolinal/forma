#!/usr/bin/env python3
"""Charts one or more benchmark runs side by side, e.g. without and with
OpenTelemetry. Standard library only.

    bench/chart.py --run "Without OpenTelemetry=bench/results/console" \
                   --run "With OpenTelemetry=bench/results/otel" \
                   --out bench/results

Writes, in --out:
- chart.svg: one panel per measure (Forma's time per message, CPU per message,
  peak memory, messages per second), a bar per target and run;
- summary.json: the same numbers, which the website reads
  (website/src/data/bench.json, copied by `make bench-site`).
"""

import argparse
import json
import pathlib
import re
from datetime import date
from html import escape

TARGET_NAMES = {
    "none": "No database",
    "mongo": "MongoDB",
    "mongo-mt": "MongoDB, multi-tenant",
}
TARGET_ORDER = ["none", "mongo", "mongo-mt"]

# Categorical slots 1 and 2 of the dataviz reference palette, light and dark,
# validated as a pair (scripts/validate_palette.js): CVD ΔE ≥ 24 in both modes.
SERIES_LIGHT = ["#2a78d6", "#eb6834"]
SERIES_DARK = ["#3987e5", "#d95926"]


def load(path):
    return json.loads(pathlib.Path(path).read_text())


def summarise_run(run_dir):
    """The measures charted, per target, from one run's JSON files."""
    run_dir = pathlib.Path(run_dir)
    targets = {}
    for target in TARGET_ORDER:
        files = {s: run_dir / f"{target}.{s}.json" for s in ("1-latency", "2-throughput", "3-realistic")}
        if not all(f.exists() for f in files.values()):
            continue
        lat, thr, real = (load(files[s]) for s in ("1-latency", "2-throughput", "3-realistic"))
        rss = [d["process"]["rss_mb_peak"] for d in (lat, thr, real)]
        targets[target] = {
            "overhead_ms": lat["phases"]["forma_overhead"]["p50"],
            "overhead_p90_ms": lat["phases"]["forma_overhead"]["p90"],
            "cpu_ms_per_message": real["process"]["cpu_ms_per_request"],
            "rss_mb_one_conversation": lat["process"]["rss_mb_peak"],
            "rss_mb_peak": max(rss),
            "messages_per_s": thr["requests_per_s"],
            "realistic": {
                "concurrency": real["concurrency"],
                "messages_per_s": real["requests_per_s"],
                "latency_p50_ms": real["latency_ms"]["p50"],
                "cores_busy": real["process"]["cores_busy"],
            },
            "errors": lat["errors"] + thr["errors"] + real["errors"],
        }
    report = run_dir / "report.md"
    machine = ""
    if report.exists():
        lines = report.read_text().splitlines()
        machine = next((l for l in lines if re.search(r"CPUs\.$", l)), "")
    return {"machine": machine, "targets": targets}


# ---- the chart ------------------------------------------------------------------

PANELS = [
    ("overhead_ms", "Forma's time per message", "ms, median, on top of the LLM", "{:.1f} ms"),
    ("cpu_ms_per_message", "CPU per message", "ms, whole tool loop, streamed", "{:.1f} ms"),
    ("rss_mb_peak", "Peak memory", "MB, up to 64 conversations at once", "{:.0f} MB"),
    ("messages_per_s", "Messages per second", "with an LLM that answers instantly", "{:,.0f}/s"),
]

WIDTH = 960
PANEL_W = 440
GAP_X = 40
LEFT = 30
LABEL_W = 150
BAR_H = 14  # ≤ 24px, per the mark specs
BAR_GAP = 2  # the surface gap between touching bars
GROUP_GAP = 16


def bar_path(x, y, w, h, r=4):
    """A bar from the baseline (square) to its data end (4px rounded)."""
    r = min(r, w / 2, h / 2)
    if w <= 0:
        return ""
    return (
        f"M{x:.1f},{y:.1f}h{w - r:.1f}a{r},{r} 0 0 1 {r},{r}"
        f"v{h - 2 * r:.1f}a{r},{r} 0 0 1 {-r},{r}h{-(w - r):.1f}z"
    )


def chart(runs, title_suffix):
    labels = list(runs.keys())
    targets = [t for t in TARGET_ORDER if all(t in r["targets"] for r in runs.values())]
    group_h = len(labels) * BAR_H + (len(labels) - 1) * BAR_GAP
    panel_h = 54 + len(targets) * group_h + (len(targets) - 1) * GROUP_GAP + 16
    top = 96
    rows = (len(PANELS) + 1) // 2
    height = top + rows * panel_h + (rows - 1) * 28 + 40

    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{height}" '
        f'viewBox="0 0 {WIDTH} {height}" font-family="Inter, ui-sans-serif, system-ui, sans-serif" '
        f'role="img" aria-labelledby="t d">',
        "<style>",
        ":root{--surface:#fcfcfb;--ink:#141922;--muted:#5b6575;--grid:#e6e3dc;"
        + "".join(f"--s{i}:{c};" for i, c in enumerate(SERIES_LIGHT))
        + "}",
        "@media (prefers-color-scheme: dark){:root{--surface:#1a1a19;--ink:#ece8df;--muted:#97a1b2;--grid:#2c2c2a;"
        + "".join(f"--s{i}:{c};" for i, c in enumerate(SERIES_DARK))
        + "}}",
        ".bg{fill:var(--surface)} .ink{fill:var(--ink)} .muted{fill:var(--muted)} .axis{stroke:var(--grid);stroke-width:1}",
        "".join(f".s{i}{{fill:var(--s{i})}}" for i in range(len(labels))),
        "text{font-size:12px} .title{font-size:18px;font-weight:600} .ptitle{font-size:14px;font-weight:600}",
        ".value{font-variant-numeric:tabular-nums}",
        "</style>",
        f'<title id="t">Forma benchmark{escape(title_suffix)}</title>',
        f'<desc id="d">{escape(", ".join(labels))}: Forma\'s time per message, CPU per message, peak memory and messages per second, per storage.</desc>',
        f'<rect class="bg" width="{WIDTH}" height="{height}"/>',
        f'<text class="ink title" x="{LEFT}" y="34">Forma benchmark{escape(title_suffix)}</text>',
    ]
    # Legend: always present for two or more series.
    x = LEFT
    for i, label in enumerate(labels):
        out.append(f'<rect class="s{i}" x="{x}" y="52" width="12" height="12" rx="3"/>')
        out.append(f'<text class="ink" x="{x + 18}" y="62">{escape(label)}</text>')
        x += 18 + 7.2 * len(label) + 28

    for p, (key, ptitle, unit, fmt) in enumerate(PANELS):
        px = LEFT + (p % 2) * (PANEL_W + GAP_X)
        py = top + (p // 2) * (panel_h + 28)
        values = [runs[l]["targets"][t][key] for t in targets for l in labels]
        vmax = max(values) or 1
        bar_max = PANEL_W - LABEL_W - 80  # room for the value at the tip
        out.append(f'<text class="ink ptitle" x="{px}" y="{py + 14}">{escape(ptitle)}</text>')
        out.append(f'<text class="muted" x="{px}" y="{py + 32}">{escape(unit)}</text>')
        y = py + 50
        base = px + LABEL_W
        out.append(f'<line class="axis" x1="{base}" x2="{base}" y1="{y - 4}" y2="{y + len(targets) * group_h + (len(targets) - 1) * GROUP_GAP + 4}"/>')
        for t in targets:
            out.append(f'<text class="ink" x="{px}" y="{y + group_h / 2 + 4}">{escape(TARGET_NAMES[t])}</text>')
            for i, label in enumerate(labels):
                v = runs[label]["targets"][t][key]
                w = max(2.0, bar_max * v / vmax)
                text = fmt.format(v)
                out.append(
                    f'<path class="s{i}" d="{bar_path(base, y, w, BAR_H)}">'
                    f"<title>{escape(TARGET_NAMES[t])}, {escape(label)}: {escape(text)}</title></path>"
                )
                out.append(f'<text class="ink value" x="{base + w + 6:.1f}" y="{y + BAR_H - 3}">{escape(text)}</text>')
                y += BAR_H + BAR_GAP
            y += GROUP_GAP - BAR_GAP

    machine = next((r["machine"] for r in runs.values() if r["machine"]), "")
    note = "Mock LLM; each message is a full tool loop (two LLM calls and a tool)."
    if machine:
        note = f"{machine} {note}"
    out.append(f'<text class="muted" x="{LEFT}" y="{height - 16}">{escape(note)}</text>')
    out.append("</svg>")
    return "\n".join(out) + "\n"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--run", action="append", required=True, metavar="LABEL=DIR", help="a run to chart (repeat)")
    ap.add_argument("--out", required=True, help="where chart.svg and summary.json go")
    ap.add_argument("--title", default="", help="appended to the chart's title")
    args = ap.parse_args()

    runs = {}
    for spec in args.run:
        label, _, run_dir = spec.partition("=")
        if not run_dir:
            ap.error(f"--run takes LABEL=DIR, got {spec!r}")
        runs[label] = summarise_run(run_dir)
        if not runs[label]["targets"]:
            ap.error(f"no complete target in {run_dir}")

    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / "chart.svg").write_text(chart(runs, args.title))
    summary = {"date": date.today().isoformat(), "runs": runs}
    (out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    print(f"wrote {out / 'chart.svg'} and {out / 'summary.json'}")


if __name__ == "__main__":
    main()
