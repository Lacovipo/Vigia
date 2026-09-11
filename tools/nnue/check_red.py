#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Comprueba una red entrenada de punta a punta, y mide su escala frente a la HCE.

    python tools/nnue/check_red.py muestra <corpus> [...] --n 2000 --salida muestra.fen
    ./target/release/generador.exe evaluar --red <red.bin> --epd muestra.fen > evals.txt
    python tools/nnue/check_red.py comprobar <red.bin> evals.txt

**`muestra`** saca posiciones útiles del corpus como FEN, con semilla fija.

**`comprobar`** hace dos cosas:

1. **Rust contra Python con la red de verdad.** Para cada posición, la suma, el
   cubo y el cp que dio el motor tienen que coincidir entero a entero con
   `netfmt.forward`. Es el vector dorado de §7.1 repetido con la red entrenada,
   y cubre lo que el dorado con red aleatoria no cubre: menos de ocho cubos y
   una tabla de cubos real.
2. **La escala, criterio 3 de la fase 3 del plan.** La dispersión de la
   evaluación de la red tiene que quedar dentro de un ±15 % de la de la HCE. Si
   no, los ocho márgenes de poda calibrados para la HCE se desajustan todos a la
   vez. Se mide sobre posiciones del corpus y no sobre el libro congelado del
   banco, que se seleccionó con |eval HCE| <= 90 y comprimiría la dispersión de
   la HCE a propósito.
"""
import argparse
import os
import sys

import numpy as np

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402
import netfmt as nf  # noqa: E402

PIEZA = 'PNBRQK'


def a_fen(registro):
    """FEN de un registro del corpus, con relojes 0 1."""
    ocupadas = int(registro['ocupadas'])
    piezas = registro['piezas']
    tablero = ['.'] * 64
    k = 0
    for casilla in range(64):
        if ocupadas >> casilla & 1:
            nibble = (int(piezas[k // 2]) >> (4 * (k % 2))) & 15
            letra = PIEZA[nibble & 7]
            tablero[casilla] = letra if nibble < 8 else letra.lower()
            k += 1
    filas = []
    for fila in range(7, -1, -1):
        texto, vacias = '', 0
        for columna in range(8):
            x = tablero[fila * 8 + columna]
            if x == '.':
                vacias += 1
            else:
                texto += (str(vacias) if vacias else '') + x
                vacias = 0
        filas.append(texto + (str(vacias) if vacias else ''))
    meta = int(registro['meta'])
    enroque = ''.join(c for bit, c in ((2, 'K'), (4, 'Q'), (8, 'k'), (16, 'q')) if meta & bit) or '-'
    al_paso = int(registro['al_paso'])
    al_paso = '-' if al_paso == 64 else 'abcdefgh'[al_paso % 8] + str(al_paso // 8 + 1)
    return '%s %s %s %s 0 1' % ('/'.join(filas), 'b' if meta & 1 else 'w', enroque, al_paso)


def muestra(args):
    rng = np.random.default_rng(args.semilla)
    frags = ds.fragmentos(args.directorios)
    candidatos = []
    for i, f in enumerate(frags):
        filas = np.flatnonzero(ds.util(f.datos, f.hce))
        candidatos.append(np.stack([np.full(len(filas), i), filas], axis=1))
    candidatos = np.concatenate(candidatos)
    elegidos = candidatos[rng.choice(len(candidatos), min(args.n, len(candidatos)), replace=False)]
    with open(args.salida, 'w', encoding='utf-8', newline='\n') as salida:
        salida.write('# %d posiciones utiles del corpus, semilla %d\n' % (len(elegidos), args.semilla))
        for i, fila in elegidos:
            salida.write(a_fen(frags[int(i)].datos[int(fila)]) + '\n')
    print('%s: %d posiciones' % (args.salida, len(elegidos)))


def comprobar(args):
    red = nf.read_net(args.red)
    hce, suya, fallos, total = [], [], 0, 0
    for linea in open(args.evals, encoding='utf-8'):
        if not linea.strip() or linea.startswith('#'):
            continue
        fen, e_hce, e_red, suma, cubo, cp = linea.rstrip('\n').split('|')
        total += 1
        esperado = nf.forward(red, fen)
        obtenido = (int(suma), int(cubo), int(cp))
        if esperado != obtenido:
            fallos += 1
            if fallos <= 3:
                print('  DISCREPA %s\n    rust   %s\n    python %s' % (fen, obtenido, esperado))
        hce.append(int(e_hce))
        suya.append(int(e_red))
    print('Rust contra Python con esta red: %d posiciones, %s'
          % (total, 'IDENTICAS entero a entero' if fallos == 0 else '%d DISCREPAN' % fallos))

    hce, suya = np.array(hce, dtype=np.float64), np.array(suya, dtype=np.float64)
    # Sin las posiciones decididas, que dominarían la desviación típica.
    mascara = (np.abs(hce) < 1500) & (np.abs(suya) < 1500)
    d_hce, d_red = hce[mascara].std(), suya[mascara].std()
    razon = d_red / d_hce if d_hce > 0 else float('nan')
    print('escala sobre %d posiciones con |eval| < 1500: desviacion tipica HCE %.1f cp, red %.1f cp -> razon %.3f (%s)'
          % (mascara.sum(), d_hce, d_red, razon,
             'dentro del +-15 %' if abs(razon - 1) <= 0.15 else 'FUERA del +-15 %: corregir con una sola ganancia'))
    if len(hce) > 2:
        print('correlacion entre HCE y red: %.3f' % np.corrcoef(hce[mascara], suya[mascara])[0, 1])
    if fallos:
        sys.exit(1)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest='orden', required=True)
    m = sub.add_parser('muestra')
    m.add_argument('directorios', nargs='+')
    m.add_argument('--n', type=int, default=2000)
    m.add_argument('--semilla', type=int, default=1)
    m.add_argument('--salida', required=True)
    c = sub.add_parser('comprobar')
    c.add_argument('red')
    c.add_argument('evals')
    args = ap.parse_args()
    {'muestra': muestra, 'comprobar': comprobar}[args.orden](args)


if __name__ == '__main__':
    main()
