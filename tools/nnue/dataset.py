#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Lector del corpus que escribe `generador datos` (formato VIGIADT1).

Es la mitad Python del contrato del corpus; la otra mitad es `codificar` en
src/bin/generador.rs, donde está documentada la disposición de los 32 bytes.
Aquí se lee con numpy sin copiar (memmap) y se decodifica a lo que necesita el
entrenador: rasgos activos de cada perspectiva, puntuación, resultado y escala
de final.

Python no reimplementa ajedrez. Lo que no se puede deducir del registro sin
reglas —si el que mueve está en jaque, si la búsqueda terminó, la escala de
final— lo escribió Rust con el código del motor.
"""
import glob
import os
import struct

import numpy as np

FEATURES = 772
PIECE_FEATURES = 768
MAX_ACTIVOS = 36
# Índice de relleno para rasgos inexistentes: el entrenador lo mapea a una
# fila de ceros, así una posición con 20 piezas y una con 32 caben en la misma
# matriz de 36 columnas.
PAD = FEATURES

CABECERA = 128
MAGIA = b'VIGIADT1'
MATE_THRESHOLD = 29000   # search::MATE_THRESHOLD
LIMITE_CP = 1500         # §5.4 del plan

DTYPE = np.dtype([
    ('ocupadas', '<u8'),
    ('piezas', 'u1', (16,)),
    ('meta', 'u1'),
    ('al_paso', 'u1'),
    ('cp', '<i2'),
    ('jugada', '<u2'),
    ('resultado', 'u1'),
    ('ply', 'u1'),
])
assert DTYPE.itemsize == 32

# MoveFlag del motor, en orden de declaración (src/types.rs): Quiet,
# DoublePawnPush, KingCastle, QueenCastle, Capture, EnPassant, PromoKnight..
# PromoQueen, PromoCaptureKnight..PromoCaptureQueen.
FLAGS_CAPTURA = (4, 5, 10, 11, 12, 13)
FLAGS_PROMOCION = (6, 7, 8, 9)   # promociones sin captura: con captura ya cuentan arriba


def leer_cabecera(ruta):
    with open(ruta, 'rb') as f:
        h = f.read(CABECERA)
    if len(h) != CABECERA or h[:8] != MAGIA:
        raise ValueError('%s: no es un fichero de corpus de Vigía' % ruta)
    version, nodos = struct.unpack_from('<II', h, 8)
    semilla, = struct.unpack_from('<Q', h, 16)
    hilo, = struct.unpack_from('<I', h, 24)
    registros, = struct.unpack_from('<Q', h, 92)
    return dict(version=version, nodos=nodos, semilla=semilla, hilo=hilo,
                sha_aperturas=h[28:60].hex(), sha_generador=h[60:92].hex(),
                registros=registros)


class Fragmento:
    """Un `hilo-NN.bin` con su `hce-NN.bin`, sin cargarlos en memoria."""

    def __init__(self, ruta):
        self.ruta = ruta
        self.cabecera = leer_cabecera(ruta)
        n = self.cabecera['registros']
        tam = os.path.getsize(ruta)
        if tam != CABECERA + 32 * n:
            raise ValueError('%s: %d bytes, la cabecera promete %d registros' % (ruta, tam, n))
        self.datos = np.memmap(ruta, dtype=DTYPE, mode='r', offset=CABECERA, shape=(n,))
        ruta_hce = os.path.join(os.path.dirname(ruta), os.path.basename(ruta).replace('hilo-', 'hce-'))
        hce = np.memmap(ruta_hce, dtype=np.uint8, mode='r')
        if hce.size != 4 * n:
            raise ValueError('%s: %d bytes para %d registros' % (ruta_hce, hce.size, n))
        self.hce = hce.reshape(n, 4)

    def __len__(self):
        return len(self.datos)


def fragmentos(directorios):
    rutas = sorted(r for d in directorios for r in glob.glob(os.path.join(d, 'hilo-*.bin')))
    if not rutas:
        raise ValueError('ningún hilo-*.bin en %s' % ', '.join(directorios))
    return [Fragmento(r) for r in rutas]


def casillas_y_piezas(datos):
    """(casillas, nibbles, válido): para cada registro, las casillas ocupadas en
    orden de bit ascendente y el nibble de pieza de cada una, en matrices de 32
    columnas; `válido` marca las columnas que existen."""
    n = len(datos)
    ocupadas = np.ascontiguousarray(datos['ocupadas']).astype('<u8', copy=False)
    bits = np.unpackbits(ocupadas.view(np.uint8).reshape(n, 8), axis=1, bitorder='little')
    orden = np.cumsum(bits, axis=1, dtype=np.int16) - 1
    filas, cols = np.nonzero(bits)
    casillas = np.zeros((n, 32), dtype=np.int32)
    casillas[filas, orden[filas, cols]] = cols
    piezas = np.ascontiguousarray(datos['piezas'])
    nibbles = np.empty((n, 32), dtype=np.int32)
    nibbles[:, 0::2] = piezas & 15
    nibbles[:, 1::2] = piezas >> 4
    valido = np.arange(32)[None, :] < bits.sum(axis=1)[:, None]
    return casillas, nibbles, valido


def rasgos(datos):
    """Rasgos activos vistos por las blancas y por las negras: dos matrices
    (n, 36) de índices ordenados, rellenas con PAD. Misma definición que
    `nnue::active_features` y `netfmt.active_features`."""
    n = len(datos)
    casillas, nibbles, valido = casillas_y_piezas(datos)
    color = nibbles >> 3
    tipo = nibbles & 7
    meta = np.ascontiguousarray(datos['meta']).astype(np.int32)
    resultado = []
    for persp in (0, 1):
        piezas = (color != persp) * 384 + tipo * 64 + (casillas ^ (56 * persp))
        piezas = np.where(valido, piezas, PAD)
        enroques = np.full((n, 4), PAD, dtype=np.int32)
        # Bits 1-4 de meta: K Q k q. Dueño 0 = blancas; corto 1 = flanco de rey.
        for j, (bit, dueno, corto) in enumerate(((2, 0, 1), (4, 0, 0), (8, 1, 1), (16, 1, 0))):
            indice = PIECE_FEATURES + (2 if dueno != persp else 0) + corto
            enroques[:, j] = np.where(meta & bit != 0, indice, PAD)
        todos = np.concatenate([piezas, enroques], axis=1)
        todos.sort(axis=1)
        resultado.append(todos)
    return resultado[0], resultado[1]


def filtros(datos, hce):
    """Cada filtro de §5.4 del plan como máscara booleana: True = se descarta."""
    meta = np.ascontiguousarray(datos['meta'])
    tipo = np.ascontiguousarray(datos['jugada']) >> 12
    cp = np.ascontiguousarray(datos['cp']).astype(np.int32)
    _, nibbles, valido = casillas_y_piezas(datos)
    tipo_pieza = np.where(valido, nibbles & 7, 5)   # relleno como rey: no cuenta
    peones = (tipo_pieza == 0).sum(axis=1)
    piezas_sin_rey_ni_peon = ((tipo_pieza >= 1) & (tipo_pieza <= 4)).sum(axis=1)
    return {
        'en_jaque': meta & 64 != 0,
        'captura': np.isin(tipo, FLAGS_CAPTURA),
        'promocion': np.isin(tipo, FLAGS_PROMOCION),
        'mate': np.abs(cp) >= MATE_THRESHOLD,
        'cp_grande': (np.abs(cp) >= LIMITE_CP) & (np.abs(cp) < MATE_THRESHOLD),
        'kpk': (peones == 1) & (piezas_sin_rey_ni_peon == 0),
        'escala_cero': np.ascontiguousarray(hce[:, 0]) == 0,
        'incompleta': meta & 128 != 0,
    }


def util(datos, hce):
    """True donde el registro sobrevive a todos los filtros."""
    return ~np.logical_or.reduce(list(filtros(datos, hce).values()))
