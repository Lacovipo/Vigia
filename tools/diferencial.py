#!/usr/bin/env python3
"""Diferencial de dos binarios UCI: ¿hacen exactamente la misma búsqueda?

Un cambio "de solo velocidad" promete el mismo árbol, jugada a jugada. El banco
de velocidad lo comprueba en 12 posiciones; esto lo comprueba en cientos, y no
solo al final de la búsqueda sino en CADA profundidad: puntuación, nodos y
variante principal de cada iteración, más la jugada elegida. Con un hilo y
`Variety` apagado la búsqueda es determinista, así que cualquier diferencia es
un cambio de comportamiento y no ruido.

Las posiciones salen de tres sitios: una lista fija de casos que rompen cosas
(mates, ahogados, jaque doble, clavadas, al paso, promociones, reloj de 50
jugadas, finales de peones, la posición de 218 jugadas), el libro de
aperturas del banco, y partidas aleatorias jugadas desde el libro (para llegar
a medios juegos y finales desordenados que ningún libro contiene). Las jugadas
legales las da el propio motor con `perft 1`.

    python -I tools/diferencial.py --a "Release/Vigia 0.36.exe" --b target/release/vigia.exe

Con `--hash 1` la tabla es diminuta y los reemplazos constantes: ahí se ve
cualquier diferencia en la política de reemplazo, que con la tabla grande
apenas se ejercita.

Sale con código 0 si TODAS las posiciones coinciden, 1 si alguna difiere.
Solo usa la biblioteca estándar. Una CPU: los dos motores se ejecutan uno
tras otro, nunca a la vez.
"""
import argparse
import random
import re
import subprocess
import sys
import time

FIJAS = [
    "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
    "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
    "8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1",
    "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
    "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
    "r4rk1/1pp1qppp/p1np1n2/2b1p1B1/2B1P1b1/P1NP1N2/1PP1QPPP/R4RK1 w - - 0 10",
    "R6R/3Q4/1Q4Q1/4Q3/2Q4Q/Q4Q2/pp1Q4/kBNN1KB1 w - - 0 1",
    "7k/5Q2/6K1/8/8/8/8/8 w - - 99 1",
    "7k/5Q2/6K1/8/8/8/8/8 w - - 98 1",
    "4k3/8/8/8/8/8/8/3QK3 b - - 100 1",
    "4k3/8/8/8/8/8/8/3QK3 w - - 98 1",
    "8/8/8/8/8/8/2Q5/k2K4 w - - 0 1",
    "4R1k1/5ppp/8/8/8/8/8/4K3 b - - 0 1",
    "8/8/8/4k3/4P3/8/8/K7 b - - 0 1",
    "8/8/8/1P2k3/8/8/8/K7 w - - 0 1",
    "8/8/8/8/k1pP3R/8/8/4K3 b - d3 0 1",
    "8/8/8/4k3/3Pp3/8/8/4K3 b - d3 0 1",
    "4k3/r7/5N2/8/8/8/8/4R1K1 b - - 0 1",
    "4r3/7b/4R3/5B2/r2NK3/3P4/8/1b5k w - - 0 1",
    "r3k2r/pppq1ppp/2n1bn2/3pp3/3PP3/2N1BN2/PPPQ1PPP/R3K2R w KQkq - 0 1",
    "6k1/5ppp/8/8/8/8/8/R3R1K1 w - - 0 1",
    "1R2k3/P7/8/8/1r6/8/8/4K3 b - - 0 1",
    "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RN2KBNR w KQkq - 4 4",
    "rnbqkbnr/ppp1p1pp/8/3pPp2/8/8/PPPP1PPP/RNBQKBNR w KQkq f6 0 3",
    "k7/8/8/3n4/4K3/8/8/8 w - - 0 1",
    "4k3/8/8/8/8/8/4P3/4K3 w - - 0 1",
    "8/P7/8/8/8/8/7p/k1K5 w - - 0 1",
    "6kq/5ppp/8/8/8/8/8/R5K1 w - - 0 1",
    "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1",
    "8/8/p7/1P6/K7/8/8/7k w - - 0 1",
    "8/5pk1/6p1/8/8/6P1/5PK1/8 w - - 0 1",
    "3r2k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1",
]

MOVE = re.compile(r"^([a-h][1-8][a-h][1-8][qrbn]?): \d+$")

# Posición cuya lista de jugadas no coincide con la de ninguna otra del diferencial:
# ver `Motor.acepta`.
CENTINELA = "k7/8/8/8/8/8/8/K6R w - - 0 1"


class Motor:
    def __init__(self, exe, hash_mb):
        self.exe = exe
        self.p = subprocess.Popen(
            [exe], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1
        )
        self.send("uci")
        self.leer_hasta("uciok")
        self.send(f"setoption name Hash value {hash_mb}")
        self.send("isready")
        self.leer_hasta("readyok")

    def send(self, linea):
        self.p.stdin.write(linea + "\n")
        self.p.stdin.flush()

    def leer_hasta(self, prefijo):
        lineas = []
        while True:
            linea = self.p.stdout.readline()
            if not linea:
                raise RuntimeError(f"{self.exe}: el motor cerró la salida esperando '{prefijo}'")
            linea = linea.rstrip("\r\n")
            lineas.append(linea)
            if linea.startswith(prefijo):
                return lineas

    def posicion(self, fen, jugadas):
        self.send(f"position fen {fen} moves {' '.join(jugadas)}" if jugadas else f"position fen {fen}")

    def lista(self):
        """Jugadas legales de la posición actual del motor."""
        self.send("perft 1")
        return [m.group(1) for l in self.leer_hasta("Nodes searched") if (m := MOVE.match(l))]

    def legales(self, fen, jugadas):
        self.posicion(fen, jugadas)
        return self.lista()

    def acepta(self, fen, jugadas):
        """None si el motor entiende la posición; si no, un texto que dice por qué.

        Un `position` que el motor rechaza (FEN imposible, o una jugada ilegal de la
        lista) se ignora en silencio, y la búsqueda siguiente analizaría la posición
        de antes: dos motores que «coinciden» en ella no habrían comparado nada. Se
        detecta dejando antes una posición centinela y viendo si la lista de
        jugadas legales sigue siendo la suya."""
        self.posicion(CENTINELA, [])
        centinela = self.lista()
        for i in range(len(jugadas) + 1):
            self.posicion(CENTINELA, [])
            legales = self.legales(fen, jugadas[:i])
            if legales == centinela:
                return f"el motor ignoró 'position fen {fen} moves {' '.join(jugadas[:i])}'"
            if i < len(jugadas) and jugadas[i] not in legales:
                return f"la jugada {jugadas[i]} no es legal tras 'moves {' '.join(jugadas[:i])}'"
        return None

    def analiza(self, fen, jugadas, limite):
        """{profundidad: (puntuación, nodos, variante)} y la jugada elegida."""
        self.send("ucinewgame")
        self.send("isready")
        self.leer_hasta("readyok")
        self.posicion(fen, jugadas)
        self.send(f"go {limite}")
        lineas = self.leer_hasta("bestmove")
        por_profundidad = {}
        for l in lineas:
            if not l.startswith("info depth"):
                continue
            t = l.split()
            d = int(t[t.index("depth") + 1])
            i = t.index("score")
            puntuacion = f"{t[i + 1]} {t[i + 2]}"
            nodos = int(t[t.index("nodes") + 1])
            variante = " ".join(t[t.index("pv") + 1:]) if "pv" in t else ""
            por_profundidad[d] = (puntuacion, nodos, variante)
        return por_profundidad, lineas[-1].split()[1]

    def cierra(self):
        try:
            self.send("quit")
            self.p.wait(timeout=5)
        except Exception:
            self.p.kill()


def lee_libro(ruta):
    with open(ruta, encoding="utf-8") as f:
        return [l.strip() for l in f if l.strip() and not l.startswith("#")]


def compara(a, b, fen, jugadas, limite):
    """None si coinciden; si no, un texto que dice dónde se separan."""
    ra, ba = a.analiza(fen, jugadas, limite)
    rb, bb = b.analiza(fen, jugadas, limite)
    for d in sorted(set(ra) | set(rb)):
        if ra.get(d) != rb.get(d):
            return f"profundidad {d}:\n      A {ra.get(d)}\n      B {rb.get(d)}"
    if ba != bb:
        return f"jugada elegida: A {ba}, B {bb}"
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--a", required=True, help="binario de referencia (p. ej. la versión liberada)")
    ap.add_argument("--b", required=True, help="binario candidato")
    ap.add_argument("--libro", default="banco/libros/vigia-2000.epd")
    ap.add_argument("--fens", help="archivo con posiciones extra, una por línea: FEN o FEN moves j1 j2 ... "
                    "(las líneas que empiezan por # se ignoran); p. ej. tools/posiciones_adversarias.txt")
    ap.add_argument("--del-libro", type=int, default=200, help="posiciones tomadas tal cual del libro")
    ap.add_argument("--aleatorias", type=int, default=200, help="partidas aleatorias desde el libro")
    ap.add_argument("--plies", type=int, default=40, help="máximo de jugadas aleatorias por partida")
    ap.add_argument("--profundidad", type=int, default=10)
    ap.add_argument("--nodos", type=int, help="busca con `go nodes N` en vez de `go depth`: así la búsqueda se "
                    "aborta a mitad de una iteración (con N pequeño, en la primera), que es el camino que `go depth` "
                    "nunca ejercita")
    ap.add_argument("--hash", type=int, default=64, help="MB de tabla de los dos motores")
    ap.add_argument("--semilla", type=int, default=20261029)
    ap.add_argument("--max-fallos", type=int, default=15, help="cuántas diferencias detallar")
    args = ap.parse_args()

    rng = random.Random(args.semilla)
    libro = lee_libro(args.libro)
    casos = [(fen, []) for fen in FIJAS]
    if args.fens:
        for linea in lee_libro(args.fens):
            fen, _, jugadas = linea.partition(" moves ")
            casos.append((fen.strip(), jugadas.split()))
    casos += [(fen, []) for fen in rng.sample(libro, min(args.del_libro, len(libro)))]
    externos = len(casos)  # las demás las juega el propio motor, así que son legales por construcción

    a, b = Motor(args.a, args.hash), Motor(args.b, args.hash)
    for motor in (a, b):
        for fen, jugadas in casos[:externos]:
            problema = motor.acepta(fen, jugadas)
            if problema:
                print(f"POSICIÓN NO VÁLIDA para {motor.exe}: {problema}\n"
                      "Un `position` rechazado se ignora en silencio y se compararía la posición anterior.")
                sys.exit(2)
    for fen in rng.sample(libro, min(args.aleatorias, len(libro))):
        jugadas = []
        for _ in range(rng.randint(4, args.plies)):
            legales = b.legales(fen, jugadas)
            if not legales:
                break
            jugadas.append(rng.choice(legales))
        casos.append((fen, jugadas))

    limite = f"nodes {args.nodos}" if args.nodos else f"depth {args.profundidad}"
    print(f"{len(casos)} posiciones, go {limite}, Hash {args.hash}, semilla {args.semilla}")
    print(f"A = {args.a}\nB = {args.b}", flush=True)
    iguales, fallos, inicio = 0, [], time.time()
    for n, (fen, jugadas) in enumerate(casos, 1):
        dif = compara(a, b, fen, jugadas, limite)
        if dif is None:
            iguales += 1
        else:
            fallos.append((fen, jugadas, dif))
            if len(fallos) <= args.max_fallos:
                print(f"DIFERENCIA en la posición {n}: position fen {fen} moves {' '.join(jugadas)}\n    {dif}", flush=True)
        if n % 50 == 0:
            print(f"  {n}/{len(casos)}  iguales {iguales}  distintas {len(fallos)}  ({time.time() - inicio:.0f} s)", flush=True)
    a.cierra()
    b.cierra()
    print(f"RESULTADO: {iguales} iguales, {len(fallos)} distintas, de {len(casos)} ({time.time() - inicio:.0f} s)")
    sys.exit(0 if not fallos else 1)


if __name__ == "__main__":
    main()
