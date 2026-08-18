"""Atribuye el error de calibración de Vigia a términos concretos del eval,
usando el desglose por términos del comando `eval` nuevo."""

import csv
import os
import pathlib
import re
import statistics
import subprocess

# Relativa al propio script: tools/calibration/ cuelga de la raíz del
# repositorio, así que el binario está siempre dos niveles más arriba. La ruta
# absoluta que había aquí solo valía en el equipo donde se escribió, y esta
# carpeta se sincroniza entre varios. VIGIA_EXE la sobrescribe si hace falta.
VIGIA = os.environ.get(
    "VIGIA_EXE",
    str(pathlib.Path(__file__).resolve().parents[2] / "target" / "release" / "vigia.exe"),
)


class BreakdownClient:
    def __init__(self):
        self.p = subprocess.Popen(
            [VIGIA], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1,
        )

    def breakdown(self, fen):
        self.p.stdin.write(f"position fen {fen}\neval\nisready\n")
        self.p.stdin.flush()
        terms = {}
        while True:
            line = self.p.stdout.readline().strip()
            if line == "readyok":
                return terms
            m = re.match(r"(\w+):\s+([+-]?\d+)$", line)
            if m:
                terms[m.group(1)] = int(m.group(2))
            m = re.match(r"endgame_scale: x(\d+)/64$", line)
            if m:
                terms["_scale"] = int(m.group(1))

    def quit(self):
        self.p.stdin.write("quit\n")
        self.p.stdin.flush()
        self.p.wait(timeout=5)


rows = []
with open("calib.csv") as f:
    for row in csv.DictReader(f):
        r = {k: (v if k == "fen" else int(v)) for k, v in row.items()}
        r["consensus"] = statistics.median([r["sf"], r["obsidian"], r["berserk"], r["caissa"]])
        r["spread"] = max(r["sf"], r["obsidian"], r["berserk"], r["caissa"]) - min(
            r["sf"], r["obsidian"], r["berserk"], r["caissa"]
        )
        r["err"] = r["vigia"] - r["consensus"]
        rows.append(r)

quiet = [r for r in rows if r["spread"] <= 120]
client = BreakdownClient()
for r in quiet:
    r["terms"] = client.breakdown(r["fen"])
client.quit()

TERM_NAMES = [
    "material", "pst", "mobility", "pawn_structure", "bishop_pair", "bishop_pawns",
    "king_safety", "mop_up", "rook_file", "rook_seventh", "knight_outpost",
    "pawn_endgame", "tempo", "threats", "kpk_exact",
]


def report(label, sample):
    if len(sample) < 8:
        return
    print(f"\n=== {label} (n={len(sample)}) ===")
    print(f"{'término':>16} {'|medio|':>8} {'hacia_err':>10}")
    for t in TERM_NAMES:
        vals = [r["terms"].get(t, 0) for r in sample]
        if all(v == 0 for v in vals):
            continue
        mean_abs = statistics.mean(abs(v) for v in vals)
        # Cuánto empuja este término en la dirección del error de cada
        # posición (positivo = el término contribuye al error).
        toward = statistics.mean(v * (1 if r["err"] > 0 else -1) for v, r in zip(vals, sample))
        print(f"{t:>16} {mean_abs:8.1f} {toward:+10.1f}")


report("FINALES phase<4 (sesgo +152)", [r for r in quiet if r["phase"] < 4])
report("IGUALADAS |cons|<60 y |vigia|>=100 (ruido)", [r for r in quiet if abs(r["consensus"]) < 60 and abs(r["vigia"]) >= 100])
report("TODAS con |err|>120", [r for r in quiet if abs(r["err"]) > 120])
report("CONTROL: |err|<40 (sanas)", [r for r in quiet if abs(r["err"]) < 40])

print("\n--- Peores 12 en finales phase<4 ---")
for r in sorted([r for r in quiet if r["phase"] < 4], key=lambda r: -abs(r["err"]))[:12]:
    big = {k: v for k, v in sorted(r["terms"].items(), key=lambda kv: -abs(kv[1]))[:4] if k != "_scale"}
    print(f"err={r['err']:+6.0f} vigia={r['vigia']:+5d} cons={r['consensus']:+6.0f}  {big}  {r['fen']}")

print("\n--- Peores 12 igualadas-ruidosas ---")
for r in sorted([r for r in quiet if abs(r["consensus"]) < 60], key=lambda r: -abs(r["vigia"]))[:12]:
    big = {k: v for k, v in sorted(r["terms"].items(), key=lambda kv: -abs(kv[1]))[:4] if k != "_scale"}
    print(f"vigia={r['vigia']:+5d} cons={r['consensus']:+6.0f}  {big}  {r['fen']}")
