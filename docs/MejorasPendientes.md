# Vigía — Mejoras pendientes

Hallazgos de revisiones externas (0.25.0) que son reales pero se han
aplazado deliberadamente, priorizados según criterio del usuario
(2026-08-08). Ver `docs/Descartados.md` para las propuestas rechazadas en
vez de aplazadas.

**Regla de oro para todo lo de aquí abajo**: no se toca sin banco de
pruebas. Ver "El prerrequisito real: medir" al final — es el paso 0 de
0.26 y condiciona el orden real de implementación, no la prioridad
declarada de cada punto.

## Prioridad alta

- **Ponder con presupuesto real** (GPT P1-11, Opus M6). Hoy `go ponder`
  corre con el presupuesto normal y, si termina antes, espera; correcto
  pero desaprovecha tiempo gratis del rival.

  **Diseño acordado**: `go ponder` lanza una búsqueda **infinita**, con un
  flag interno que marca que se está ponderando. Si llega `stop` con ese
  flag activo, se corta y se pasa a búsqueda normal con presupuesto nuevo
  (comportamiento actual de un `stop` cualquiera). Si llega `ponderhit`, se
  calcula el presupuesto real de la jugada y se le resta el tiempo ya
  gastado ponderando; si el resultado es ≤ 0, se mueve de inmediato con lo
  que ya se tiene. Falta afinar detalles (qué pasa si la búsqueda infinita
  ya alcanzó profundidad máxima antes del `ponderhit`, cómo se reporta el
  tiempo restante al hilo de búsqueda sin pararlo y relanzarlo). No es
  medible con el harness actual, que usa `movetime` fijo y nunca pondera;
  depende del banco de pruebas.

- **Syzygy** (Gemini 4.5, GPT 6.5). El usuario ya dispone de EGTBs hasta 6
  piezas (como la mayoría de referencias con las que se calibra). Se
  considera relativamente fácil de integrar con Pyrrhic (biblioteca en C,
  de cabecera única, la que usan Stockfish/Ethereal/etc. — "phatom" en la
  nota original del usuario es Pyrrhic) y da Elo gratis sin tocar
  búsqueda/eval. Sube de aplazado sin fecha a candidato real para 0.26,
  después del banco de pruebas.

  **Efecto sobre el oráculo KPK** (§5 del documento técnico): con Syzygy
  disponible, el oráculo interno de K+P vs K deja de aportar cobertura que
  Syzygy no dé ya. Se mantiene como *fallback* barato para cuando las
  tablas no están montadas (no todo el mundo las tiene), pero deja de ser
  el camino principal para finales simples una vez Syzygy esté integrado.

- **Magic bitboards** (Opus P8). Sustituye los rayos clásicos de
  `movegen.rs` (§3 del documento técnico). Es velocidad pura sin cambiar el
  comportamiento del motor, por eso no compite con las mejoras de fuerza
  que sí necesitan validarse una a una: es una optimización de motor, no de
  juego, y el propio perft/tests de consistencia existentes ya la
  verifican por construcción.

## Prioridad media — velocidad, evaluar coste/beneficio

- **`MovePicker` por etapas** (Opus P4, GPT 6.2) y **legalidad por
  clavadas en vez de make/unmake por jugada** (Opus P1). Ninguna de las
  dos es una técnica que el usuario haya implementado antes en sus propios
  motores, pero ambas vinieron de revisiones de Opus y GPT a máxima
  potencia de razonamiento, y el objetivo declarado es velocidad, que
  siempre suma. Quedan como reestructuraciones de varios días — se
  abordan cuando haya hueco, no son bloqueantes de nada.

- **TT sin `Mutex` global** (Gemini 4.4, GPT P2-03, Opus). Cambio de diseño
  real (clusters atómicos o *sharding*). Su valor depende de cuánta
  contención hay realmente con el número de hilos que se va a usar en la
  práctica: el usuario prueba siempre con 1 hilo, como mucho 2/4/8 — nadie
  prueba con más de 8. Por tanto **no es prioritario subir `MAX_THREADS`
  por encima de 16**, y el caso de uso real (pocos hilos) es precisamente
  donde un `Mutex` único pesa menos. Se revisita solo si el banco de
  pruebas muestra pérdida de nodos/seg medible a 4–8 hilos; si no, se deja
  como está. La carrera de generación que GPT describía, que sí era un
  bug, ya está arreglada (§6 del documento técnico).

## Prioridad baja / exploratorio — validar una vez haya banco de pruebas

- **Clavadas en SEE** (GPT P1-03, `4k3/4n3/8/3p4/8/8/Q7/4R1K1 w`, donde el
  recapturador `Ne7` está clavado). Es real, pero la ganancia estimada es
  mínima y el coste en el camino caliente (SEE se llama por captura en
  ordenación y en quiescencia) probablemente no compensa. Se deja para
  cuando haya banco de pruebas que permita confirmarlo con certeza en vez
  de intuirlo.

- **Persistir la historia de corrección entre llamadas `go`** (GPT P1-09,
  segunda mitad). El `Context` es por hilo y por búsqueda; persistirla
  exige una estructura compartida con envejecimiento, como la TT.
  Interesante para más adelante, a validar en el banco de pruebas antes de
  comprometerse.

- **Límite a las evasiones de jaque en quiescencia** (Opus, menor). El
  crecimiento ya está acotado por `MAX_PLY`. Se puede probar más adelante
  si el banco de pruebas sugiere que aporta algo.

- **Eval MG/EG completa** (Opus M1, GPT 6.4) y mejoras de evaluación en
  general. Siempre hay margen, pero cada término se valida por separado
  contra el banco de pruebas — nunca varios a la vez, para poder atribuir
  el resultado.

- **Técnicas de fuerza adicionales**: LMR sensible a la historia (Gemini
  4.1, Opus M4), poda por SEE de tranquilas (Gemini 4.2, Opus M4), *capture
  history* (Gemini 4.3, GPT 6.2), sonda de TT en quiescencia, ProbCut,
  multicut, gestión de tiempo adaptativa (Opus M5). Todas plausibles,
  todas importantes a medio plazo — se prueban de una en una, cada una
  contra el banco de pruebas, nunca en bloque.

## Fuera de planificación (no aplazado — descartado por ahora, con revisión futura)

- **NNUE**. El criterio del usuario: hace falta primero un motor HCE de
  3000+ CCRL antes de plantear NNUE. No se reconsidera hasta llegar ahí.

## El prerrequisito real: medir

Los tres informes de 0.25.0 convergen en que el punto más débil del
proyecto no es el motor sino el método de medición (Opus M8, GPT 7.3). Un
match de 16 partidas no distingue +20 Elo del ruido, de modo que ninguna de
las técnicas de arriba puede evaluarse hoy. El orden correcto para 0.26 es:

1. SPRT sobre el harness (`elo0=0, elo1=5, alpha=beta=0.05`).
2. Paralelizar el bucle de partidas.
3. Libro de aperturas más ancho (100–200 líneas; el polyglot ya presente
   puede servir si se valida).
4. Salida PGN para poder clasificar las derrotas.

**Recursos de cómputo**: el usuario pone la máquina a disposición para las
tandas de partidas, pero suele tener otras cosas corriendo — antes de
lanzar cualquier batería hay que decirle explícitamente cuántas CPUs se
necesitan para que libere sitio.

Con el banco de pruebas montado, ponder real, Syzygy, `MovePicker`, eval
MG/EG y el resto de la lista de arriba pasan a ser cambios evaluables uno a
uno en vez de apuestas.
