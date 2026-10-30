"""Prueba de humo del motor por UCI, pensada para varios hilos.

El Juez juega con `Threads=1`; esto es lo que ejercita la TT compartida sin
cerrojo (0.37) y las paradas con `stop` bajo Lazy SMP. Solo biblioteca estándar.

Uso: python -I tools/soak_uci.py <exe> <epd> <n> <hilos> <hash_mb> <modo>
modo: time  -> go movetime 200, sin ucinewgame (TT contaminada entre posiciones)
      depth -> go depth 11, con ucinewgame entre posiciones
      stop  -> go infinite y stop tras un retardo aleatorio

Comprueba, por posicion: que llegue bestmove, que sea legal segun `perft 1`
(o 0000/(none) si no hay jugadas), y que el proceso no muera ni deje panics.
"""
import queue
import random
import re
import subprocess
import sys
import threading
import time

SPECIAL = [
    "R6R/3Q4/1Q4Q1/4Q3/2Q4Q/Q4Q2/pp1Q4/kBNN1KB1 w - - 0 1",  # 218 jugadas legales
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "rnb1kbnr/pppp1ppp/8/4p3/6Pq/5P2/PPPPP2P/RNBQKBNR w KQkq - 1 3",  # mate
    "7k/5Q2/6K1/8/8/8/8/8 b - - 0 1",  # ahogado
    "8/8/8/4k3/8/4K3/4P3/8 w - - 0 1",
    "8/8/8/8/3k4/8/8/KQ6 w - - 0 1",
    "8/P6k/8/8/8/8/p6K/8 w - - 0 1",
    "8/8/8/KPp4r/8/8/8/7k w - c6 0 1",  # al paso clavado en fila
    "4k3/8/8/8/8/8/8/R3K2R w KQ - 0 1",
    "r3k2r/8/8/8/8/8/8/4K3 b kq - 0 1",
]


class Eng:
    def __init__(self, exe):
        self.p = subprocess.Popen(
            [exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1,
            encoding="utf-8", errors="replace")
        self.q = queue.Queue()
        self.all = []
        threading.Thread(target=self._rd, daemon=True).start()

    def _rd(self):
        for line in self.p.stdout:
            self.q.put(line.rstrip("\r\n"))
        self.q.put(None)

    def send(self, s):
        self.p.stdin.write(s + "\n")
        self.p.stdin.flush()

    def until(self, pred, timeout):
        end = time.time() + timeout
        out = []
        while True:
            rem = end - time.time()
            if rem <= 0:
                raise TimeoutError("sin respuesta; ultimas lineas: %r" % out[-3:])
            try:
                line = self.q.get(timeout=rem)
            except queue.Empty:
                raise TimeoutError("sin respuesta; ultimas lineas: %r" % out[-3:])
            if line is None:
                raise EOFError("el motor termino; ultimas lineas: %r" % out[-3:])
            out.append(line)
            self.all.append(line)
            if pred(line):
                return out


def legal_moves(e, fen):
    e.send("position fen " + fen)
    e.send("perft 1")
    lines = e.until(lambda l: l.startswith("Nodes searched"), 15)
    moves = set()
    for l in lines:
        m = re.match(r"^([a-h][1-8][a-h][1-8][qrbn]?): \d+", l)
        if m:
            moves.add(m.group(1))
    return moves


def main():
    exe, epd, n, threads, hash_mb, mode = sys.argv[1:7]
    n, threads, hash_mb = int(n), int(threads), int(hash_mb)
    fens = [l.strip() for l in open(epd, encoding="utf-8")
            if l.strip() and not l.startswith("#")]
    step = max(1, len(fens) // n)
    sample = SPECIAL + fens[::step][:n]
    rnd = random.Random(20261029)

    e = Eng(exe)
    e.send("uci")
    e.until(lambda l: l == "uciok", 10)
    e.send("setoption name Threads value %d" % threads)
    e.send("setoption name Hash value %d" % hash_mb)
    e.send("isready")
    e.until(lambda l: l == "readyok", 10)

    fallos = []
    depths, nodes_tot, t_tot, overshoot = [], 0, 0.0, 0.0
    for i, fen in enumerate(sample):
        try:
            legal = legal_moves(e, fen)
            if mode == "depth":
                e.send("ucinewgame")
                e.send("isready")
                e.until(lambda l: l == "readyok", 10)
                e.send("position fen " + fen)
            t0 = time.time()
            if mode == "time":
                e.send("go movetime 200")
                lim = 8
            elif mode == "depth":
                e.send("go depth 11")
                lim = 120
            else:
                e.send("go infinite")
                time.sleep(rnd.uniform(0.02, 0.4))
                e.send("stop")
                lim = 8
            out = e.until(lambda l: l.startswith("bestmove"), lim)
            dt = time.time() - t0
            best = out[-1].split()[1] if len(out[-1].split()) > 1 else ""
            if legal:
                if best not in legal:
                    fallos.append((i, fen, "bestmove ilegal %r" % best))
            elif best not in ("0000", "(none)", "none"):
                fallos.append((i, fen, "sin jugadas y bestmove %r" % best))
            info = [l for l in out if l.startswith("info") and " depth " in l]
            if info:
                m = re.search(r" depth (\d+)", info[-1])
                if m:
                    depths.append(int(m.group(1)))
                m = re.search(r" nodes (\d+)", info[-1])
                if m:
                    nodes_tot += int(m.group(1))
                    t_tot += dt
            if mode == "time":
                overshoot = max(overshoot, dt - 0.2)
        except (TimeoutError, EOFError) as ex:
            fallos.append((i, fen, "%s: %s" % (type(ex).__name__, ex)))
            break

    try:
        e.send("quit")
        rc = e.p.wait(15)
    except Exception as ex:  # el motor no sale
        rc = "no salio: %r" % ex
        e.p.kill()
    panics = [l for l in e.all if "panick" in l.lower()]
    print("modo=%s hilos=%d hash=%dMB posiciones=%d" % (mode, threads, hash_mb, len(sample)))
    print("fallos=%d codigo_salida=%s panics=%d" % (len(fallos), rc, len(panics)))
    if depths:
        print("profundidad media=%.2f nps aprox=%.0f max retraso=%.3fs" % (
            sum(depths) / len(depths), nodes_tot / max(t_tot, 1e-9), overshoot))
    for f in fallos[:10]:
        print("FALLO", f)
    for p in panics[:5]:
        print("PANIC", p)
    sys.exit(1 if fallos or panics or rc != 0 else 0)


main()
