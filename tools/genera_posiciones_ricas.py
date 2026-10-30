"""Genera posiciones con muchísima movilidad (listas de más de 64 jugadas).

Para qué: la prueba diferencial de partidas reales casi nunca llega a nodos
interiores con más de 64 jugadas pseudolegales, y ahí es donde el bitset de
tranquilas buscadas (`tried_quiets`, 5 palabras) y la ordenación por claves
usan sus palabras altas y su desempate por posición de 9 bits. Aquí cada bando
lleva dos damas, dos torres, un alfil y un caballo sobre un tablero casi vacío
(y a veces algún peón): la movilidad pasa de cien jugadas.

La legalidad de cada FEN la decide el propio motor (rechaza reyes adyacentes,
bando que no mueve en jaque, etc.), y las jugadas legales las cuenta con
`perft 1`. Un FEN rechazado se reconoce porque la posición se queda en la
inicial (20 jugadas).

    python -I tools/genera_posiciones_ricas.py <exe> <n> <min_jugadas> <semilla> [piezas] > salida.txt

Solo biblioteca estándar. Una CPU.
"""
import random
import re
import subprocess
import sys

MOVE = re.compile(r"^([a-h][1-8][a-h][1-8][qrbn]?): \d+$")


class Motor:
    def __init__(self, exe):
        self.p = subprocess.Popen([exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self.send("uci")
        self.hasta("uciok")

    def send(self, s):
        self.p.stdin.write(s + "\n")
        self.p.stdin.flush()

    def hasta(self, prefijo):
        out = []
        while True:
            l = self.p.stdout.readline()
            if not l:
                raise RuntimeError("el motor cerró la salida")
            l = l.rstrip("\r\n")
            out.append(l)
            if l.startswith(prefijo):
                return out

    def legales(self, fen):
        """Jugadas legales, o None si el motor rechazó el FEN."""
        self.send("position startpos")
        self.send("position fen " + fen)
        self.send("perft 1")
        jugadas = [m.group(1) for l in self.hasta("Nodes searched") if (m := MOVE.match(l))]
        if len(jugadas) == 20 and not fen.startswith("rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR"):
            return None
        return jugadas

    def cierra(self):
        try:
            self.send("quit")
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def a_fen(piezas, turno):
    filas = []
    for rank in range(7, -1, -1):
        fila, vacias = "", 0
        for file in range(8):
            c = piezas.get(rank * 8 + file)
            if c is None:
                vacias += 1
            else:
                fila += (str(vacias) if vacias else "") + c
                vacias = 0
        filas.append(fila + (str(vacias) if vacias else ""))
    return "/".join(filas) + f" {turno} - - 0 1"


def candidata(rng, juego):
    casillas = rng.sample(range(64), 24)
    piezas, it = {}, iter(casillas)
    for color in (str.upper, str.lower):
        for p in juego:
            piezas[next(it)] = color(p)
        for _ in range(rng.choice((0, 0, 1, 2, 3))):
            sq = next(it)
            if 8 <= sq < 56:  # un peón no puede estar en la fila 1 ni en la 8
                piezas[sq] = color("p")
    return a_fen(piezas, rng.choice("wb"))


def main():
    exe, n, minimo, semilla = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), int(sys.argv[4])
    # Piezas de cada bando (el rey primero). Menos piezas y más móviles dan más
    # jugadas que muchas piezas estorbándose: con "kqqrrbn" casi ninguna pasa de 75.
    juego = sys.argv[5] if len(sys.argv) > 5 else "kqqqrb"
    rng = random.Random(semilla)
    m = Motor(exe)
    vistas, hechas, intentos = set(), 0, 0
    while hechas < n and intentos < 200 * n:
        intentos += 1
        fen = candidata(rng, juego)
        if fen in vistas:
            continue
        vistas.add(fen)
        jugadas = m.legales(fen)
        if jugadas is None or len(jugadas) < minimo:
            continue
        print(fen)
        hechas += 1
    m.cierra()
    sys.stderr.write(f"{hechas} posiciones con >= {minimo} jugadas legales, de {intentos} candidatas\n")


main()
