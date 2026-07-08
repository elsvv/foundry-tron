#!/usr/bin/env bash
# Diff two single-version `foundry-bench --json-output` summaries into a
# noise-aware Markdown regression/improvement report.
#
# Usage: compare-bench.sh <base.json> <candidate.json>
# Env: BASE_LABEL, CANDIDATE_LABEL, FLOOR_PCT (2.0), NOISE_MULT (1.0),
#      FAIL_ON_REGRESSION (exit 1 on regression; off by default).
# Exits 0 after writing a report; non-zero only on bad input (or a regression
# when FAIL_ON_REGRESSION=1).
set -euo pipefail

BASE_JSON="${1:-}"
CANDIDATE_JSON="${2:-}"

if [ -z "$BASE_JSON" ] || [ ! -f "$BASE_JSON" ]; then
    echo "error: base JSON not provided or missing: '$BASE_JSON'" >&2
    exit 2
fi
if [ -z "$CANDIDATE_JSON" ] || [ ! -f "$CANDIDATE_JSON" ]; then
    echo "error: candidate JSON not provided or missing: '$CANDIDATE_JSON'" >&2
    exit 2
fi

BASE_JSON="$BASE_JSON" \
CANDIDATE_JSON="$CANDIDATE_JSON" \
BASE_LABEL="${BASE_LABEL:-base}" \
CANDIDATE_LABEL="${CANDIDATE_LABEL:-this PR}" \
FLOOR_PCT="${FLOOR_PCT:-2.0}" \
NOISE_MULT="${NOISE_MULT:-1.0}" \
FAIL_ON_REGRESSION="${FAIL_ON_REGRESSION:-}" \
python3 - <<'EOF'
import json, os, sys

# Guard the strict comparison so a delta sitting exactly on the floor reads as neutral.
EPS = 1e-9

base_label = os.environ["BASE_LABEL"]
cand_label = os.environ["CANDIDATE_LABEL"]
floor = float(os.environ["FLOOR_PCT"])
noise_mult = float(os.environ["NOISE_MULT"])
fail_on_regression = os.environ.get("FAIL_ON_REGRESSION") == "1"

with open(os.environ["BASE_JSON"]) as f:
    base = json.load(f)
with open(os.environ["CANDIDATE_JSON"]) as f:
    cand = json.load(f)


def mean_of(v):
    # Tolerate bare-float entries from older summary files.
    return v["mean"] if isinstance(v, dict) else float(v)


def rel_stddev(v):
    if not isinstance(v, dict):
        return 0.0
    m = v.get("mean") or 0.0
    s = v.get("stddev")
    if not m or s is None:
        return 0.0
    return abs(s / m) * 100.0


def fmt_duration(seconds):
    if seconds < 0.001:
        return f"{seconds * 1000.0:.2f} ms"
    if seconds < 1.0:
        return f"{seconds:.3f} s"
    if seconds < 60.0:
        return f"{seconds:.2f} s"
    minutes = int(seconds // 60)
    return f"{minutes}m {seconds % 60:.1f}s"


rows = []
regressions = improvements = neutral = dropped = 0
for key in sorted(base.keys() | cand.keys()):
    b = base.get(key)
    c = cand.get(key)
    if b is None:
        rows.append((key, "N/A", fmt_duration(mean_of(c)), "🆕 new"))
        continue
    if c is None:
        rows.append((key, fmt_duration(mean_of(b)), "N/A", "⚠️ dropped"))
        dropped += 1
        continue

    bm, cm = mean_of(b), mean_of(c)
    delta = (cm - bm) / bm * 100.0 if bm else 0.0
    # Combined relative noise of the two runs.
    band = (rel_stddev(b) ** 2 + rel_stddev(c) ** 2) ** 0.5 * noise_mult
    # reth-style verdict: flag only when the whole band clears the floor. Wall
    # time is lower-is-better, so improvement is positive when the candidate is
    # faster (delta < 0).
    improvement = -delta
    if improvement - band > floor + EPS:
        emoji = "🟢"
        improvements += 1
    elif improvement + band < -floor - EPS:
        emoji = "🔴"
        regressions += 1
    else:
        emoji = "⚪"
        neutral += 1

    sign = "+" if delta > 0 else ""
    change = f"{sign}{delta:.1f}% {emoji} (±{band:.1f}%, floor {floor:.1f}%)"
    rows.append((key, fmt_duration(bm), fmt_duration(cm), change))

out = []
if regressions:
    headline = f"🔴 {regressions} regression(s)"
elif improvements:
    headline = f"🟢 {improvements} improvement(s), no regressions"
else:
    headline = "⚪ no significant changes"
counts = f"{regressions} regression, {improvements} improvement, {neutral} neutral"
if dropped:
    counts += f", {dropped} dropped"
out.append(f"**{headline}** ({counts})")
out.append("")
out.append(f"| Benchmark | {base_label} | {cand_label} | Change |")
out.append("|-----------|------|---------|--------|")
for key, b, c, change in rows:
    out.append(f"| `{key}` | {b} | {c} | {change} |")
out.append("")
out.append(f"<sub>🔴 regression · 🟢 improvement · ⚪ within noise. "
           f"A change is flagged only when the whole noise band (±) clears the "
           f"{floor:.1f}% floor.</sub>")

print("\n".join(out))
sys.exit(1 if fail_on_regression and regressions else 0)
EOF
