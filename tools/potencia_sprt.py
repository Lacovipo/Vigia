#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Antes de lanzar una tanda de decisión: ¿con qué probabilidad NO decide?

    python tools/potencia_sprt.py --elo 7 --tope 4000 8000
    python tools/potencia_sprt.py --elo 5 8 12 --tope 4000 \\
        --resumen banco/resultados/034-espejo-B/resumen.json

Simula el SPRT del banco (`elo0`, `elo1`, `alfa`, `beta`, comprobando tras cada
pareja) sobre un efecto real supuesto y un tope de parejas, y dice cuántas veces
acepta H1, cuántas H0 y cuántas agota el tope sin decidir.

Existe por un fallo concreto de 0.34. `034-espejo-A` se lanzó con un tope de
4.000 parejas para un efecto que se esperaba «entre +5 y +15», agotó el tope con
el LLR en +2,49 de +2,94 y el espejo no entró. Al escribirlo se dijo que con +7
«no decidir pasa una de cada cuatro veces», y así quedó en la preinscripción de
la tanda siguiente. Nadie lo había calculado: **pasa una de cada dos** (54 %).
El tope estaba mal puesto desde el principio, y con este cálculo delante se
habría visto antes de gastar cinco horas de máquina.

La regla que sale de ahí: el tope se elige para el efecto **más pequeño que se
quiere poder aprobar**, no para el que se espera, y la probabilidad de no
decidir se escribe en la preinscripción con su cifra.

La forma de la distribución de parejas (cuántas 0, ½, 1, 1½ y 2) sale de tandas
reales: por defecto, las tres del espejo, 10.065 parejas a 100 ms. Con
`--resumen` se usan otras. La fórmula del LLR es la del banco sin su
regularización de casillas vacías; reproduce los LLR de esas tandas con un error
de milésimas (2,493 contra 2,491 en `034-espejo-A`).

Necesita numpy, como las herramientas de `tools/nnue`.
"""
import argparse
import json
import math

import numpy as np

PUNTOS = np.array([0.0, 0.25, 0.5, 0.75, 1.0])
# 034-espejo-A, 034-espejo-A-estimacion y 034-espejo-B, sumadas.
PENTANOMIAL_POR_DEFECTO = [384 + 238 + 336, 893 + 540 + 763, 1340 + 856 + 1237, 936 + 594 + 842, 447 + 272 + 387]
# Las primeras parejas no deciden nada: el banco tampoco para ahí, y la varianza
# de una muestra de diez parejas no es una varianza.
MINIMO = 20


def puntuacion(elo):
    return 1.0 / (1.0 + 10.0 ** (-elo / 400.0))


def inclinar(p, objetivo):
    """La distribución más parecida a `p` cuya media es `objetivo`.

    Inclinación exponencial: conserva la forma medida (cuántas parejas se
    reparten y cuántas se barren) y solo desplaza la media. Es lo que distingue
    esto de suponer una normal con la varianza de un trinomio."""
    bajo, alto = -50.0, 50.0
    for _ in range(200):
        t = (bajo + alto) / 2
        q = p * np.exp(t * PUNTOS)
        q /= q.sum()
        if (q * PUNTOS).sum() < objetivo:
            bajo = t
        else:
            alto = t
    return q


def simular(p, elo, tope, elo0, elo1, alfa, beta, pruebas, rng):
    q = inclinar(p, puntuacion(elo))
    s0, s1 = puntuacion(elo0), puntuacion(elo1)
    alta, baja = math.log((1 - beta) / alfa), math.log(beta / (1 - alfa))
    n = np.arange(1, tope + 1)
    h1 = h0 = sin = 0
    parejas_hasta_decidir = 0
    lote = max(1, min(pruebas, 40_000_000 // tope))
    hechas = 0
    while hechas < pruebas:
        ahora = min(lote, pruebas - hechas)
        v = rng.choice(PUNTOS, size=(ahora, tope), p=q)
        suma, suma2 = np.cumsum(v, axis=1), np.cumsum(v * v, axis=1)
        media = suma / n
        varianza = np.maximum(suma2 / n - media * media, 1e-9)
        llr = (s1 - s0) * (2 * media - s0 - s1) * n / (2 * varianza)
        llr[:, :MINIMO] = 0
        arriba, abajo = llr >= alta, llr <= baja
        primera_arriba = np.where(arriba.any(1), arriba.argmax(1), tope)
        primera_abajo = np.where(abajo.any(1), abajo.argmax(1), tope)
        h1 += int((primera_arriba < primera_abajo).sum())
        h0 += int((primera_abajo < primera_arriba).sum())
        sin += int(((primera_arriba == tope) & (primera_abajo == tope)).sum())
        # El índice es la pareja en que cruza, contando desde cero; sin cruce, el tope.
        parejas_hasta_decidir += int(np.minimum(np.minimum(primera_arriba, primera_abajo) + 1, tope).sum())
        hechas += ahora
    return h1 / pruebas, h0 / pruebas, sin / pruebas, parejas_hasta_decidir / pruebas


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('--elo', type=float, nargs='+', required=True, help='efecto real que se supone, en Elo')
    ap.add_argument('--tope', type=int, nargs='+', required=True, help='max_parejas de la tanda')
    ap.add_argument('--elo0', type=float, default=0.0)
    ap.add_argument('--elo1', type=float, default=5.0)
    ap.add_argument('--alfa', type=float, default=0.05)
    ap.add_argument('--beta', type=float, default=0.05)
    ap.add_argument('--resumen', nargs='*', default=[], help='resumen.json de tandas de las que tomar el pentanomial')
    ap.add_argument('--pruebas', type=int, default=20000)
    ap.add_argument('--semilla', type=int, default=12345)
    args = ap.parse_args()

    cuentas = np.zeros(5)
    for ruta in args.resumen:
        with open(ruta, encoding='utf-8') as f:
            cuentas += np.array(json.load(f)['pentanomial'], float)
    if not args.resumen:
        cuentas = np.array(PENTANOMIAL_POR_DEFECTO, float)
    p = cuentas / cuentas.sum()
    media = (p * PUNTOS).sum()
    sigma = math.sqrt((p * PUNTOS * PUNTOS).sum() - media * media)
    print('pentanomial de %d parejas: sigma por pareja %.4f' % (cuentas.sum(), sigma))
    print('SPRT elo0=%g elo1=%g alfa=%g beta=%g, %d simulaciones por fila\n'
          % (args.elo0, args.elo1, args.alfa, args.beta, args.pruebas))
    print('%10s %8s %11s %11s %13s %16s' % ('efecto', 'tope', 'acepta_h1', 'acepta_h0', 'SIN DECISION', 'parejas (media)'))
    rng = np.random.default_rng(args.semilla)
    for elo in args.elo:
        for tope in args.tope:
            h1, h0, sin, parejas = simular(p, elo, tope, args.elo0, args.elo1, args.alfa, args.beta, args.pruebas, rng)
            print('%+9.1f %8d %10.1f%% %10.1f%% %12.1f%% %16.0f' % (elo, tope, 100 * h1, 100 * h0, 100 * sin, parejas))


if __name__ == '__main__':
    main()
