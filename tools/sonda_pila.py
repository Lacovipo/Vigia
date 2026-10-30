"""Consumo de pila de la búsqueda: umbral mínimo (bisección sobre
RUST_MIN_STACK) frente a la profundidad.

`RUST_MIN_STACK` fija la pila de los hilos creados con `thread::spawn` sin
tamaño explícito (el de búsqueda y los de Lazy SMP); un desbordamiento mata el
proceso con el código 0xC00000FD (3221225725) en Windows. Se busca el menor
tamaño con el que la búsqueda a profundidad d aún termina. Los umbrales salen en
saltos de 64 KiB (67K, 131K...). Se sospecha que Windows reserva la pila por
bloques de ese tamaño, pero no está comprobado: acotan, no miden al byte.
Sirve para comparar dos binarios (0.37 puso `MoveList` y las claves de
orden en la pila) y para vigilar el margen frente a los 2 MiB por defecto con
MAX_PLY = 128.

Uso: python -I tools/sonda_pila.py <exe1> [<exe2> ...]
"""
import os
import queue
import subprocess
import sys
import threading
import time

POS = {
    "kiwipete": "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "mediojuego": "r1bq1rk1/pp2ppbp/2np1np1/8/3NP3/2N1BP2/PPPQ2PP/R3KB1R w KQ - 0 9",
}
DEPTHS = [4, 6, 8, 10, 12, 14]


def survives(exe, stack, fen, depth):
    env = dict(os.environ, RUST_MIN_STACK=str(stack))
    p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.STDOUT, text=True, bufsize=1, env=env,
                         encoding="utf-8", errors="replace")
    q = queue.Queue()

    def rd():
        for l in p.stdout:
            q.put(l.rstrip("\r\n"))
        q.put(None)

    threading.Thread(target=rd, daemon=True).start()
    try:
        p.stdin.write("uci\nsetoption name Hash value 16\nisready\nposition fen %s\ngo depth %d\n" % (fen, depth))
        p.stdin.flush()
    except OSError:
        return False
    ok = False
    end = time.time() + 120
    while time.time() < end:
        try:
            l = q.get(timeout=max(0.05, end - time.time()))
        except queue.Empty:
            break
        if l is None:
            break
        if l.startswith("bestmove"):
            ok = True
            break
    try:
        p.stdin.write("quit\n")
        p.stdin.flush()
    except OSError:
        pass
    try:
        p.wait(5)
    except Exception:
        p.kill()
    try:
        p.stdin.close()
    except OSError:
        pass
    return ok


def threshold(exe, fen, depth):
    lo, hi = 8 * 1024, 2 * 1024 * 1024  # lo falla (supuesto), hi sobrevive
    if survives(exe, lo, fen, depth):
        return lo
    if not survives(exe, hi, fen, depth):
        return None
    while hi - lo > 4 * 1024:
        mid = (lo + hi) // 2 // 1024 * 1024
        if survives(exe, mid, fen, depth):
            hi = mid
        else:
            lo = mid
    return hi


def main():
    for exe in sys.argv[1:]:
        print("==", exe)
        for name, fen in POS.items():
            row = []
            for d in DEPTHS:
                t = threshold(exe, fen, d)
                row.append("%d:%s" % (d, "?" if t is None else "%dK" % (t // 1024)))
            print("  %-10s umbral de pila por profundidad  %s" % (name, "  ".join(row)))
            sys.stdout.flush()


main()
