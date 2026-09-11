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

- **Legalidad por clavadas** (Opus P1) — **HECHO en 0.28.0**, en
  `src/movegen.rs`, documentado en §3 del documento técnico. Era la mitad
  del punto de prioridad media que se llamaba "`MovePicker` por etapas y
  legalidad por clavadas"; la otra mitad sigue pendiente y ha cambiado de
  valor (ver más abajo).

  **Decisión: aceptada**, por el carril de velocidad, sin gastar una sola
  partida.

  ```bash
  ./target/release/banco.exe velocidad --motor <nuevo> \
      --contra "Release/Vigia 0.27.exe" --profundidad 12 --hash 32 --hilos 1
  ```

  | pasada | orden | resultado |
  |---|---|---|
  | 1 | 0.28 contra 0.27 | +29,7 % |
  | 2 | 0.27 contra 0.28 (invertida) | +34,1 % |
  | 3 | 0.28 contra 0.27 | +32,5 % |

  **Con la máquina en reposo.** Repetidas más tarde con el equipo ocupado
  dieron +41,2 %, +13,6 %, +40,4 % y +49,9 %, y el nps absoluto de 0.27
  osciló un 30 % sin que nada cambiara: el comando mide A entero y luego B
  entero, así que una carga que aparece a mitad se cobra contra uno solo de
  los dos. La cifra que se cita es la banda de las tres primeras, no la
  media de las siete, y de ahí sale una tarea nueva para el banco (ver más
  abajo).

  Nodos **idénticos** en las 12 posiciones en las siete pasadas, que es el
  criterio duro que autoriza a saltarse `sprt`, y en las siete 0.28 salió
  más rápido: la dirección nunca estuvo en duda, solo la magnitud.

  **Medida después también como fuerza**, aunque el carril de velocidad no
  lo exigía: **+72,4 Elo, IC 95 % [+67,3, +77,6]** sobre 12.144 partidas
  (`028-velocidad-en-elo-estimacion`), con +0,493 plies de profundidad
  media. Es la mayor ganancia medida del proyecto. Se hizo porque contestaba
  además una pregunta abierta desde 0.26, y lo que contestó reordena esta
  lista entera — ver la sección siguiente. En perft puro, que es 100 %
  generación, los dos perft profundos bajan de 0,12–0,14 s a 0,03–0,06 s.

  Lo que enseñó y merece copiarse: **medir el techo antes de escribir el
  código**. Duplicando a propósito el make/unmake de legalidad, el mismo
  árbol tardaba un 27,9 % más, luego el filtro era el 27,9 % del tiempo de
  nodo y una legalidad gratuita valía como mucho +38,7 %. Con eso se
  declaró de antemano una predicción falsable (+25/+33 %) y un umbral de
  reversión. Es barato —dos compilaciones y dos minutos de una CPU— y
  convierte la medición final en una comprobación en vez de en una
  sorpresa.

- **Sonda de tabla de transposición en quiescencia** — **HECHO en 0.27.0**,
  en `src/search.rs`, documentado en §6 del documento técnico.

  **Decisión: aceptada.** Es la **primera mejora de fuerza del proyecto con
  veredicto formal del banco**.

  ```bash
  ./target/release/banco.exe sprt --config banco/configs/027-tt-quiescencia-largo.json
  ```

  | experimento | libro | parejas | cómo terminó | Elo |
  |---|---:|---:|---|---:|
  | `027-tt-quiescencia` | 2.000 | 2.000 | agotó el tope, `continuar` | +11,4 |
  | `027-tt-quiescencia-largo` | 20.000 | 1.313 | **`acepta_h1`** | +20,4 |

  **Magnitud honesta: entre +11 y +15 Elo, no +20,4.** La tanda que cruzó
  paró en el instante favorable en que cruzó, y eso sesga su cifra al alza;
  la primera, que agotó su tope sin pararse en ninguna frontera, es la
  estimación limpia. Agregando las dos (6.626 partidas) sale +15,0
  [+8,1, +21,8]. Lo que el `acepta_h1` certifica es que supera +5 Elo.

  Tres cosas que enseñó y que están en §7 de `docs/BancoPruebas.md`: que el
  control por nodos **no puede medir** esta familia de cambios, que la cifra
  de una tanda que cruzó frontera va sesgada, y cuántas parejas hace falta
  jugar de verdad para cada resolución.

  Y una que contradijo la hipótesis: la profundidad media **no sube**. La
  ganancia es acertar más a la misma profundidad, no llegar más hondo.

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

## Lo que 0.28 cambió sobre cómo se prioriza esta lista

Hasta 0.28 esta lista se ordenaba por Elo esperado. Ahora hay un número
medido que reordena casi todo (§8.7 del documento técnico):

> **2,26 Elo por cada 1 % de nodos/segundo.** 180 Elo por doblar la
> velocidad, no los 65 del folclore. Medido a 100 ms con un cambio de solo
> velocidad, que es el único experimento limpio posible para esto.

Tres consecuencias, y ninguna es menor:

1. **La velocidad pasa a ser la mejor mejora por hora de máquina, con
   diferencia.** Un 10 % de nodos/segundo vale ~+22 Elo, más que cualquier
   mejora de evaluación o de poda de esta lista. Y se valida con `banco
   velocidad` en **minutos**, exigiendo nodos idénticos, en vez de en cinco
   o seis horas de `sprt` con un desenlace probable de *sin decisión*. La
   asimetría es brutal: mejor premio, coste de validación mil veces menor y
   un criterio de aceptación que no admite discusión.

2. **Las mejoras de fuerza de +3/+5 Elo no es que sean difíciles de medir:
   es que están batidas.** Un 3 % de velocidad las iguala y se comprueba
   antes de comer. No se descartan, pero dejan de ir primero.

3. **Una evaluación cara parte con una deuda enorme.** Una NNUE que deje
   los nodos/segundo a la mitad empieza **180 Elo por debajo** antes de
   evaluar mejor una sola posición. Esto no mata NNUE, pero fija su
   presupuesto: la red tiene que ser pequeña y con acumulador incremental, o
   el trato no sale. Es la restricción de diseño más dura que tiene el
   proyecto por delante.

**La reserva, ya con un segundo punto medido**: a 300 ms el mismo contraste
da +69,1 Elo, o sea **172 Elo por doblar frente a 180 a 100 ms —
indistinguibles** (0,65 sigmas). Aquí se predijo que la curva se aplanaría
y a control de torneo el porcentaje valdría bastante menos; en ese tramo,
no ocurre. Sigue siendo una extrapolación corta —de 100 a 300 ms hay 1,6
duplicaciones y la profundidad solo va de ~10,6 a ~12,6 plies—, así que un
tercer punto a 800 ms sigue mereciendo la pena. Pero la conclusión operativa
se refuerza en vez de debilitarse: **la deuda que paga una evaluación cara
no era un artefacto del control rápido en el que se midió**.

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

- **Syzygy** (Gemini 4.5, GPT 6.5). La premisa está **comprobada en disco**
  (2026-09): `C:/Ajedrez/TB_syzygy/`, 1.014 ficheros, 150 GB, hasta 6
  piezas (359 tablas de 6). No es una suposición heredada.

  Lo que sí hay que corregir de la nota original: **Pyrrhic no vale aquí**.
  Es una biblioteca en C, y el proyecto no admite dependencias externas ni
  FFI, así que el sondeador habría que escribirlo en Rust puro desde cero
  — decodificador WDL y DTZ, tablas comprimidas, indexación por simetría.
  Del orden de 2.000 líneas de código denso, y sin implementación de
  referencia enlazada contra la que compararse: la validación tendría que
  apoyarse en posiciones de resultado conocido y en coherencia interna. Sin
  `mmap` (que también pide FFI) se leería por `std::fs` con `seek`, que es
  más lento pero perfectamente viable: un sondeo lee bloques por
  desplazamiento, no la tabla entera.

  **La medición barata que aquí se proponía ya está hecha**, sobre las
  12.144 partidas de `028-velocidad-en-elo-estimacion`:

  | cómo terminan las partidas | % |
  |---|---:|
  | abandono adjudicado | 70,0 |
  | repetición triple | 17,1 |
  | tablas adjudicadas | 6,1 |
  | material insuficiente | 3,1 |
  | cincuenta jugadas | 2,3 |
  | mate | 1,1 |
  | tope de plies / ahogado | 0,3 |

  **El 70 % de las partidas se corta por abandono adjudicado** y otro 6 %
  por tablas adjudicadas, así que la inmensa mayoría no llega nunca a un
  final donde una tabla de 6 piezas cambiaría algo. El Elo medible en este
  banco es pequeño **por construcción de la adjudicación**, no por falta de
  valor de la técnica.

  Conclusión: si algún día se hace Syzygy, no se justifica por Elo de
  autojuego y no se valida con `sprt` — se justifica por juego correcto en
  finales, y su criterio de aceptación tiene que ser otro (una suite de
  finales con resultado conocido, por ejemplo). Mientras tanto, hay una
  alternativa mucho más barata para lo mismo, abajo.

  **Efecto sobre el oráculo KPK** (§5 del documento técnico): con Syzygy
  disponible, el oráculo interno de K+P vs K deja de aportar cobertura que
  Syzygy no dé ya. Se mantiene como *fallback* barato para cuando las
  tablas no están montadas (no todo el mundo las tiene), pero deja de ser
  el camino principal para finales simples una vez Syzygy esté integrado.

- **Oráculos de finales básicos por análisis retrógrado** (candidato nuevo
  de 0.28, alternativa barata a Syzygy). `kpk.rs` ya construye su tabla de
  K+P contra K por análisis retrógrado, y la técnica se extiende a KQK, KRK
  y —con más trabajo, porque hay que arrinconar al rey en la esquina del
  color correcto— KBNK. Tablas pequeñas, autocontenidas, construidas en el
  propio binario sin depender de 150 GB en disco, y **verificables de forma
  exhaustiva** porque el dominio entero cabe en memoria.

  Da lo que Syzygy daría en los finales que de verdad aparecen, sin 2.000
  líneas de decodificador ni un fichero externo. Y mejora las etiquetas de
  final para un futuro entrenamiento de red, que es donde el conocimiento
  exacto vale doble. Igual que Syzygy, no se valida con `sprt`: se valida
  contra el dominio completo, que es una prueba mucho más fuerte.

## Prioridad media — velocidad, evaluar coste/beneficio

- **`MovePicker` por etapas** (Opus P4, GPT 6.2). La legalidad por clavadas
  que iba en este mismo punto está **hecha en 0.28.0** (ver "Hecho"), y eso
  cambia el valor de lo que queda, a la baja: el filtro de legalidad ha
  pasado del 27,9 % del tiempo de nodo a ~2 %, así que "no generar las
  tranquilas hasta que hagan falta" ya no ahorra lo que ahorraba.

  **Hay que volver a medir el reparto antes de comprometerse**, con el
  mismo truco barato que se usó para la legalidad: duplicar a propósito el
  trabajo que se quiere quitar y ver cuánto tarda de más el mismo árbol.
  Dos compilaciones y dos minutos de una CPU, frente a los varios días que
  cuesta la reestructuración.

  **Pero el listón que tiene que superar bajó mucho con 0.28**: a 2,26 Elo
  por punto porcentual de nodos/segundo, un 5 % ya vale ~+11 Elo, que es
  más de lo que promete cualquier mejora de evaluación de esta lista y se
  comprueba en minutos. Con ese cambio de escala, esto y cualquier otra
  cosa que dé velocidad vuelven a la cabeza de la cola. Aquí se escribió
  que el valor de esta mejora bajaba porque el filtro de legalidad pasó del
  27,9 % al ~2 % del tiempo de nodo; sigue siendo verdad que el pastel es
  más pequeño, pero cada trozo vale el triple de lo que se creía.

  Un aviso para cuando se aborde, porque es un fallo que este motor ya tuvo
  y arregló: `quiescence_inner` distingue hoy "no hay capturas que valga la
  pena leer" de "no hay ninguna jugada legal", y confundirlas hacía que un
  **ahogado** se puntuara con la evaluación estática. El test que lo cubre
  tiene que estar escrito *antes* de tocar la generación, no después.

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
  history* (Gemini 4.3, GPT 6.2), ~~sonda de TT en quiescencia~~ (hecha en
  0.27.0, ver arriba), ProbCut, multicut, gestión de tiempo adaptativa
  (Opus M5). Todas plausibles, todas importantes a medio plazo — se prueban
  de una en una, cada una contra el banco de pruebas, nunca en bloque.

  **Aviso de resolución, añadido en 0.28 tras analizarlas contra el
  código**: las cuatro primeras comparten un problema que esta lista no
  decía. Su efecto esperado *en este motor* ronda +3/+5 Elo, porque lo que
  ya hay solapa con ellas —la ordenación de tranquilas ya usa exactamente
  `history + cont_history`, SEE ya está *sumado* dentro de la puntuación de
  capturas y no solo como filtro, y LMP con `(3+d²)/2` ya cubre las
  profundidades donde la poda SEE de tranquilas mordería—. Con `elo0=0,
  elo1=5`, **un efecto de +4 Elo no cruza la frontera nunca**: el desenlace
  más probable de cinco o seis horas de máquina es *sin decisión*. No
  significa que no se hagan; significa que se entra sabiendo eso, y que
  entre ellas van primero las dos que admiten un prefiltro barato con
  `banco velocidad` (poda por SEE de tranquilas y ProbCut, que encogen el
  árbol de forma observable en segundos) antes que las dos que no lo
  admiten.

- **LMR indexada por tranquilas buscadas en vez de por índice en la lista**
  (hallazgo de 0.28, no venía de ninguna revisión). LMP se corrigió en su
  día para contar `quiets_tried` en lugar del índice sobre la lista
  ordenada completa, con un comentario explícito de por qué: "en una
  posición táctica con ocho capturas plausibles, la forma antigua podaba
  *todas* las tranquilas a profundidad ≤2". **LMR sigue usando
  `move_index`** (`src/search.rs`, condición `move_index >=
  LMR_FULL_DEPTH_MOVES`). En ese mismo nodo de ocho capturas, la primera
  tranquila entra con índice 8 y se lleva una reducción grande sin ser
  tardía en ningún sentido útil. Es la misma asimetría que el proyecto ya
  identificó y arregló una vez, son unas cinco líneas, y merece ir por
  delante de LMR-sensible-a-la-historia. Cambia nodos, así que se valida
  con `sprt`.

## NNUE — ya no está fuera de planificación (decisión del usuario, 2026-09)

El criterio anterior era que hacía falta un HCE de 3000+ CCRL antes de
plantearlo. **Levantado.** Pasa a ser una opción más, y con el dato de
0.28 encima de la mesa es probablemente *la* opción. Los datos salen de
Vigía, no de fuera: es condición del usuario y define el proyecto.

**Por qué no conviene pulir más la HCE antes.** Una red no se entrena
contra la evaluación estática sino contra la **puntuación de una búsqueda**
a profundidad 8-10, que contiene conocimiento táctico que la evaluación
estática no tiene. La primera red no hereda el techo de la HCE: destila
búsqueda en evaluación, y suele batir ya a la HCE de la que salieron sus
etiquetas. De ahí el corolario que decide el dilema de "¿mejoro la HCE
primero para tener mejores datos?": **mejorar la búsqueda sí mejora los
datos; mejorar la evaluación no, porque la evaluación es justo lo que se
va a jubilar.**

**El presupuesto, que es la parte dura.** A 2,26 Elo por punto porcentual
de nodos/segundo, una red que deje la velocidad a la mitad parte con ~175
Elo en contra — y eso vale igual a 100 que a 300 ms, así que no es un
artefacto del control rápido. Objetivo de diseño: **que la red no cueste más de un 30 % de
los nodos/segundo** (≈ −68 Elo de deuda), lo que obliga a red pequeña,
acumulador incremental y cuantización entera. Sin SIMD explícito, además,
porque `std::simd` es inestable y aquí no hay dependencias: la red tiene
que ser lo bastante pequeña para que Rust autovectorizado la mueva.

**Decisiones ya tomadas** (el usuario las delegó explícitamente):

- **Iterar ya, sin pulir más la HCE**, por el razonamiento de arriba.
- **Entrenador en `tools/nnue/`**, en Python, como ya vive
  `tools/calibration/`. La *generación* de datos, en cambio, en Rust dentro
  del banco: son millones de posiciones. El motor sigue con
  `[dependencies]` vacío — el entrenador no es el motor.
- **Red empotrada con `include_bytes!`**, no en fichero aparte. Dos
  razones, y manda la segunda: Vigía siempre ha sido un exe que se copia a
  un directorio y funciona, y un `.nnue` suelto es un modo de fallo nuevo
  justo en un torneo; y **la firma de todo experimento del banco incluye el
  sha del binario**, así que con la red fuera dos tandas podrían ser "el
  mismo binario" con redes distintas y se rompería en silencio la
  reproducibilidad sobre la que se apoya el banco entero.

**De Stockfish 19** conviene tomar las ideas baratas —entradas agrupadas
por casilla de rey, *clipped ReLU*, cuantización int16 en el acumulador e
int8 en los pesos, actualización incremental, cubos de salida por número de
piezas— y sobre todo su receta de filtrado de datos, que es media
victoria: descartar posiciones en jaque, descartar aquellas cuya mejor
jugada es captura, mezclar puntuación con resultado. Lo que **no** se puede
tomar es el tamaño: sus redes necesitan SIMD explícito para ir rápido.

### Estado de la NNUE

| fase | estado |
|---|---|
| 0 — congelar los libros | **hecha**: sha256 anotados en `BancoPruebas.md`, y `.gitattributes` para que un clon no los convierta a CRLF |
| 1 — cerrar el bucle con la red de material | **hecha**: los cuatro criterios cumplidos |
| 2 — generador y corpus | pendiente |
| 3 — entrenar, cuantizar y exportar | pendiente |
| 4 a 7 | pendientes |

La fase 1, criterio a criterio:

1. **Tests y clippy**: 374 tests en verde y 0 avisos.
2. **Vector dorado en los dos sentidos**: 4.096 posiciones, idéntico entero a
   entero, con los 8 cubos de salida y todos los números de piezas de 2 a 32
   representados.
3. **Coste de la ruta de evaluación**: ≈ 87 ns por nodo, por debajo de la banda
   de 92–172 del plan, del lado bueno. Medido con el método correcto y no con el
   que escribía el plan (ver §4 de `BancoPruebas.md`: restar nps entre árboles
   distintos no aísla nada). La vectorización está comprobada en el binario con
   LTO.
4. **Cumplido**: 256 parejas contra 0.28 (`028-nnue-fase1-material`, 50.000
   nodos). **−236,4 Elo, IC 95 % [−271,7, −204,9]**: el intervalo entero por
   debajo de −150, que era el criterio decidido antes de jugar. Y en 512
   partidas ni una jugada ilegal, ni una pérdida por tiempo, ni una
   desconexión: todas terminan por mate, tablas o adjudicación. A 50.000
   nodos y no a los 25.000 del plan, que disparan el guardián del banco.

   Un dato de propina: a igualdad de nodos la red de material busca **0,91
   plies más hondo** que la HCE (9,93 frente a 9,03). Su evaluación plana corta
   mucho más y su árbol es otro, que es la misma razón por la que comparar nps
   entre las dos no mide el coste de la evaluación.

Dos correcciones al plan salieron por el camino y quedan anotadas en él: el
criterio 3 no puede derivarse del nps, y el criterio 4 no puede ir a 25.000
nodos.

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
   partir de `C:/JC/Books/gm2001.epd` y reproducibles desde su semilla. Aquí
   ponía "a partir de un libro Polyglot" y era falso: lo desmiente la cabecera
   del propio libro, y `banco libro` solo lee EPD.
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

1bis. ~~**Libro de 20.000 posiciones**~~ → **hecho**:
   `banco/libros/vigia-20000.epd`, y es el que decidió la tanda larga de
   0.27. Lo que sigue vigente de este punto no es hacerlo sino asumir las
   horas de partidas que habilita.

1ter. ~~**`banco humo` no se podía ejecutar**~~ → **arreglado en 0.28**. Su
   presupuesto por defecto de 25.000 nodos caía justo por debajo del 50 %
   que exige el guardián de "no respeta el límite por nodos", y no solo con
   el binario nuevo: 0.27 se planta en los mismos 8.310 nodos. El defecto
   pasa a 50.000, medido. Detalle en §6 de `docs/BancoPruebas.md`. Merece
   quedar anotado porque es el modo de fallo típico de una herramienta de
   verificación: no avisa de que lleva tiempo sin poder correr, simplemente
   nadie la corre.

1quater. **`banco velocidad` debería intercalar A y B posición a posición**,
   en vez de medir A entero y luego B entero. Hallazgo de 0.28: repitiendo
   la misma comparación con la máquina ocupada salieron +41,2 %, +13,6 %,
   +40,4 % y +49,9 % para el mismo par de binarios, con el nps absoluto de
   la base oscilando un 30 %. Una carga que aparece a mitad de camino se
   cobra entera contra uno de los dos bandos, y alternar el orden entre
   pasadas lo *detecta* pero no lo arregla. Intercalando, una deriva lenta
   afectaría a los dos por igual. Es poco código en
   `src/bin/banco/velocidad.rs` y protege el único carril de validación que
   no cuesta partidas.

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
imprime al arrancar. La máquina es un AMD Ryzen 9 9950X: **16 núcleos
físicos, 32 hilos lógicos** (comprobado; aquí ponía 12 y era falso). Para
medir fuerza cuenta el número de físicos, porque dos partidas compartiendo
un núcleo por SMT van cada una a su ritmo y ensucian el control por tiempo.

Y el aviso vale también para lo que no son partidas: una recompilación en
`--release` con LTO y `codegen-units = 1` ocupa la máquina un buen rato.
Una batería de mutantes que recompila nueve veces seguidas hay que
lanzarla con permiso, no de fondo.

### Cómo se usa esto a partir de ahora

Ponder real, Syzygy, `MovePicker`, eval MG/EG y el resto de la lista dejan de
ser apuestas. El procedimiento está en `docs/BancoPruebas.md` §10, y se
resume en: **una mejora cada vez**, `banco sprt` contra la release anterior
con `elo0=0, elo1=5`, y aceptar la respuesta que salga —incluido el
`acepta_h0`, que significa "no llega a +5 Elo" y no "empeora".
