#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Entrena Atalaya-256 sobre el corpus de `generador datos` (fase 3 del plan).

    python tools/nnue/train.py <directorio> [...] --k K --salida red.pt
                               [--epocas 30] [--lote 16384] [--lr 1e-3]

**El modelo en coma flotante es el motor, sin redondear.** Cada magnitud está
elegida para que `quantize.py` no tenga que hacer nada más que multiplicar y
redondear:

- El acumulador vive en "unidades de activación": 1,0 flotante son 127 enteros
  (QA). La activación es `clamp(acc, 0, 1)²`, que en enteros es `(v·v) >> 4`.
- La salida es **directamente centipeones**. En enteros, el peso de salida es
  `w·32` (QB) y el sesgo `b·32.258`, y el motor divide la suma por 32.258: la
  cuenta cierra sola.

**Los recortes son las cotas T1 y T2 del plan, garantizadas por construcción:**

- T1, acumulador: `|w_ft| ≤ 6`, o sea 762 enteros. 36 rasgos activos suman
  como mucho 27.432, dentro de i16 con margen para el sesgo.
- T2, salida: `|w_out| ≤ 127` cp. Aunque las 512 activaciones valgan el máximo,
  la suma cabe en i32.

La pérdida compara `σ(pred/K)` con `σ(cp/K)` (§5.6 del plan), con la escala de
final del motor **dentro** de la pasada hacia delante: la red se entrena sabiendo
que la van a amortiguar. Con λ = 1, peso cero al resultado de la partida.

La validación separa **partidas enteras**, no posiciones sueltas: las posiciones
de una misma partida están correlacionadas (§5.5), y mezclarlas daría una
validación optimista.
"""
import argparse
import hashlib
import os
import queue
import sys
import threading
import time

import numpy as np
import torch
import torch.nn as nn

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import dataset as ds  # noqa: E402
from corpus_stats import cubos_con_regla  # noqa: E402

HIDDEN = 256
CLIP_FT = 6.0
CLIP_OUT = 127.0
CLIP_SESGO_SALIDA = 2000.0
TROZO = 500_000


class Atalaya(nn.Module):
    def __init__(self, n_cubos):
        super().__init__()
        # Una fila más para el relleno PAD, que no suma nada.
        self.ft = nn.EmbeddingBag(ds.FEATURES + 1, HIDDEN, mode='sum', padding_idx=ds.PAD)
        self.ft_sesgo = nn.Parameter(torch.zeros(HIDDEN))
        self.salida = nn.Parameter(torch.zeros(n_cubos, 2 * HIDDEN))
        self.salida_sesgo = nn.Parameter(torch.zeros(n_cubos))
        with torch.no_grad():
            # Unos 30 rasgos activos: con desviación 0,05 el acumulador ronda
            # 0,27 y alrededor de la mitad de las neuronas empieza activa.
            self.ft.weight.normal_(0.0, 0.05)
            self.ft.weight[ds.PAD].zero_()
            self.ft_sesgo.fill_(0.25)
            self.salida.normal_(0.0, 1.0)

    def forward(self, nosotros, ellos, cubo):
        a = (self.ft(nosotros) + self.ft_sesgo).clamp(0.0, 1.0)
        b = (self.ft(ellos) + self.ft_sesgo).clamp(0.0, 1.0)
        t = torch.cat([a, b], dim=1) ** 2
        return (t * self.salida[cubo]).sum(dim=1) + self.salida_sesgo[cubo]

    @torch.no_grad()
    def recortar(self):
        self.ft.weight.clamp_(-CLIP_FT, CLIP_FT)
        self.ft.weight[ds.PAD].zero_()
        self.ft_sesgo.clamp_(-CLIP_FT, CLIP_FT)
        self.salida.clamp_(-CLIP_OUT, CLIP_OUT)
        self.salida_sesgo.clamp_(-CLIP_SESGO_SALIDA, CLIP_SESGO_SALIDA)


class Corpus:
    """Índices globales (fragmento << 32 | fila) de los registros útiles,
    repartidos en entrenamiento y validación por partidas enteras."""

    def __init__(self, directorios, fraccion_validacion):
        self.frags = ds.fragmentos(directorios)
        entrenamiento, validacion = [], []
        ocupacion = np.zeros(33, dtype=np.int64)
        h = hashlib.sha256()
        for i, f in enumerate(self.frags):
            with open(f.ruta, 'rb') as fich:
                h.update(fich.read(ds.CABECERA))
            n = len(f)
            util = np.zeros(n, dtype=bool)
            for a in range(0, n, TROZO):
                util[a:a + TROZO] = ds.util(f.datos[a:a + TROZO], f.hce[a:a + TROZO])
            partida = np.cumsum(np.ascontiguousarray(f.datos['ply']) == 0) - 1
            # Un hash de la partida decide su lado, estable entre ejecuciones.
            es_validacion = ((partida * 2654435761 + i * 40503) % 10_000) < fraccion_validacion * 10_000
            filas = np.flatnonzero(util).astype(np.int64)
            globales = (np.int64(i) << 32) | filas
            entrenamiento.append(globales[~es_validacion[filas]])
            validacion.append(globales[es_validacion[filas]])
            ocupacion += np.bincount(np.ascontiguousarray(f.hce[:, 1])[util], minlength=33)[:33]
        self.entrenamiento = np.concatenate(entrenamiento)
        self.validacion = np.concatenate(validacion)
        self.ocupacion = ocupacion
        # La cabecera de cada fragmento lleva semilla, nodos, número de
        # registros y los sha del generador y de las aperturas: su hash
        # identifica el corpus sin leer gigas.
        self.sha = h.digest()

    def lote(self, indices, cubo_de):
        partes = []
        for i in np.unique(indices >> 32):
            filas = np.sort(indices[(indices >> 32) == i] & 0xFFFFFFFF)
            f = self.frags[int(i)]
            datos = f.datos[filas]
            hce = np.asarray(f.hce[filas])
            blancas, negras = ds.rasgos(datos)
            negras_mueven = (np.ascontiguousarray(datos['meta']) & 1).astype(bool)[:, None]
            partes.append((
                np.where(negras_mueven, negras, blancas),
                np.where(negras_mueven, blancas, negras),
                cubo_de[hce[:, 1]],
                hce[:, 0].astype(np.float32) / 64.0,
                np.ascontiguousarray(datos['cp']).astype(np.float32),
                np.ascontiguousarray(datos['resultado']).astype(np.float32) / 2.0,
            ))
        return [np.concatenate(columna) for columna in zip(*partes)]


def a_tensores(lote, dispositivo):
    nosotros, ellos, cubo, escala, cp, resultado = lote
    return (torch.from_numpy(nosotros.astype(np.int64)).to(dispositivo, non_blocking=True),
            torch.from_numpy(ellos.astype(np.int64)).to(dispositivo, non_blocking=True),
            torch.from_numpy(cubo.astype(np.int64)).to(dispositivo, non_blocking=True),
            torch.from_numpy(escala).to(dispositivo, non_blocking=True),
            torch.from_numpy(cp).to(dispositivo, non_blocking=True),
            torch.from_numpy(resultado).to(dispositivo, non_blocking=True))


def perdida(modelo, tensores, k, lam):
    nosotros, ellos, cubo, escala, cp, resultado = tensores
    prediccion = modelo(nosotros, ellos, cubo) * escala
    objetivo = lam * torch.sigmoid(cp / k) + (1.0 - lam) * resultado
    return ((torch.sigmoid(prediccion / k) - objetivo) ** 2).mean()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument('directorios', nargs='+')
    ap.add_argument('--k', type=float, required=True, help='escala de la sigmoide, de fit_k.py')
    ap.add_argument('--salida', required=True, help='checkpoint .pt')
    ap.add_argument('--epocas', type=int, default=30)
    ap.add_argument('--lote', type=int, default=16384)
    ap.add_argument('--lr', type=float, default=1e-3)
    ap.add_argument('--lam', type=float, default=1.0, help='peso de la puntuación frente al resultado')
    ap.add_argument('--validacion', type=float, default=0.02, help='fracción de partidas')
    ap.add_argument('--umbral-cubo', type=int, default=1_000_000)
    ap.add_argument('--semilla', type=int, default=1)
    args = ap.parse_args()

    torch.manual_seed(args.semilla)
    rng = np.random.default_rng(args.semilla)
    dispositivo = 'cuda' if torch.cuda.is_available() else 'cpu'

    inicio = time.time()
    corpus = Corpus(args.directorios, args.validacion)
    tabla, n_cubos = cubos_con_regla(corpus.ocupacion, args.umbral_cubo)
    cubo_de = np.array(tabla, dtype=np.int64)
    print('%d fragmentos; %d posiciones de entrenamiento y %d de validacion; %d cubos %s; K = %.1f; %s (%.0f s)'
          % (len(corpus.frags), len(corpus.entrenamiento), len(corpus.validacion), n_cubos, tabla, args.k,
             dispositivo, time.time() - inicio))

    modelo = Atalaya(n_cubos).to(dispositivo)
    optimizador = torch.optim.Adam(modelo.parameters(), lr=args.lr)
    pasos_epoca = max(1, len(corpus.entrenamiento) // args.lote)
    planificador = torch.optim.lr_scheduler.CosineAnnealingLR(optimizador, T_max=args.epocas * pasos_epoca)

    muestra_val = corpus.validacion
    if len(muestra_val) > 200_000:
        muestra_val = np.sort(rng.choice(muestra_val, 200_000, replace=False))
    val_tensores = [a_tensores(corpus.lote(muestra_val[a:a + args.lote], cubo_de), dispositivo)
                    for a in range(0, len(muestra_val), args.lote)]

    def validar():
        modelo.eval()
        with torch.no_grad():
            total = sum(perdida(modelo, t, args.k, args.lam).item() * len(t[0]) for t in val_tensores)
        modelo.train()
        return total / max(1, sum(len(t[0]) for t in val_tensores))

    mejor = float('inf')
    for epoca in range(args.epocas):
        orden = rng.permutation(corpus.entrenamiento)
        cola = queue.Queue(maxsize=8)

        def productor():
            # Una excepción aquí no puede quedarse en este hilo: el principal se
            # quedaría esperando un lote que no llega, o terminaría la época en
            # silencio con la mitad de los datos. Viaja por la cola y se relanza.
            try:
                for paso in range(pasos_epoca):
                    cola.put(corpus.lote(orden[paso * args.lote:(paso + 1) * args.lote], cubo_de))
            except BaseException as error:
                cola.put(error)
                return
            cola.put(None)

        hilo = threading.Thread(target=productor, daemon=True)
        hilo.start()
        suma, n, t0 = 0.0, 0, time.time()
        while (lote := cola.get()) is not None:
            if isinstance(lote, BaseException):
                raise lote
            p = perdida(modelo, a_tensores(lote, dispositivo), args.k, args.lam)
            optimizador.zero_grad(set_to_none=True)
            p.backward()
            optimizador.step()
            planificador.step()
            modelo.recortar()
            suma += p.item()
            n += 1
        hilo.join()
        val = validar()
        print('epoca %2d  entrenamiento %.6f  validacion %.6f  lr %.2e  (%.0f s)'
              % (epoca + 1, suma / max(n, 1), val, planificador.get_last_lr()[0], time.time() - t0))
        if val < mejor:
            mejor = val
            torch.save({
                'modelo': modelo.state_dict(), 'n_cubos': n_cubos, 'cubo': tabla, 'k': args.k,
                'sha_corpus': corpus.sha, 'posiciones': int(len(corpus.entrenamiento)),
                'epoca': epoca + 1, 'validacion': val, 'args': vars(args),
            }, args.salida)
    print('mejor validacion %.6f -> %s' % (mejor, args.salida))


if __name__ == '__main__':
    main()
