"""Re-consulta solo a Vigia sobre las posiciones de calib.csv (los oráculos
no han cambiado) y reescribe el CSV con la columna vigia actualizada."""

import csv
import re
import subprocess
import sys

VIGIA = r"C:\Users\JOSE CARLOS\OneDrive\IA\Ajedrez\Claude\target\release\vigia.exe"

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
