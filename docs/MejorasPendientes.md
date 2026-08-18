# Vigía — Mejoras pendientes

Hallazgos de revisiones externas (0.25.0) que son reales pero se han
aplazado deliberadamente, priorizados según criterio del usuario
(2026-08-08). Ver `docs/Descartados.md` para las propuestas rechazadas en
vez de aplazadas.

**Regla de oro para todo lo de aquí abajo**: no se toca sin banco de
pruebas. **El banco ya existe** (`banco`, ver `docs/BancoPruebas.md`), así
que la regla deja de ser un bloqueo y pasa a ser un procedimiento: cada
punto de esta lista se implementa solo y se valida solo, con `banco sprt`
contra la release anterior. El apartado "El prerrequisito real: medir" del
final recoge el estado y lo que el banco ya ha enseñado.

## Hecho

- **Magic bitboards** (Opus P8) — **HECHO en 0.26.0**, implementado en
  `src/magic.rs` y documentado en §3 del documento técnico.

  `banco velocidad` no genera `id` de experimento como `sprt`: no hay tanda
  de partidas que archivar, sino una medición que se repite en segundos.
  Queda aquí el comando y lo que dio, que es lo que hace falta para
  repetirla:

  ```bash
  ./target/release/banco velocidad --motor <con-magics> --contra <con-rayos> --profundidad 12
  ```

  **Decisión: aceptada.** Nodos idénticos en las 12 posiciones (el criterio
  duro: la búsqueda visita exactamente lo mismo, luego el cambio es de solo
  velocidad) y entre +12,8 % y +18,7 % de nodos/segundo en tres
  ejecuciones seguidas (media ≈ +15 %), muy por encima del ruido de ±4 %. No se pasa por
  `sprt` precisamente porque los nodos no se mueven: no hay diferencia de
  juego que medir.

  Medido en la máquina de trabajo (12 CPUs, MSVC), enfrentando los
  ejecutables congelados `../zzRelease/Vigia 0.26.exe` y
  `../zzRelease/Vigia 0.25.exe`.

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
  tiempo restante al hilo de búsqueda sin pararlo y relanzarlo).

  **Cómo validarla**: el banco tampoco pondera todavía — arbitra con
  `go`/`bestmove` y no emite `go ponder` ni `ponderhit`, así que la ganancia
  real de esta mejora sigue sin ser medible de extremo a extremo. Lo que sí
  puede medirse ya es que **no haya regresión** en juego normal (`elo0=-5,
  elo1=0`). Para medir la ganancia hay que añadir soporte de ponder al
  árbitro, que es trabajo del banco y no del motor; está listado como
  pendiente al final.

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
  donde un `Mutex` único pesa menos. Se revisita solo si el banco
  muestra pérdida de nodos/seg medible a 4–8 hilos, lo que se comprueba con
  `banco velocidad --hilos 4` y `--hilos 8` frente a `--hilos 1`; si no
  aparece, se deja como está. La carrera de generación que GPT describía, que sí era un
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

## El prerrequisito real: medir — HECHO

Los tres informes de 0.25.0 convergieron en que el punto más débil del
proyecto no era el motor sino el método de medición (Opus M8, GPT 7.3). Un
match de 16 partidas no distingue +20 Elo del ruido, de modo que ninguna de
las técnicas de arriba podía evaluarse.

**Ese bloqueo está resuelto.** El banco de pruebas es el binario `banco`
(`src/bin/banco/`), documentado en **`docs/BancoPruebas.md`**. Los cuatro
puntos que se habían identificado están cubiertos:

1. ~~SPRT sobre el harness~~ → `banco sprt`, con estadística **pentanomial**
   (la unidad es la pareja de partidas con colores invertidos, no la
   partida) y el LLR de Fishtest resuelto sin dependencias externas.
2. ~~Paralelizar el bucle de partidas~~ → `partidas.workers`, con la
   decisión evaluada sobre el prefijo contiguo para que el paralelismo no
   altere ni el resultado ni el punto de parada. Verificado: 1 y 4 workers
   dan partidas idénticas.
3. ~~Libro de aperturas más ancho~~ → `banco/libros/vigia-256.epd`, 256
   posiciones equilibradas y deduplicadas, generadas con `banco libro` a
   partir de un libro Polyglot y reproducibles desde su semilla.
4. ~~Salida PGN~~ → `partidas.pgn` en SAN, con puntuación y profundidad por
   jugada, para clasificar las derrotas en vez de solo contarlas.

Y dos cosas que no estaban en la lista y aparecieron al construirlo:

5. `banco velocidad` — para cambios de **solo velocidad** (`MovePicker`,
   legalidad por clavadas), que se validan comprobando que los nodos no
   cambian y los nodos/segundo suben. Segundos en lugar de horas. Ya tiene
   un caso real: los magic bitboards de la sección "Hecho" salieron por
   aquí.
6. `banco humo` — la verificación de que el propio banco no miente:
   enfrentando un binario determinista contra sí mismo, **todas** las
   parejas tienen que quedar en tablas exactas.

### Lo que la primera medición ya ha enseñado

- **La release 0.24 no respeta `go nodes`** (se planta en el 17 % de lo
  pedido). Medida así contra un binario que sí los respeta, el banco reporta
  +422 Elo que no significan nada. El banco lo detecta ahora y aborta; hay
  que releer con cuidado cualquier comparación histórica hecha por nodos.
- **La cifra de "+77 Elo" de 0.25 frente a 0.24 no se reproduce.** A
  `movetime` 100 ms y 128 parejas: −17,7 Elo con IC 95 % de −53,7 a +18,0,
  sin decisión. Y `banco velocidad` explica parte del porqué: 0.25-dev es un
  ~16 % más lento en nodos/segundo que 0.24. Detalle en
  `docs/Documentacion_tecnica.md` §8.6.

### Trabajo pendiente del propio banco

Por orden de utilidad para lo que viene:

1. ~~**Ampliar el libro** por encima de 256 posiciones~~ → **hecho**:
   `banco/libros/vigia-2000.epd`, 2.000 posiciones muestreadas de
   `C:/Ajedrez/Probon_Gem/apertura.txt` (3.825.105 posiciones, todas
   distintas; 3.082.412 pasan los filtros). La fuente del libro viejo,
   `C:/JC/Books/gm2001.epd`, ya no existe en el disco.
   `banco/configs/plantilla.json` apunta ya al nuevo.

   **Corrección**: al cerrarlo escribí aquí que 2.000 posiciones ponían las
   diferencias de +2–3 Elo al alcance. Es falso por un factor de ocho. Medido
   sobre las 1.000 parejas de `027-tt-quiescencia`, 1.000 parejas dan ±12,3
   Elo, y como el intervalo va con la raíz, ±3 Elo pide ~16.900 parejas. El
   libro es además un techo duro, porque no caben más parejas que posiciones
   únicas: con 2.000 no se baja de ±8,7 Elo por mucho tiempo que se le dé.
   Tabla completa en §7 de `docs/BancoPruebas.md`.

1bis. **Libro de 20.000 posiciones**, que es lo que este techo pide de
   verdad. La fuente tiene 3,8 millones y generar el libro cuesta 17
   segundos, así que el trabajo no es hacerlo sino asumir las horas de
   partidas que habilita.
2. **Repetir 0.25 vs 0.24 a 300 y 800 ms** para cerrar la pregunta abierta,
   ya con el libro ancho.
3. **Soporte de ponder en el árbitro** (`go ponder` / `ponderhit`), sin el
   cual la mejora de ponder de prioridad alta no es medible.
4. Recolectar una suite EPD externa (WAC, ECM…) para `banco epd`.

### Recursos de cómputo

Sigue vigente: el usuario pone la máquina, pero suele tener otras cosas
corriendo. **Antes de lanzar cualquier batería hay que decirle explícitamente
cuántas CPUs se necesitan** para que libere sitio. `partidas.workers` es el
número de partidas simultáneas y cada una ocupa una CPU; el comando lo
imprime al arrancar. La máquina actual tiene 12.

### Cómo se usa esto a partir de ahora

Ponder real, Syzygy, `MovePicker`, eval MG/EG y el resto de la lista dejan de
ser apuestas. El procedimiento está en `docs/BancoPruebas.md` §10, y se
resume en: **una mejora cada vez**, `banco sprt` contra la release anterior
con `elo0=0, elo1=5`, y aceptar la respuesta que salga —incluido el
`acepta_h0`, que significa "no llega a +5 Elo" y no "empeora".
