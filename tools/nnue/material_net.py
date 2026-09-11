#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Construye a mano la red de la fase 1 del plan: solo material, sin entrenar.

    python tools/nnue/material_net.py

Escribe `nets/material-256.bin`.

Para qué sirve una red tan tonta: **para cualquier FEN se sabe con lápiz y
papel qué tiene que devolver**. Eso convierte la fase 1 en la verificación más
barata y más completa del bucle entero —formato de fichero, `include_bytes!`,
cargador, cotas, acumulador incremental, inferencia, cuantización, `uci eval`
y banco— sin gastar una hora de generación de datos ni una época de
entrenamiento. El riesgo de integración se paga antes que el de arquitectura.

Cómo funciona. 64 parejas de neuronas simétricas. Para cada perspectiva:

    a0 = 64 + d        a1 = 64 - d

donde `d` es el material propio menos el ajeno en unidades de 64 cp, con
pesos del transformador de +unidades para las piezas propias y -unidades para
las ajenas (peón 2, caballo 5, alfil 5, torre 8, dama 14, rey 0), y el signo
invertido en la segunda neurona de cada pareja. Con SCReLU:

    t0 - t1 = ((64+d)^2 - (64-d)^2) >> 4 = 16 d

y es exacto, no aproximado: el término d^2/16 tiene la misma parte fraccionaria
en los dos cuadrados y el `>> 4` la trunca igual en ambos. Con pesos de salida
+2016 y -2016 en esas 128 neuronas del bando que mueve, y 0 en todo lo demás:

    S  = 64 parejas * 2016 * 16 d = 2.064.384 d
    cp = S / 32.258 ~= 64 d                 (satura en +-4.032 si |d| > 63)

El test `the_material_network_scores_material_exactly_as_computed_by_hand` de
`src/nnue.rs` repite esta cuenta en Rust, independientemente, sobre posiciones
concretas.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import netfmt as nf  # noqa: E402

UNITS = {nf.PAWN: 2, nf.KNIGHT: 5, nf.BISHOP: 5, nf.ROOK: 8, nf.QUEEN: 14, nf.KING: 0}
PAIRS = 64
BIAS = 64
OUT_WEIGHT = 2016


def build():
    ft_w = [0] * (nf.FEATURES * nf.HIDDEN)
    ft_b = [0] * nf.HIDDEN
    for neuron in range(2 * PAIRS):
        ft_b[neuron] = BIAS
        sign = 1 if neuron % 2 == 0 else -1
        for kind, units in UNITS.items():
            for sq in range(64):
                own = kind * 64 + sq            # propia: theirs = 0
                theirs = 384 + kind * 64 + sq   # ajena:  theirs = 1
                ft_w[own * nf.HIDDEN + neuron] = sign * units
                ft_w[theirs * nf.HIDDEN + neuron] = -sign * units

    out_w = [0] * (nf.MAX_BUCKETS * 2 * nf.HIDDEN)
    for bucket in range(nf.MAX_BUCKETS):
        base = bucket * 2 * nf.HIDDEN
        for neuron in range(2 * PAIRS):         # solo la mitad del que mueve
            out_w[base + neuron] = OUT_WEIGHT if neuron % 2 == 0 else -OUT_WEIGHT
    out_b = [0] * nf.MAX_BUCKETS

    # El valor de partida del plan para el mapa de cubos. Aquí da igual: los
    # ocho cubos son idénticos. Pero el fichero tiene que llevar uno válido.
    bucket_of = [max(0, min(7, (p - 1) // 4)) for p in range(33)]
    return ft_w, ft_b, out_w, out_b, bucket_of


def check_by_hand(net):
    """La cuenta del docstring, sobre tres posiciones, antes de escribir nada."""
    casos = [
        ('rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1', 0),
        ('4k3/8/8/8/8/8/8/3QK3 w - - 0 1', 896),       # d = +14
        ('4k3/8/8/8/8/8/8/3QK3 b - - 0 1', -896),      # mueve el que no tiene la dama
        ('4k3/8/8/8/8/8/QQQQQ3/4K3 w - - 0 1', 4032),  # d = 70: satura
    ]
    for fen, esperado in casos:
        _, _, cp = nf.forward(net, fen)
        assert cp == esperado, (fen, cp, esperado)


def main():
    ft_w, ft_b, out_w, out_b, bucket_of = build()
    net = dict(ft_w=ft_w, ft_b=ft_b, out_w=out_w, out_b=out_b,
               bucket_of=bucket_of, n_buckets=nf.MAX_BUCKETS, k=0)
    check_by_hand(net)

    raiz = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
    destino = os.path.join(raiz, 'nets', 'material-256.bin')
    os.makedirs(os.path.dirname(destino), exist_ok=True)
    sha = nf.write_net(destino, ft_w, ft_b, out_w, out_b, bucket_of, nf.MAX_BUCKETS)

    releida = nf.read_net(destino)
    check_by_hand(releida)
    print('%s  %d bytes  sha256 %s' % (destino, os.path.getsize(destino), sha))


if __name__ == '__main__':
    main()
