"""Análisis del CSV de calibración: sesgos de Vigia vs consenso de oráculos."""

import csv
import statistics
import sys

CSV = sys.argv[1] if len(sys.argv) > 1 else "calib.csv"

rows = []
with open(CSV) as f:
    for row in csv.DictReader(f):
        r = {k: (v if k == "fen" else int(v)) for k, v in row.items()}
        r["consensus"] = statistics.median([r["sf"], r["obsidian"], r["berserk"], r["caissa"]])
        r["spread"] = max(r["sf"], r["obsidian"], r["berserk"], r["caissa"]) - min(
            r["sf"], r["obsidian"], r["berserk"], r["caissa"]
        )
        r["err"] = r["vigia"] - r["consensus"]
        rows.append(r)

print(f"{len(rows)} posiciones totales")

# Posiciones "fiables": los oráculos se ponen razonablemente de acuerdo entre
# sí (si divergen mucho, la posición es táctica/compleja y no sirve de patrón).
quiet = [r for r in rows if r["spread"] <= 120]
print(f"{len(quiet)} con consenso de oráculos (spread <= 120 cp)\n")


def describe(label, sample):
    if len(sample) < 10:
        return
    errs = [r["err"] for r in sample]
    vig = [r["vigia"] for r in sample]
    con = [r["consensus"] for r in sample]
    bias = statistics.mean(errs)
    mae = statistics.mean(abs(e) for e in errs)
    # Correlación y pendiente de regresión vigia ~ consenso
    mv, mc = statistics.mean(vig), statistics.mean(con)
    cov = sum((v - mv) * (c - mc) for v, c in zip(vig, con)) / len(vig)
    sdv = statistics.pstdev(vig)
    sdc = statistics.pstdev(con)
    corr = cov / (sdv * sdc) if sdv and sdc else 0
    slope = cov / (sdc * sdc) if sdc else 0
    # Concordancia de signo en posiciones no igualadas
    signif = [(v, c) for v, c in zip(vig, con) if abs(c) >= 50]
    sign_ok = sum(1 for v, c in signif if v * c > 0) / len(signif) if signif else 0
    print(
        f"{label:28s} n={len(sample):4d}  sesgo={bias:+6.1f}  MAE={mae:6.1f}  "
        f"corr={corr:.3f}  pendiente={slope:.2f}  amp_vigia={sdv:5.1f} vs amp_orac={sdc:5.1f}  "
        f"signo_ok={sign_ok:.0%}"
    )


describe("TODAS (consenso)", quiet)
describe("apertura (phase 20-24)", [r for r in quiet if r["phase"] >= 20])
describe("medio juego (phase 10-19)", [r for r in quiet if 10 <= r["phase"] < 20])
describe("final temprano (phase 4-9)", [r for r in quiet if 4 <= r["phase"] < 10])
describe("final (phase 0-3)", [r for r in quiet if r["phase"] < 4])
describe("igualadas (|con|<60)", [r for r in quiet if abs(r["consensus"]) < 60])
describe("decididas (|con|>200)", [r for r in quiet if abs(r["consensus"]) > 200])

print("\n--- Peores 20 desviaciones (con consenso fiable) ---")
for r in sorted(quiet, key=lambda r: -abs(r["err"]))[:20]:
    print(
        f"err={r['err']:+6.0f} vigia={r['vigia']:+6.0f} cons={r['consensus']:+6.0f} "
        f"(sf={r['sf']:+6.0f} ob={r['obsidian']:+6.0f} be={r['berserk']:+6.0f} ca={r['caissa']:+6.0f}) "
        f"ph={r['phase']:2d} {r['fen']}"
    )
