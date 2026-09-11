#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Ajusta K, la escala de la sigmoide, sobre el corpus.

    python tools/nnue/fit_k.py <directorio> [<directorio> ...]

El entrenador compara `σ(pred/K)` con `σ(cp/K)`: la pérdida vive en el espacio
de la puntuación esperada, no en centipeones, para que un error de 50 cp cuente
mucho en una posición igualada —donde se decide la partida— y poco en una con
+900, donde da igual. K fija esa geometría, y se elige como la que hace que
`σ(cp/K)` prediga mejor el resultado real de la partida, por mínimos cuadrados.

No se hereda el K = 140 que salió sobre el corpus del banco: allí el 70 % de
las partidas estaban adjudicadas por la propia evaluación, y el resultado *era*
la puntuación (§5.6 del plan). Este corpus no tiene adjudicación.
"""
import argparse
import math
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402

TROZO = 500_000


def cargar(directorios, maximo, semilla):
    cps, resultados = [], []
    for f in ds.fragmentos(directorios):
        for a in range(0, len(f), TROZO):
            datos, hce = f.datos[a:a + TROZO], f.hce[a:a + TROZO]
            util = ds.util(datos, hce)
            cps.append(np.ascontiguousarray(datos['cp'])[util].astype(np.float64))
            resultados.append(np.ascontiguousarray(datos['resultado'])[util].astype(np.float64) / 2.0)
    cp, resultado = np.concatenate(cps), np.concatenate(resultados)
    if len(cp) > maximo:
        elegidos = np.random.default_rng(semilla).choice(len(cp), maximo, replace=False)
        cp, resultado = cp[elegidos], resultado[elegidos]
    return cp, resultado


def error(k, cp, resultado):
    return float(np.mean((1.0 / (1.0 + np.exp(-cp / k)) - resultado) ** 2))


def ajustar(cp, resultado, bajo=30.0, alto=3000.0, iteraciones=60):
    """Sección áurea sobre log K: el error es unimodal en K y la escala útil va
    de decenas a miles de centipeones."""
    a, b = math.log(bajo), math.log(alto)
    phi = (math.sqrt(5) - 1) / 2
    c, d = b - phi * (b - a), a + phi * (b - a)
    fc, fd = error(math.exp(c), cp, resultado), error(math.exp(d), cp, resultado)
    for _ in range(iteraciones):
        if fc < fd:
            b, d, fd = d, c, fc
            c = b - phi * (b - a)
            fc = error(math.exp(c), cp, resultado)
        else:
            a, c, fc = c, d, fd
            d = a + phi * (b - a)
            fd = error(math.exp(d), cp, resultado)
    return math.exp((a + b) / 2)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('directorios', nargs='+')
    ap.add_argument('--maximo', type=int, default=5_000_000, help='registros útiles como mucho')
    ap.add_argument('--semilla', type=int, default=1)
    args = ap.parse_args()

    cp, resultado = cargar(args.directorios, args.maximo, args.semilla)
    k = ajustar(cp, resultado)
    print('%d registros utiles; tablas %.1f %%' % (len(cp), 100 * np.mean(resultado == 0.5)))
    for prueba in (100, 200, 300, 400, 600, 800):
        print('  K = %4d  error %.5f' % (prueba, error(prueba, cp, resultado)))
    print('K = %.1f  error %.5f' % (k, error(k, cp, resultado)))


if __name__ == '__main__':
    main()
