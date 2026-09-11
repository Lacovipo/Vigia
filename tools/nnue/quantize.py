#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Cuantiza un checkpoint de train.py a un fichero de red de Vigía.

    python tools/nnue/quantize.py red.pt <directorio-corpus> [--salida nets/]

Escribe `atalaya-256-<sha8>.bin`, listo para empotrar. Las dos cotas de
desbordamiento (T1 y T2) se comprueban antes de escribir, con `netfmt`, igual
que las comprueba después el cargador en Rust.

Y mide el **error de cuantización**: sobre posiciones de validación del corpus,
compara la salida del modelo flotante con la de la red entera calculada con
`netfmt.forward_features`, la pasada hacia delante que el vector dorado ata al
motor entero a entero. Si esas dos cifras se separan, lo que se ha entrenado no
es lo que va a jugar.
"""
import argparse
import hashlib
import os
import sys

import numpy as np
import torch

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402
import netfmt as nf  # noqa: E402
from train import Atalaya, Corpus  # noqa: E402

QA = nf.QA
QB = 1 << nf.QB_LOG2


def a_enteros(estado, n_cubos):
    ft = estado['ft.weight'].cpu().numpy()[:nf.FEATURES]          # [rasgo][dimensión]
    ft_w = np.rint(ft * QA).astype(np.int64).reshape(-1)
    ft_b = np.rint(estado['ft_sesgo'].cpu().numpy() * QA).astype(np.int64)
    salida = np.zeros((nf.MAX_BUCKETS, 2 * nf.HIDDEN), dtype=np.int64)
    salida[:n_cubos] = np.rint(estado['salida'].cpu().numpy() * QB)
    sesgo = np.zeros(nf.MAX_BUCKETS, dtype=np.int64)
    sesgo[:n_cubos] = np.rint(estado['salida_sesgo'].cpu().numpy() * nf.OUTPUT_DIVISOR)
    for nombre, v, lo, hi in (('ft_w', ft_w, -32768, 32767), ('ft_b', ft_b, -32768, 32767),
                              ('salida', salida, -32768, 32767), ('sesgo', sesgo, -2**31, 2**31 - 1)):
        if v.min() < lo or v.max() > hi:
            raise SystemExit('%s se sale de su tipo entero: %d..%d' % (nombre, v.min(), v.max()))
    return ft_w.tolist(), ft_b.tolist(), salida.reshape(-1).tolist(), sesgo.tolist()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('checkpoint')
    ap.add_argument('directorios', nargs='+', help='el corpus, para medir el error de cuantización')
    ap.add_argument('--salida', default='nets')
    ap.add_argument('--muestras', type=int, default=2000)
    args = ap.parse_args()

    ck = torch.load(args.checkpoint, map_location='cpu', weights_only=False)
    n_cubos, tabla = ck['n_cubos'], ck['cubo']
    ft_w, ft_b, salida, sesgo = a_enteros(ck['modelo'], n_cubos)

    temporal = os.path.join(args.salida, 'atalaya-256-temporal.bin')
    os.makedirs(args.salida, exist_ok=True)
    sha = nf.write_net(temporal, ft_w, ft_b, salida, sesgo, tabla, n_cubos,
                       k=int(round(ck['k'])), corpus_sha=ck['sha_corpus'], positions=ck['posiciones'])
    destino = os.path.join(args.salida, 'atalaya-256-%s.bin' % sha[:8])
    os.replace(temporal, destino)
    red = nf.read_net(destino)
    print('%s  (%d cubos, K %d, epoca %d, validacion %.6f)'
          % (destino, n_cubos, round(ck['k']), ck['epoca'], ck['validacion']))

    # Error de cuantización sobre posiciones de validación.
    modelo = Atalaya(n_cubos)
    modelo.load_state_dict(ck['modelo'])
    modelo.eval()
    corpus = Corpus(args.directorios, ck['args']['validacion'])
    rng = np.random.default_rng(1)
    elegidos = np.sort(rng.choice(corpus.validacion, min(args.muestras, len(corpus.validacion)), replace=False))
    nosotros, ellos, cubo, _, _, _ = corpus.lote(elegidos, np.array(tabla, dtype=np.int64))
    with torch.no_grad():
        flotante = modelo(torch.from_numpy(nosotros.astype(np.int64)), torch.from_numpy(ellos.astype(np.int64)),
                          torch.from_numpy(cubo.astype(np.int64))).numpy()
    piezas = [int(np.sum(fila != ds.PAD)) for fila in nosotros]
    enteros = []
    for i in range(len(nosotros)):
        us = [int(f) for f in nosotros[i] if f != ds.PAD]
        them = [int(f) for f in ellos[i] if f != ds.PAD]
        # El número de piezas es el de rasgos de pieza, sin los de enroque.
        n_piezas = sum(1 for f in us if f < nf.PIECE_FEATURES)
        _, _, cp = nf.forward_features(red, us, them, n_piezas)
        enteros.append(cp)
    diferencia = np.abs(np.array(enteros) - flotante)
    del piezas
    print('error de cuantizacion sobre %d posiciones: medio %.2f cp, p99 %.1f cp, maximo %.1f cp'
          % (len(diferencia), diferencia.mean(), np.percentile(diferencia, 99), diferencia.max()))
    print('sha256 %s' % hashlib.sha256(open(destino, 'rb').read()).hexdigest())


if __name__ == '__main__':
    main()
