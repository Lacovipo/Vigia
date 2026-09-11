#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Estadísticas de un corpus de `generador datos`: lo que piden los criterios de
la fase 2 de docs/PlanNNUE.md.

    python tools/nnue/corpus_stats.py <directorio> [<directorio> ...] [--umbral-cubo N]

Informa de:

- supervivencia a cada filtro de §5.4 y a todos juntos;
- ocupación por número de piezas y la tabla `CUBO[33]` con la regla del
  millón: se parte de CUBO[p] = min(7, (p-1)/4) y se fusiona con su vecino
  cualquier cubo con menos posiciones útiles que el umbral;
- partidas, longitud y resultados;
- autocorrelación de la puntuación dentro de cada partida a desfase 2 plies
  (mismo bando al mover) con |cp| < 300, longitud de decorrelación, reparto de
  la varianza entre e intra partidas y **muestras efectivas**, que es el número
  con el que se dimensiona la red (§5.5 del plan).
"""
import argparse
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402


def cubos_con_regla(ocupacion, umbral):
    """ocupacion[p] = posiciones útiles con p piezas. Devuelve (CUBO[33], n)."""
    grupos = [[p for p in range(33) if max(0, min(7, (p - 1) // 4)) == c] for c in range(8)]
    grupos = [g for g in grupos if g]

    def peso(g):
        return sum(ocupacion[p] for p in g)

    while len(grupos) > 1 and min(peso(g) for g in grupos) < umbral:
        i = min(range(len(grupos)), key=lambda k: peso(grupos[k]))
        if i == 0:
            j = 1
        elif i == len(grupos) - 1:
            j = i - 1
        else:
            j = i - 1 if peso(grupos[i - 1]) <= peso(grupos[i + 1]) else i + 1
        a, b = sorted((i, j))
        grupos[a:b + 1] = [grupos[a] + grupos[b]]
    tabla = [0] * 33
    for c, g in enumerate(grupos):
        for p in g:
            tabla[p] = c
    return tabla, len(grupos)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('directorios', nargs='+')
    ap.add_argument('--umbral-cubo', type=int, default=1_000_000)
    args = ap.parse_args()

    frags = ds.fragmentos(args.directorios)
    total = sum(len(f) for f in frags)
    nodos = sorted(set(f.cabecera['nodos'] for f in frags))
    print('%d fragmentos, %d registros en bruto, a %s nodos' % (len(frags), total, nodos))

    descartes = {}
    utiles = 0
    ocupacion = np.zeros(33, dtype=np.int64)
    longitudes, resultados_blancas = [], []
    pares_x, pares_y = [], []
    suma_var_intra, n_intra, todos_cp = 0.0, 0, []
    efectivas_utiles = []

    for f in frags:
        datos, hce = f.datos, f.hce
        fil = ds.filtros(datos, hce)
        for k, v in fil.items():
            descartes[k] = descartes.get(k, 0) + int(v.sum())
        mascara = ~np.logical_or.reduce(list(fil.values()))
        utiles += int(mascara.sum())
        ocupacion += np.bincount(np.ascontiguousarray(hce[:, 1])[mascara], minlength=33)[:33]

        ply = np.ascontiguousarray(datos['ply']).astype(np.int64)
        cp = np.ascontiguousarray(datos['cp']).astype(np.int64)
        res = np.ascontiguousarray(datos['resultado']).astype(np.int64)
        negras = (np.ascontiguousarray(datos['meta']) & 1).astype(np.int64)
        inicios = np.flatnonzero(ply == 0)
        fines = np.append(inicios[1:], len(datos))
        for a, b in zip(inicios, fines):
            longitudes.append(b - a)
            resultados_blancas.append(int((res[a] - 1) * (1 - 2 * negras[a])))
            juego = cp[a:b]
            # El reparto de la varianza se mide sobre las etiquetas que de verdad
            # se entrenan, los registros utiles. La restriccion |cp| < 300 es solo
            # para la autocorrelacion, como en §5.5 del plan; aplicarla aqui daba
            # cifras que no se podian comparar con las medidas alli.
            etiquetas = juego[mascara[a:b]]
            if len(etiquetas) >= 2:
                suma_var_intra += float(np.var(etiquetas)) * len(etiquetas)
                n_intra += len(etiquetas)
                todos_cp.append(etiquetas)
            efectivas_utiles.append(int(mascara[a:b].sum()))
            if b - a > 2:
                x, y = juego[:-2], juego[2:]
                ok = (np.abs(x) < 300) & (np.abs(y) < 300)
                pares_x.append(x[ok])
                pares_y.append(y[ok])

    print()
    print('=== filtros de §5.4 (un registro puede caer en varios) ===')
    for k, v in sorted(descartes.items(), key=lambda kv: -kv[1]):
        print('  %-12s %10d  %5.2f %%' % (k, v, 100.0 * v / total))
    print('  %-12s %10d  %5.2f %%  <- supervivencia' % ('UTILES', utiles, 100.0 * utiles / total))

    print()
    print('=== partidas ===')
    n_partidas = len(longitudes)
    rb = np.array(resultados_blancas)
    print('  %d partidas, %.1f plies de media (mediana %d, maximo %d)'
          % (n_partidas, float(np.mean(longitudes)), int(np.median(longitudes)), int(np.max(longitudes))))
    print('  ganan blancas %.1f %%, tablas %.1f %%, ganan negras %.1f %%'
          % (100 * (rb == 1).mean(), 100 * (rb == 0).mean(), 100 * (rb == -1).mean()))

    print()
    print('=== ocupacion por numero de piezas (utiles) y tabla de cubos, umbral %d ===' % args.umbral_cubo)
    for p in range(2, 33):
        if ocupacion[p]:
            print('  %2d piezas %9d' % (p, ocupacion[p]))
    tabla, n_cubos = cubos_con_regla(ocupacion, args.umbral_cubo)
    print('  n_cubos = %d' % n_cubos)
    print('  CUBO[33] = %s' % tabla)

    print()
    print('=== correlacion y muestras efectivas (§5.5) ===')
    x = np.concatenate(pares_x) if pares_x else np.array([])
    y = np.concatenate(pares_y) if pares_y else np.array([])
    if len(x) > 10 and x.std() > 0 and y.std() > 0:
        rho = float(np.corrcoef(x, y)[0, 1])
        longitud = 2 * math.log(0.5) / math.log(rho) if 0 < rho < 1 else float('inf')
        conjunto = np.concatenate(todos_cp)
        var_total = float(np.var(conjunto))
        var_intra = suma_var_intra / max(n_intra, 1)
        entre = 1 - var_intra / var_total if var_total > 0 else float('nan')
        efectivas = sum(max(1.0, u / longitud) for u in efectivas_utiles if u > 0)
        print('  rho a desfase 2 con |cp| < 300: %.3f  (%d pares)' % (rho, len(x)))
        print('  longitud de decorrelacion (rho^k = 0,5): %.1f plies' % longitud)
        print('  etiquetas utiles: desviacion tipica total %.1f cp, intra-partida %.1f cp -> %.1f %% de la varianza entre partidas'
              % (math.sqrt(var_total), math.sqrt(var_intra), 100 * entre))
        print('  MUESTRAS EFECTIVAS: ~%.0f, o sea %.2f por parametro de Atalaya-256 (201.992)'
              % (efectivas, efectivas / 201_992))
    else:
        print('  (muy pocos datos para estimarla)')


if __name__ == '__main__':
    main()
