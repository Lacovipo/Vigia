# Vigía — Propuestas descartadas

Hallazgos de revisiones externas que describen algo real pero cuya
propuesta concreta se ha rechazado, con el motivo. Extraído de
`docs/Documentacion_tecnica.md` §11.1 (0.25.0). Ver `docs/MejorasPendientes.md`
para lo aplazado en vez de descartado.

Ninguno de estos es un desacuerdo sobre los hechos: todos describen algo
real. La discrepancia es de alcance o de método.

- **Gemini 2.2, la corrección concreta propuesta** (exigir que el bando
  defensor tenga a lo sumo 1 peón para aplicar la escala sin peones). El
  problema que describe —la discontinuidad al pasar de K+A vs K+3P a K+A vs
  K+4P— es real y está arreglado, pero con otra regla: la escala solo se
  aplica si el bando favorecido por la evaluación cruda *es* el que no puede
  ganar. La propuesta de Gemini desactivaría además la regla para K+A vs
  K+2P, que la calibración de 0.24.0 midió como familia entablada frente a
  cuatro motores oráculo; se perdería la corrección que motivó la regla
  para arreglar un caso que la regla del signo ya cubre con más precisión.

- **GPT P2-16, responder `bestmove 0000`** cuando `searchmoves` no nombra
  ninguna jugada legal. La inconsistencia que señala está arreglada (el
  parser ya no fabrica un `Some(vec![])` que la búsqueda descarta en
  silencio), pero la salida elegida es ignorar la cláusula entera. Entre
  incumplir una petición malformada y perder una jugada jugable en una
  posición que tiene muchas, lo segundo es peor: un token mal escrito de
  una GUI no debe costar la partida.

- **Las estimaciones de Elo de Gemini** (+20/+35 por LMR con historia,
  +30/+60 por TT lock-free, etc.). No son verificables aquí y no se han
  usado para priorizar. Con 16 partidas de harness no se distingue +20 Elo
  del ruido (ver `docs/MejorasPendientes.md` §El prerrequisito real: medir).

- **GPT P2-15, que la raíz refleje un derecho de tablas ya existente.**
  El motor debe devolver una jugada legal igualmente, y el score de raíz
  solo se reporta. El coste (comprobar reloj, repetición y material en la
  raíz) no compra ninguna decisión distinta. Se deja como está.
