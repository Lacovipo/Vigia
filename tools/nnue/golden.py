#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""El vector dorado de la red, cruzado en los dos sentidos (§7.1 de docs/PlanNNUE.md).

    cargo build --release
    ./target/release/generador.exe indices --semilla 20260911 --salida tools/nnue/indices.txt
    python tools/nnue/golden.py

Un solo oráculo no basta: si un mismo lado fabricara las posiciones, los índices
y el resultado esperado, un fallo suyo produciría un dorado con el mismo fallo y
el test pasaría. Por eso cada lado es oráculo de la mitad que el otro no puede
fabricar.

**Sentido A — los índices los manda Rust.** `indices.txt` lo escribió el
generador con el código del motor. Aquí se recalculan los rasgos activos de cada
FEN con `netfmt.py` y se falla si difiere una sola entrada. Ancla el índice de
rasgo, la orientación de la perspectiva, la distinción propia/ajena y los cuatro
rasgos de enroque.

**Sentido B — la pasada hacia delante la manda Python.** Se construye una red
de pesos pseudoaleatorios con semilla fija, se escribe en `golden-net.bin`, y en
`golden.txt` se anota para cada FEN la suma `S` en i32, el cubo y el cp. El test
`golden_vector_matches_the_python_forward_pass` de `src/nnue.rs` carga esa red
con el cargador del motor y exige igualdad exacta, entero a entero. Ancla la
disposición [rasgo][dimensión], el endianismo, el clamp, el `>> 4`, la elección
de cubo, la división final con su redondeo y el signo.

Por qué una red aleatoria y no la de material: aquella tiene casi todos los
pesos a cero y sus ocho cubos son idénticos, así que no vería una disposición
traspuesta ni un cubo equivocado. Una red con todos los pesos distintos, sí.
"""
import hashlib
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import netfmt as nf  # noqa: E402

AQUI = os.path.dirname(os.path.abspath(__file__))
SEMILLA_RED = 20260911


class Rng:
    """xorshift64, el mismo que usan los tests de src/nnue.rs para sus redes."""

    def __init__(self, semilla):
        self.x = semilla & 0xFFFFFFFFFFFFFFFF

    def siguiente(self):
        x = self.x
        x ^= (x << 13) & 0xFFFFFFFFFFFFFFFF
        x ^= x >> 7
        x ^= (x << 17) & 0xFFFFFFFFFFFFFFFF
        self.x = x
        return x

    def simetrico(self, cota):
        return self.siguiente() % (2 * cota + 1) - cota


def red_aleatoria():
    rng = Rng(SEMILLA_RED)
    ft_w = [rng.simetrico(90) for _ in range(nf.FEATURES * nf.HIDDEN)]
    ft_b = [rng.simetrico(60) for _ in range(nf.HIDDEN)]
    out_w = [rng.simetrico(60) for _ in range(nf.MAX_BUCKETS * 2 * nf.HIDDEN)]
    out_b = [rng.simetrico(5000) for _ in range(nf.MAX_BUCKETS)]
    bucket_of = [max(0, min(nf.MAX_BUCKETS - 1, (p - 1) // 4)) for p in range(33)]
    return ft_w, ft_b, out_w, out_b, bucket_of


def leer_indices(ruta):
    posiciones = []
    with open(ruta, encoding='utf-8') as f:
        for n, linea in enumerate(f, 1):
            linea = linea.rstrip('\n')
            if not linea or linea.startswith('#'):
                continue
            categoria, fen, blancas, negras = linea.split('|')
            posiciones.append((n, categoria, fen,
                               [int(x) for x in blancas.split()],
                               [int(x) for x in negras.split()]))
    return posiciones


def sentido_a(posiciones):
    fallos = 0
    for n, categoria, fen, blancas, negras in posiciones:
        for persp, de_rust in ((nf.WHITE, blancas), (nf.BLACK, negras)):
            de_python = nf.active_features(fen, persp)
            if de_python != de_rust:
                fallos += 1
                if fallos <= 5:
                    print('  linea %d (%s), perspectiva %s:\n    rust   %s\n    python %s\n    %s'
                          % (n, categoria, 'blancas' if persp == nf.WHITE else 'negras',
                             de_rust, de_python, fen))
    if fallos:
        raise SystemExit('SENTIDO A FALLA: %d perspectivas discrepan entre Rust y Python' % fallos)
    print('sentido A: %d posiciones, los índices de Rust y de Python coinciden en las dos perspectivas'
          % len(posiciones))


def sentido_b(posiciones):
    ft_w, ft_b, out_w, out_b, bucket_of = red_aleatoria()
    ruta_red = os.path.join(AQUI, 'golden-net.bin')
    sha = nf.write_net(ruta_red, ft_w, ft_b, out_w, out_b, bucket_of, nf.MAX_BUCKETS, k=400)
    red = nf.read_net(ruta_red)

    inicio = time.time()
    lineas = [
        '# Vector dorado de la red, sentido B: la pasada hacia delante calculada por Python.',
        '# Red: golden-net.bin, pesos pseudoaleatorios con semilla %d, sha256 %s' % (SEMILLA_RED, sha),
        '# Una línea por posición, en el orden de indices.txt: S cubo cp  (cp desde el bando que mueve)',
    ]
    for _, _, fen, _, _ in posiciones:
        s, cubo, cp = nf.forward(red, fen)
        lineas.append('%d %d %d' % (s, cubo, cp))
    with open(os.path.join(AQUI, 'golden.txt'), 'w', encoding='utf-8', newline='\n') as f:
        f.write('\n'.join(lineas) + '\n')
    print('sentido B: %d pasadas hacia delante en %.0f s, red %s' % (len(posiciones), time.time() - inicio, sha[:16]))


def main():
    ruta = os.path.join(AQUI, 'indices.txt')
    if not os.path.exists(ruta):
        raise SystemExit('falta %s: generarlo primero con `generador indices`' % ruta)
    posiciones = leer_indices(ruta)
    sentido_a(posiciones)
    sentido_b(posiciones)


if __name__ == '__main__':
    main()
