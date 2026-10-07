#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Comprueba que el entrenador calcula los mismos rasgos que el motor.

    python tools/nnue/check_rasgos.py <directorio-corpus> [...] [--n 3000] [--semilla 3]

El índice de rasgo se calcula en **tres sitios**: `src/nnue.rs`, `netfmt.py`
(escalar, posición a posición) y `dataset.py` (vectorizado con numpy, que es lo
que alimenta el entrenamiento). El vector dorado de `golden.py` ata los dos
primeros entre sí. Este fichero ata el tercero, que hasta 0.34 no lo ataba nada:
se comprobó una vez con un guion suelto y se dio por bueno.

Por qué importa que sea permanente: si `dataset.rasgos` se desvía de
`netfmt.active_features`, **no falla nada**. El entrenamiento converge, la
cuantización cuadra, `check_red.py` da idéntico entero a entero —compara Rust con
`netfmt`, no con `dataset`— y el motor juega con una red entrenada sobre un
espacio de entrada permutado. El único síntoma es que juega peor.

Hay que ejecutarlo cada vez que cambie `rasgos` en `dataset.py`, o cualquiera de
las funciones de índice de `netfmt.py`.
"""
import argparse
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402
import netfmt as nf  # noqa: E402
from check_red import a_fen  # noqa: E402


def columna_del_rey(fen, rey):
    """Columna (0-7) del rey `rey` ('K' o 'k') en la FEN, o None si no está."""
    for fila in fen.split()[0].split('/'):
        columna = 0
        for ch in fila:
            if ch.isdigit():
                columna += int(ch)
            else:
                if ch == rey:
                    return columna
                columna += 1
    return None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('directorios', nargs='+')
    ap.add_argument('--n', type=int, default=3000, help='posiciones por fragmento')
    ap.add_argument('--fragmentos', type=int, default=4)
    ap.add_argument('--semilla', type=int, default=3)
    args = ap.parse_args()

    rng = np.random.default_rng(args.semilla)
    comparaciones = distintas = 0
    # Lo que tiene que haber salido en la muestra para que «cero distintas»
    # signifique algo: los dos espejos, el caso en que solo uno de los dos
    # bandos lo lleva, y derechos de enroque.
    solo_uno = con_enroque = espejo_blancas = espejo_negras = posiciones = 0
    for f in ds.fragmentos(args.directorios)[:args.fragmentos]:
        filas = np.sort(rng.choice(len(f), min(args.n, len(f)), replace=False))
        datos = f.datos[filas]
        blancas, negras = ds.rasgos(datos)
        for i in range(len(filas)):
            fen = a_fen(datos[i])
            for persp, matriz in ((0, blancas), (1, negras)):
                esperado = nf.active_features(fen, persp)
                obtenido = sorted(int(x) for x in matriz[i] if x != ds.PAD)
                comparaciones += 1
                if esperado != obtenido:
                    distintas += 1
                    if distintas <= 5:
                        print('DIFIERE  %s  perspectiva %d\n   netfmt  %s\n   dataset %s'
                              % (fen, persp, esperado, obtenido))
            b = (columna_del_rey(fen, 'K') or 0) >= 4
            n = (columna_del_rey(fen, 'k') or 0) >= 4
            espejo_blancas += b
            espejo_negras += n
            solo_uno += b != n
            con_enroque += fen.split()[2] != '-'
            posiciones += 1

    print('%d comparaciones sobre %d posiciones: %d distintas' % (comparaciones, posiciones, distintas))
    print('   cobertura: espejo para blancas %d, para negras %d, solo para uno de los dos %d, con enroque %d'
          % (espejo_blancas, espejo_negras, solo_uno, con_enroque))
    if distintas:
        sys.exit('dataset.rasgos y netfmt.active_features NO coinciden: el entrenador vería otros rasgos que el motor.')
    if min(solo_uno, con_enroque, espejo_blancas, posiciones - espejo_blancas) == 0:
        sys.exit('la muestra no cubre algún caso (ver cobertura): «cero distintas» no demuestra nada.')
    print('OK: el entrenador y el motor ven los mismos rasgos.')


if __name__ == '__main__':
    main()
