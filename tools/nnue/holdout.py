#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Compara la red con la HCE sobre partidas que la red no ha visto.

    python tools/nnue/holdout.py muestra <corpus> [...] --n 20000 --salida h.txt
    ./target/release/generador.exe evaluar --red <red.bin> --epd h.txt > h-evals.txt
    python tools/nnue/holdout.py comparar h.txt h-evals.txt --k 160.7

Es el criterio 4 de la fase 3 del plan, que era *referencia, no veto*, y aquí se
mide de la única forma que significa algo: **sobre las mismas etiquetas y con las
mismas partidas apartadas que usó el entrenamiento**. `muestra` reproduce el
reparto de `train.py` —el hash de la partida decide su lado, no el de la
posición— y saca posiciones útiles del lado de validación.

Lo que se compara es la pérdida del plan, `σ(eval/K)` contra `σ(cp/K)`: la HCE
entra como el evaluador estático que generó esas etiquetas y la red como el que
pretende sustituirla. La HCE parte con desventaja —las etiquetas son búsquedas de
25.000 nodos, no evaluaciones estáticas— y justamente por eso el número que
importa es cuánto de esa distancia recorta la red.

No decide nada por sí solo: quien decide es el banco de pruebas (fase 5).
"""
import argparse
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402
from check_red import a_fen  # noqa: E402

TROZO = 500_000


def muestra(args):
    rng = np.random.default_rng(args.semilla)
    frags = ds.fragmentos(args.directorios)
    candidatos = []
    for i, f in enumerate(frags):
        n = len(f)
        util = np.zeros(n, dtype=bool)
        for a in range(0, n, TROZO):
            util[a:a + TROZO] = ds.util(f.datos[a:a + TROZO], f.hce[a:a + TROZO])
        # El mismo reparto que train.py, con la misma aritmética: si aquí se
        # calculara de otra forma, la comparación se haría sobre posiciones que
        # la red sí ha visto y saldría un número halagador y falso.
        partida = np.cumsum(np.ascontiguousarray(f.datos['ply']) == 0) - 1
        es_validacion = ((partida * 2654435761 + i * 40503) % 10_000) < args.validacion * 10_000
        filas = np.flatnonzero(util & es_validacion)
        candidatos.append(np.stack([np.full(len(filas), i), filas], axis=1))
    candidatos = np.concatenate(candidatos)
    elegidos = candidatos[rng.choice(len(candidatos), min(args.n, len(candidatos)), replace=False)]
    with open(args.salida, 'w', encoding='utf-8', newline='\n') as salida:
        salida.write('# %d posiciones de validacion, semilla %d: fen etiqueta_cp_desde_quien_mueve\n'
                     % (len(elegidos), args.semilla))
        for i, fila in elegidos:
            registro = frags[int(i)].datos[int(fila)]
            salida.write('%s %d\n' % (a_fen(registro), int(registro['cp'])))
    print('%s: %d posiciones de validacion de %d candidatas' % (args.salida, len(elegidos), len(candidatos)))


def lee_muestra(ruta):
    fens, etiquetas = [], []
    for linea in open(ruta, encoding='utf-8'):
        if not linea.strip() or linea.startswith('#'):
            continue
        campos = linea.split()
        fens.append(' '.join(campos[:6]))
        etiquetas.append(int(campos[6]))
    return fens, np.array(etiquetas, dtype=np.float64)


def lee_evals(ruta):
    fens, hce, red = [], [], []
    for linea in open(ruta, encoding='utf-8'):
        if not linea.strip() or linea.startswith('#'):
            continue
        campos = linea.rstrip('\n').split('|')
        fens.append(campos[0])
        hce.append(int(campos[1]))
        red.append(int(campos[2]))
    return fens, np.array(hce, dtype=np.float64), np.array(red, dtype=np.float64)


def sigmoide(x, k):
    return 1.0 / (1.0 + np.exp(-x / k))


def comparar(args):
    fens_m, etiqueta = lee_muestra(args.muestra)
    fens_e, hce, red = lee_evals(args.evals)
    # Los relojes no cuentan: el motor reescribe la FEN al volcarla y no tiene
    # por que devolver los mismos. Lo que tiene que cuadrar es la posicion.
    clave = [' '.join(f.split()[:4]) for f in fens_m]
    if clave != [' '.join(f.split()[:4]) for f in fens_e]:
        raise SystemExit('las dos listas de posiciones no coinciden (%d y %d lineas)' % (len(fens_m), len(fens_e)))
    negras = np.array([f.split()[1] == 'b' for f in fens_m])
    signo = np.where(negras, -1.0, 1.0)
    hce, red = hce * signo, red * signo   # todo desde quien mueve, como la etiqueta

    objetivo = sigmoide(etiqueta, args.k)
    print('%d posiciones de validacion, K = %.1f' % (len(etiqueta), args.k))
    print('%-8s %10s %10s %10s %10s' % ('', 'perdida', 'error medio', 'p90', 'correlacion'))
    for nombre, v in (('HCE', hce), ('red', red)):
        perdida = float(np.mean((sigmoide(v, args.k) - objetivo) ** 2))
        error = np.abs(v - etiqueta)
        print('%-8s %10.6f %7.1f cp %7.1f cp %10.3f'
              % (nombre, perdida, error.mean(), np.percentile(error, 90), np.corrcoef(v, etiqueta)[0, 1]))
    perdida_hce = float(np.mean((sigmoide(hce, args.k) - objetivo) ** 2))
    perdida_red = float(np.mean((sigmoide(red, args.k) - objetivo) ** 2))
    print('la red recorta el %.1f %% de la perdida de la HCE' % (100 * (1 - perdida_red / perdida_hce)))
    print('desviacion tipica: etiqueta %.1f cp, HCE %.1f cp, red %.1f cp'
          % (etiqueta.std(), hce.std(), red.std()))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='orden', required=True)
    m = sub.add_parser('muestra')
    m.add_argument('directorios', nargs='+')
    m.add_argument('--n', type=int, default=20000)
    m.add_argument('--semilla', type=int, default=7)
    m.add_argument('--validacion', type=float, default=0.02)
    m.add_argument('--salida', required=True)
    c = sub.add_parser('comparar')
    c.add_argument('muestra')
    c.add_argument('evals')
    c.add_argument('--k', type=float, default=160.7)
    args = ap.parse_args()
    {'muestra': muestra, 'comparar': comparar}[args.orden](args)


if __name__ == '__main__':
    main()
