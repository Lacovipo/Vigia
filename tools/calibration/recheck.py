"""Re-consulta solo a Vigia sobre las posiciones de calib.csv (los oráculos
no han cambiado) y reescribe el CSV con la columna vigia actualizada."""

import csv
import os
import pathlib
import re
import subprocess
import sys

# Relativa al propio script: tools/calibration/ cuelga de la raíz del
# repositorio, así que el binario está siempre dos niveles más arriba. La ruta
# absoluta que había aquí solo valía en el equipo donde se escribió, y esta
# carpeta se sincroniza entre varios. VIGIA_EXE la sobrescribe si hace falta.
VIGIA = os.environ.get(
    "VIGIA_EXE",
    str(pathlib.Path(__file__).resolve().parents[2] / "target" / "release" / "vigia.exe"),
)

rows = list(csv.DictReader(open("calib.csv")))
p = subprocess.Popen([VIGIA], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL, text=True, bufsize=1)
for r in rows:
    p.stdin.write(f"position fen {r['fen']}\neval\nisready\n")
    p.stdin.flush()
    while True:
        line = p.stdout.readline().strip()
        m = re.match(r"Evaluation: (-?\d+)", line)
        if m:
            r["vigia"] = m.group(1)
        if line == "readyok":
            break
p.stdin.write("quit\n")
p.stdin.flush()

out = sys.argv[1] if len(sys.argv) > 1 else "calib2.csv"
with open(out, "w", newline="") as f:
    w = csv.DictWriter(f, fieldnames=rows[0].keys())
    w.writeheader()
    w.writerows(rows)
print(f"reescrito {out}")
