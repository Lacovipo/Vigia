#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Lee el `parejas.jsonl` de una tanda del banco y saca lo que el resumen no da:
profundidad media por motor y cómo terminan las partidas.

    python tools/analiza_tanda.py banco/resultados/<id>

Existe por un fallo concreto, cometido dos veces. Cada partida guarda
`candidato_blancas` y una lista `jugadas`, y la tentación es atribuir las de
índice par a las blancas. Es falso: la primera jugada la hace quien mueve en
la posición del libro, y 10.920 de las 20.000 posiciones de
`vigia-20000.epd` tienen negras a mover. Atribuir por paridad mezcla los dos
motores en más de la mitad de las partidas y empuja las dos medias hacia el
promedio común — con el aspecto tranquilizador de "los dos motores llegan
igual de hondo", que es exactamente lo que uno esperaría ver si el cambio no
hiciera nada. Así quedó documentada la profundidad de 0.27 con el signo
invertido durante dos versiones.

Sin dependencias, como todo aquí.
"""
import json
import os
import sys


def analiza(directorio):
    ruta = os.path.join(directorio, 'parejas.jsonl')
    suma = {'candidato': 0.0, 'base': 0.0}
    cuenta = {'candidato': 0, 'base': 0}
    finales = {}
    parejas = 0

    with open(ruta, encoding='utf-8') as f:
        for linea in f:
            linea = linea.strip()
            if not linea:
                continue
            pareja = json.loads(linea)
            parejas += 1
            # Quién mueve en la posición de apertura, que es quien hace la
            # jugada de índice 0. Este es el dato que lo cambia todo.
            abren_blancas = pareja['fen'].split()[1] == 'w'
            for juego in pareja['juegos']:
                candidato_blancas = juego['candidato_blancas']
                finales[juego['final']] = finales.get(juego['final'], 0) + 1
                for i, jugada in enumerate(juego['jugadas']):
                    profundidad = jugada.get('d')
                    if profundidad is None:
                        continue
                    mueven_blancas = (i % 2 == 0) == abren_blancas
                    quien = 'candidato' if mueven_blancas == candidato_blancas else 'base'
                    suma[quien] += profundidad
                    cuenta[quien] += 1

    if not cuenta['candidato'] or not cuenta['base']:
        raise SystemExit('la tanda no tiene jugadas con profundidad anotada')

    media_c = suma['candidato'] / cuenta['candidato']
    media_b = suma['base'] / cuenta['base']
    total = sum(finales.values())

    print('tanda: %s' % directorio)
    print('%d parejas, %d partidas' % (parejas, total))
    print()
    print('profundidad media del candidato : %.4f  (%d jugadas)' % (media_c, cuenta['candidato']))
    print('profundidad media de la base    : %.4f  (%d jugadas)' % (media_b, cuenta['base']))
    print('diferencia                      : %+.4f plies' % (media_c - media_b))
    print()
    print('cómo terminan las partidas:')
    for final, n in sorted(finales.items(), key=lambda kv: -kv[1]):
        print('  %-24s %6d  (%.1f %%)' % (final, n, 100.0 * n / total))


if __name__ == '__main__':
    if len(sys.argv) != 2:
        raise SystemExit(__doc__)
    analiza(sys.argv[1])
