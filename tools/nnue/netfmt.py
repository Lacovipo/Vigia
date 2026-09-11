# -*- coding: utf-8 -*-
"""Formato del fichero de red de Vigía (Atalaya-256) y las cuentas enteras que
el motor hace con él.

Este módulo es la mitad Python de un contrato cuya otra mitad es
`src/nnue.rs`. Cada constante de aquí tiene allí su gemela, y el vector dorado
(`golden.py` y su test en Rust) existe para que una discrepancia entre las dos
mitades no pueda pasar desapercibida: un índice de rasgo distinto entrena una
red perfectamente buena sobre un espacio de entrada permutado, y el único
síntoma es que el motor juega peor.

Python no reimplementa ajedrez aquí. Leer la colocación de una FEN es leer
texto; las FEN que se usan como vector dorado las escribe Vigía con `to_fen`,
así que ya vienen con los derechos de enroque saneados por el propio motor.

Sin dependencias: `struct` y `hashlib` son de la biblioteca estándar.
"""
import hashlib
import struct

# --------------------------------------------------------- la arquitectura
FEATURES = 772
PIECE_FEATURES = 768
HIDDEN = 256
MAX_BUCKETS = 8
QA = 127
QB_LOG2 = 5
OUTPUT_DIVISOR = 32258          # QA^2 * QB / 16
HALF_OUTPUT_DIVISOR = 16129
MAX_ACTIVE_FEATURES = 36        # 32 piezas + 4 derechos de enroque
MAX_ACTIVATION = (QA * QA) >> 4  # 1008

HEADER_LEN = 128
MAGIC = b"VIGIANN1"
# Empaquetado igual que `nnue::ARCH`: rasgos | anchura | QA | log2(QB).
ARCH = (FEATURES << 22) | (HIDDEN << 12) | (QA << 5) | QB_LOG2
WEIGHTS_LEN = FEATURES * HIDDEN * 2 + HIDDEN * 2 + MAX_BUCKETS * 2 * HIDDEN * 2 + MAX_BUCKETS * 4

WHITE, BLACK = 0, 1
PAWN, KNIGHT, BISHOP, ROOK, QUEEN, KING = range(6)
WK, WQ, BK, BQ = 0b0001, 0b0010, 0b0100, 0b1000
RIGHTS = (WK, WQ, BK, BQ)

_PIECE_OF_CHAR = {
    'P': (WHITE, PAWN), 'N': (WHITE, KNIGHT), 'B': (WHITE, BISHOP),
    'R': (WHITE, ROOK), 'Q': (WHITE, QUEEN), 'K': (WHITE, KING),
    'p': (BLACK, PAWN), 'n': (BLACK, KNIGHT), 'b': (BLACK, BISHOP),
    'r': (BLACK, ROOK), 'q': (BLACK, QUEEN), 'k': (BLACK, KING),
}
_RIGHT_OF_CHAR = {'K': WK, 'Q': WQ, 'k': BK, 'q': BQ}


# ------------------------------------------------------------- los rasgos
def piece_feature(persp, color, kind, sq):
    """Índice de rasgo de una pieza vista desde `persp` (0 blancas, 1 negras).

    La casilla se espeja verticalmente para las negras (`sq ^ 56`), y la pieza
    es "propia" o "ajena" respecto de quien mira, no blanca o negra. Así las
    dos perspectivas comparten la misma tabla de pesos.
    """
    s = sq ^ (56 * persp)
    theirs = 1 if color != persp else 0
    return theirs * 384 + kind * 64 + s


def castling_feature(persp, right):
    owner = WHITE if right & (WK | WQ) else BLACK
    kingside = 1 if right & (WK | BK) else 0
    return PIECE_FEATURES + (1 if owner != persp else 0) * 2 + kingside


def parse_fen(fen):
    """(piezas, bando que mueve, bits de enroque). Piezas: (color, tipo, casilla)."""
    fields = fen.split()
    placement, stm, castling = fields[0], fields[1], fields[2]
    pieces = []
    rank, file = 7, 0
    for ch in placement:
        if ch == '/':
            rank -= 1
            file = 0
        elif ch.isdigit():
            file += int(ch)
        else:
            color, kind = _PIECE_OF_CHAR[ch]
            pieces.append((color, kind, rank * 8 + file))
            file += 1
    side = WHITE if stm == 'w' else BLACK
    bits = 0
    if castling != '-':
        for ch in castling:
            bits |= _RIGHT_OF_CHAR[ch]
    return pieces, side, bits


def active_features(fen, persp):
    pieces, _, bits = parse_fen(fen)
    features = [piece_feature(persp, c, k, s) for (c, k, s) in pieces]
    features += [castling_feature(persp, r) for r in RIGHTS if bits & r]
    return sorted(features)


# ---------------------------------------------------------- el fichero
def _weights_bytes(ft_w, ft_b, out_w, out_b):
    assert len(ft_w) == FEATURES * HIDDEN, len(ft_w)
    assert len(ft_b) == HIDDEN, len(ft_b)
    assert len(out_w) == MAX_BUCKETS * 2 * HIDDEN, len(out_w)
    assert len(out_b) == MAX_BUCKETS, len(out_b)
    data = (struct.pack('<%dh' % len(ft_w), *ft_w)
            + struct.pack('<%dh' % len(ft_b), *ft_b)
            + struct.pack('<%dh' % len(out_w), *out_w)
            + struct.pack('<%di' % len(out_b), *out_b))
    assert len(data) == WEIGHTS_LEN, len(data)
    return data


def write_net(path, ft_w, ft_b, out_w, out_b, bucket_of, n_buckets,
              k=0, corpus_sha=bytes(32), positions=0):
    assert 1 <= n_buckets <= MAX_BUCKETS
    assert len(bucket_of) == 33 and all(0 <= b < n_buckets for b in bucket_of)
    assert len(corpus_sha) == 32
    net = dict(ft_w=list(ft_w), ft_b=list(ft_b), out_w=list(out_w), out_b=list(out_b),
               bucket_of=list(bucket_of), n_buckets=n_buckets, k=k)
    check_bounds(net)
    weights = _weights_bytes(ft_w, ft_b, out_w, out_b)
    header = (MAGIC
              + struct.pack('<IHB', ARCH, k, n_buckets)
              + bytes(bucket_of)
              + corpus_sha
              + struct.pack('<Q', positions)
              + hashlib.sha256(weights).digest()
              + bytes(8))
    assert len(header) == HEADER_LEN, len(header)
    with open(path, 'wb') as f:
        f.write(header + weights)
    return hashlib.sha256(header + weights).hexdigest()


def read_net(path):
    with open(path, 'rb') as f:
        data = f.read()
    assert len(data) == HEADER_LEN + WEIGHTS_LEN, len(data)
    header, weights = data[:HEADER_LEN], data[HEADER_LEN:]
    assert header[:8] == MAGIC
    arch, k, n_buckets = struct.unpack('<IHB', header[8:15])
    assert arch == ARCH, hex(arch)
    assert hashlib.sha256(weights).digest() == header[88:120], 'sha de los pesos'
    off = 0
    n = FEATURES * HIDDEN
    ft_w = list(struct.unpack_from('<%dh' % n, weights, off)); off += 2 * n
    ft_b = list(struct.unpack_from('<%dh' % HIDDEN, weights, off)); off += 2 * HIDDEN
    n = MAX_BUCKETS * 2 * HIDDEN
    out_w = list(struct.unpack_from('<%dh' % n, weights, off)); off += 2 * n
    out_b = list(struct.unpack_from('<%di' % MAX_BUCKETS, weights, off))
    return dict(ft_w=ft_w, ft_b=ft_b, out_w=out_w, out_b=out_b,
                bucket_of=list(header[15:48]), n_buckets=n_buckets, k=k)


# ------------------------------------------------------ las dos cotas
def check_bounds(net):
    """T1 y T2 de §3.5 del plan, las mismas que comprueba el cargador en Rust."""
    ft_w, ft_b = net['ft_w'], net['ft_b']
    for dim in range(HIDDEN):
        column = sorted(ft_w[f * HIDDEN + dim] for f in range(FEATURES))
        top = sum(w for w in column[-MAX_ACTIVE_FEATURES:] if w > 0)
        bottom = sum(w for w in column[:MAX_ACTIVE_FEATURES] if w < 0)
        assert ft_b[dim] + top <= 32767, 'T1: la dimension %d puede desbordar hacia arriba' % dim
        assert ft_b[dim] + bottom >= -32768, 'T1: la dimension %d puede desbordar hacia abajo' % dim
    for b in range(net['n_buckets']):
        row = net['out_w'][b * 2 * HIDDEN:(b + 1) * 2 * HIDDEN]
        worst = sum(abs(w) for w in row) * MAX_ACTIVATION + abs(net['out_b'][b]) + HALF_OUTPUT_DIVISOR
        assert worst <= 2**31 - 1, 'T2: el cubo %d puede desbordar la salida' % b


# ------------------------------------------------ la pasada hacia delante
def divide_rounding(s):
    """Redondeo a mitad alejándose de cero, y por división: `>>` redondearía
    hacia -inf y rompería eval(p) == -eval(p espejada)."""
    if s >= 0:
        return (s + HALF_OUTPUT_DIVISOR) // OUTPUT_DIVISOR
    return -((-s + HALF_OUTPUT_DIVISOR) // OUTPUT_DIVISOR)


def accumulator(net, fen, persp):
    acc = list(net['ft_b'])
    for f in active_features(fen, persp):
        row = net['ft_w'][f * HIDDEN:(f + 1) * HIDDEN]
        acc = [a + w for a, w in zip(acc, row)]
    assert all(-32768 <= a <= 32767 for a in acc), 'acumulador fuera de i16'
    return acc


def screlu(a):
    v = min(max(a, 0), QA)
    return (v * v) >> 4


def forward(net, fen):
    """(S, cubo, cp) de la red cruda, desde el bando que mueve.

    Sin prefacio KPK ni escala de final: eso es ajedrez y lo pone el motor.
    Esto es exactamente lo que el vector dorado compara entero a entero.
    """
    pieces, stm, _ = parse_fen(fen)
    us = accumulator(net, fen, stm)
    them = accumulator(net, fen, 1 - stm)
    bucket = net['bucket_of'][len(pieces)]
    row = net['out_w'][bucket * 2 * HIDDEN:(bucket + 1) * 2 * HIDDEN]
    s = net['out_b'][bucket] + sum(screlu(a) * w for a, w in zip(us + them, row))
    assert -2**31 <= s < 2**31, 'suma de salida fuera de i32'
    return s, bucket, divide_rounding(s)
