# Vigía — Banco de pruebas

Cómo se valida una mejora antes de darla por buena. Este documento es el
punto de entrada: si vas a tocar búsqueda, evaluación o velocidad, empieza
aquí.

El banco es el binario `banco`, que se compila con el resto del proyecto y
no tiene dependencias externas (ni Python, ni SciPy, ni cutechess). Arbitra
las partidas con las reglas del propio motor, enlazando contra la biblioteca
`vigia`.

```bash
cargo build --release
./target/release/banco.exe ayuda
```

---

## 1. Por qué existe

Hasta 0.25 la medición de fuerza eran 16 partidas con 8 aperturas. Con esa
muestra, el error típico es de ±150–200 Elo: las cifras "+394 Elo" de 0.24 y
"−89 Elo" de 0.23 son estadísticamente **el mismo dato**. Ninguna de las
técnicas de `MejorasPendientes.md` podía evaluarse así, y por eso todas
estaban bloqueadas detrás de "hace falta banco de pruebas".

El banco resuelve las cuatro carencias concretas que se habían identificado:
prueba secuencial (SPRT), paralelismo, libro ancho y salida PGN. Y añade dos
que aparecieron al construirlo: un banco de velocidad para cambios que no
deben alterar la búsqueda, y una verificación de que el propio banco no está
midiendo mal.

---

## 2. Qué herramienta usar para cada cambio

| Si el cambio… | Herramienta | Coste típico |
|---|---|---|
| cambia cómo juega (búsqueda, evaluación, tiempo) | `banco sprt` | horas |
| solo debe ir más rápido, sin cambiar decisiones | `banco velocidad` | segundos |
| podría romper un motivo táctico concreto | `banco epd` | minutos |
| es del propio banco | `banco humo` | un minuto |

Las tres primeras son complementarias, no alternativas. `velocidad` y `epd`
no sustituyen nunca a `sprt` para aprobar una mejora de fuerza: `velocidad`
solo certifica que un cambio **no** tocó la búsqueda, y `epd` es
diagnóstico.

---

## 3. `banco sprt` — medir fuerza

### 3.1 Qué hace

Enfrenta un **candidato** contra una **base** sobre un libro de aperturas,
jugando cada apertura dos veces con los colores intercambiados, y para en
cuanto la evidencia acumulada basta para decidir.

La unidad estadística es la **pareja** (las dos partidas de una misma
apertura), no la partida. Esto se llama estadística *pentanomial* y no es un
refinamiento cosmético: buena parte del ruido de "esta apertura favorecía a
las blancas" se cancela dentro de la propia pareja, y la prueba decide con
bastantes menos partidas.

El resultado de una pareja, visto desde el candidato, cae en uno de cinco
cajones:

| cajón | qué pasó | puntos |
|---|---|---|
| 0 | perdió las dos | 0 |
| 1 | perdió una, entabló la otra | ½ |
| 2 | entabló las dos, **o** ganó una y perdió la otra | 1 |
| 3 | ganó una, entabló la otra | 1½ |
| 4 | ganó las dos | 2 |

El SPRT contrasta H0 ("la diferencia es `elo0`") contra H1 ("es `elo1`") y
acumula un LLR. Cuando el LLR cruza `log((1−β)/α)` se acepta H1; cuando
cruza `log(β/(1−α))` se acepta H0; mientras tanto, se sigue jugando. El LLR
usa el modelo de máxima verosimilitud generalizado de Fishtest, resuelto por
bisección sobre la ecuación secular (por eso no hace falta SciPy).

### 3.2 Uso

```bash
./target/release/banco.exe sprt --config banco/configs/mi-experimento.json
```

Códigos de salida: `0` acepta_h1, `10` acepta_h0, `20` continuar, `1` error.

Copia `banco/configs/plantilla.json`, que lleva cada campo comentado.
Configuración mínima:

```json
{
  "esquema": 1,
  "id": "lmr-sensible-a-historia-vs-025",
  "candidato": {"nombre": "0.26-dev", "ruta": "target/release/vigia.exe"},
  "base": {"nombre": "0.25", "ruta": "../zzRelease/Vigia 0.25.exe"},
  "libro": {"fichero": "banco/libros/vigia-256.epd", "semilla": 20260818},
  "busqueda": {"nodos": 25000},
  "partidas": {"max_parejas": 256, "workers": 1},
  "sprt": {"elo0": 0.0, "elo1": 5.0, "alpha": 0.05, "beta": 0.05},
  "salida": "banco/resultados/lmr-historia"
}
```

Las rutas son relativas al directorio desde el que se lanza `banco`, que
debe ser la raíz del proyecto. Cualquier clave que empiece por `_` es un
comentario. **Una clave desconocida es un error**, no un aviso: un
`max_parejs` mal escrito que se ignorase en silencio dejaría la tanda
corriendo con otros parámetros durante horas.

`Hash: 32` y `Threads: 1` se aplican por defecto si no se dicen, y quedan
escritos en el manifiesto.

### 3.3 Elegir las hipótesis

| Qué se quiere saber | `elo0` | `elo1` |
|---|---|---|
| ¿esta mejora aporta algo? | 0 | 5 |
| ¿este cambio (limpieza, refactor) no empeora? | −5 | 0 |
| ¿esta mejora es grande? | 0 | 10 |

`alpha = beta = 0.05` es lo estándar. Bajarlos alarga mucho las tandas.

Con `elo0=0, elo1=5` y α=β=0.05, una mejora real de +10 Elo suele decidirse
en unos cientos de parejas; una de +2 Elo puede no decidirse nunca dentro
del libro disponible, y eso es información honesta, no un fallo.

### 3.4 Control de búsqueda: la decisión más importante

`busqueda` admite exactamente uno de cuatro modos:

- **`nodos`** — el recomendado. Reproducible: la misma configuración da
  exactamente las mismas partidas, en esta máquina y en otra, con uno o con
  ocho workers. No depende de la carga del equipo.
- **`profundidad`** — también reproducible, pero favorece al motor que hace
  más trabajo por nodo, así que mide algo distinto de la fuerza real.
- **`movetime_ms`** — justo entre motores distintos, pero deja de ser
  reproducible: depende de la máquina y de lo que esté corriendo a la vez.
- **`reloj`** (`base_ms` + `incremento_ms`) — control de tiempo real, con
  los relojes llevados por el árbitro. Es lo único que mide la gestión de
  tiempo, y lo menos reproducible de todo.

> **Trampa real, encontrada al construir esto.** El control por nodos solo
> es justo si **los dos** binarios respetan `go nodes`. La release 0.24 de
> Vigía no lo hace: pedidos 25.000 nodos, anuncia la jugada tras unos 4.100
> (el 17 %). Enfrentada así a un binario que sí los respeta, pierde el 92 %
> de las partidas y el banco reporta **+422 Elo**. Esa cifra no mide fuerza:
> mide que un bando pensó cuatro veces menos.
>
> Desde entonces el banco **sondea ambos motores antes de empezar** y aborta
> si alguno se queda por debajo del 50 % de los nodos pedidos. Contra
> binarios antiguos hay que usar `movetime_ms`.

### 3.5 Recursos: hay que avisar antes

`partidas.workers` es el número de partidas simultáneas, y cada una ocupa
una CPU (dos procesos, pero solo uno pensando a la vez). **Antes de lanzar
una tanda larga hay que decirle al usuario cuántas CPUs se van a ocupar**
para que libere sitio. El comando lo imprime al arrancar:

```
recursos  : 4 worker(s) → hasta 4 CPUs ocupadas de 12 disponibles
```

Con `Threads > 1` en las opciones UCI, el banco exige `workers = 1`: si no,
se estaría midiendo la contención del planificador.

La memoria aproximada es `2 × workers × Hash`.

### 3.6 Qué produce

En el directorio `salida`:

- **`manifiesto.json`** — la configuración normalizada, el SHA-256 de cada
  binario y del libro, el `id name` que anunció cada motor, el entorno y una
  **firma** del experimento.
- **`parejas.jsonl`** — una línea JSON por pareja, en orden, con todas las
  jugadas (UCI, SAN, puntuación, profundidad, milisegundos). Es la **única
  fuente de verdad**; se vacía a disco tras cada pareja.
- **`resumen.json`** — recuentos, pentanomial, Elo con intervalo, LOS, LLR,
  decisión, reparto de finales.
- **`partidas.pgn`** — PGN estándar en SAN, con la puntuación y la
  profundidad de cada jugada como comentario, para abrirlo en cualquier
  visor y ver dónde se torció una partida.

`resumen.json` y `partidas.pgn` son derivados: `banco informe --run <dir>`
los regenera desde `parejas.jsonl`, verificando de paso que el fichero es un
prefijo contiguo y coherente.

### 3.7 Interrumpir y reanudar

Ctrl+C en cualquier momento. Lo escrito en `parejas.jsonl` está completo;
volver a lanzar el mismo comando reanuda desde ahí. La reanudación exige que
la **firma** coincida: si han cambiado los binarios, el libro, la semilla,
el control de búsqueda, la adjudicación o las hipótesis, el banco se niega a
mezclar.

Quedan deliberadamente **fuera** de la firma `workers`, los márgenes y
`max_parejas`, porque no cambian qué se mide: se puede reanudar con más o
menos CPUs libres, o alargar una tanda que acabó sin decidir.

Lo que **no** se puede hacer es alargar una tanda que ya decidió. El banco
lo rechaza: seguir jugando después de cruzar una frontera invalida el nivel
de significación de la prueba. Si hace falta más evidencia, se abre un
experimento nuevo con otro `id` y otro libro.

### 3.8 Cómo se lee el resultado

```
--- 025dev-vs-024 ---
Partidas: 414  (207 parejas)
Candidato: 369 ganadas, 23 tablas, 22 perdidas
Pentanomial [LL, LD, LW/DD, DW, WW]: [0, 3, 19, 20, 165]
Puntuación: 91.91 %
Elo: +422.1  (IC 95 %: +372.7 … +487.2)   LOS: 100.0 %
LLR: +2.9575   fronteras [-2.944, +2.944]   →  acepta_h1
```

- **`acepta_h1`**: hay evidencia a favor de `elo1`. El cambio se acepta.
- **`acepta_h0`**: hay evidencia a favor de `elo0`. Ojo con la lectura: con
  `elo0=0, elo1=5`, esto **no** demuestra que el cambio empeore, demuestra
  que no llega a +5 Elo.
- **`continuar`**: no se cruzó ninguna frontera. No es "casi": es que no se
  sabe. La cifra de Elo de una tanda que terminó en `continuar` es
  orientativa y su intervalo suele ser enorme.
- **LOS** (*likelihood of superiority*): probabilidad de que el candidato
  sea mejor. Es un complemento intuitivo, no la decisión.

El resumen avisa por su cuenta de dos cosas: partidas terminadas de forma
anómala (incomparecencia o jugada ilegal — eso no es un resultado de
ajedrez, es un motor que falla, y hasta explicarlo el Elo no es
interpretable) y una proporción alta de partidas que llegaron al tope de
plies.

---

## 4. `banco velocidad` — cambios de solo velocidad

Magic bitboards, un generador más rápido, una reordenación de estructuras:
cambios que deben ir más rápido **sin cambiar ni una decisión de la
búsqueda**. Eso es comprobable directamente, en segundos, sin miles de
partidas.

```bash
./target/release/banco.exe velocidad \
  --motor target/release/vigia.exe \
  --contra ../zzRelease/Vigia\ 0.25.exe \
  --profundidad 12
```

Doce posiciones a profundidad fija (apertura, medio juego táctico,
posiciones cerradas, varios finales). El criterio:

- **Los nodos por posición deben ser idénticos.** Si difieren, el cambio no
  es de solo velocidad: altera lo que la búsqueda visita, y entonces hay que
  pasarlo por `banco sprt`. El comando lo dice explícitamente y enseña las
  posiciones que divergen.
- **Los nodos/segundo deben subir.** Esa es la mejora.

Medido en esta máquina, el mismo binario contra sí mismo da ±4 % de
nodos/segundo entre ejecuciones. Una mejora por debajo de ese ruido necesita
repetir la medición varias veces antes de creérsela.

### Precedente: los magic bitboards

El primer cambio aprobado por este comando —y la versión 0.26.0 entera—,
que sirve de ejemplo de cómo se lee su salida. Sustituir el recorrido clásico de rayos por magic bitboards dio
nodos idénticos en las **12** posiciones —línea *"Nodos idénticos en las 12
posiciones"*, que es la que autoriza a saltarse `sprt`— y entre +12,8 % y
+18,7 % de nodos/segundo en tres ejecuciones seguidas (+18,7 / +12,8 / +13,9,
media ≈ +15 %). Tres y no una: la ganancia es del orden de cuatro veces el
ruido, pero la dispersión entre ellas (seis puntos porcentuales) enseña por
qué una sola cifra no se cita como si fuera exacta. Detalle en §3 de `docs/Documentacion_tecnica.md`.

Si la línea de nodos idénticos **no** aparece, el comando lista las
posiciones que divergen y el cambio deja de ser candidato a este comando:
pasa a `sprt`, sin excepciones y por mucho que la explicación de la
divergencia parezca inofensiva.

---

## 5. `banco epd` — no-regresión táctica

```bash
./target/release/banco.exe epd --fichero suite.epd \
  --motor target/release/vigia.exe --movetime 1000
```

Lee EPD con operaciones `bm` (mejor jugada), `am` (jugada a evitar) e `id`,
en SAN o en UCI. Reporta aciertos y lista los fallos con su FEN.

**No es una puerta de calidad y no mide fuerza.** Acertar más posiciones de
una lista no implica jugar mejor, y hay cambios buenos que bajan el
marcador. Sirve para dos cosas: detectar un derrumbe (de 180 a 90 aciertos
es un fallo, no ruido) y tener posiciones concretas que mirar cuando el SPRT
dice que algo empeoró y no se sabe por qué.

El proyecto no incluye ninguna suite; hay que traerla de fuera (WAC, ECM,
Arasan, etc.).

---

## 6. `banco humo` — verificar el propio banco

```bash
./target/release/banco.exe humo --motor target/release/vigia.exe
```

Enfrenta un binario **contra sí mismo**. Con control por nodos, un hilo y
`Variety` apagado, la búsqueda de Vigía es determinista, y `ucinewgame`
vacía la tabla de transposición. Por tanto, las dos partidas de cada pareja
son **la misma partida con los papeles cambiados**: quien abre con blancas
en la primera hace exactamente lo mismo que quien abre con blancas en la
segunda.

De ahí sale un invariante fuerte y fácil de comprobar: **todas las parejas
tienen que caer en el cajón central**, con 0,00 Elo exacto. Si no, el banco
introduce una asimetría entre los dos bandos (un arranque que contamina, un
margen que corta una búsqueda, estado que sobrevive entre partidas) y
ninguna medición suya es de fiar.

Es la prueba de extremo a extremo del sistema completo: árbitro, libro,
simetría de colores, paralelismo, persistencia y estadística.

Conviene ejecutarla después de tocar el banco, y tras cambiar el motor de
forma que pudiera afectar al determinismo.

---

## 7. `banco libro` — construir el libro de aperturas

El libro que se usa por defecto está en el repositorio:
`banco/libros/vigia-2000.epd`, **2.000 posiciones**. No hace falta
regenerarlo para reproducir nada. Sigue ahí `vigia-256.epd`, que es el que
usaron las tandas anteriores y hace falta para repetirlas tal cual.

```bash
./target/release/banco.exe libro \
  --fuente "C:/Ajedrez/Probon_Gem/apertura.txt" \
  --salida banco/libros/vigia-2000.epd \
  --n 2000 --min-jugada 1 --max-jugada 1 --semilla 20260818
```

Filtros por defecto: jugadas 4–12, al menos 26 piezas, no en jaque, no
terminada, y `|evaluación estática| ≤ 90 cp`.

**`--min-jugada 1 --max-jugada 1` no es cosmético**: todas las posiciones de
ese origen llevan el contador de jugada a 1 aunque tengan diez o quince
plies jugados. Con los valores por defecto el filtro las habría rechazado
las 3.825.105, y el error resultante habría sido "no hay bastantes
posiciones", que no señala hacia la causa. Al construir un libro conviene
leer primero el recuento de descartes que imprime el propio comando.

De 3.825.105 posiciones del origen —todas distintas entre sí, ni un solo
duplicado— pasan los filtros 3.082.412. Los descartes: 742.486 por
desequilibrio, 194 por estar en jaque y 13 por pocas piezas. De las
aceptadas se muestrean 2.000 con la semilla dada.

El libro anterior, de 256, salió de `C:/JC/Books/gm2001.epd` (27.925
posiciones, 17.374 tras los filtros). Ese fichero ya no está en el disco,
que es la razón de que ampliarlo obligara a cambiar de fuente.

La cabecera del fichero registra origen, su SHA-256, filtros y semilla, así
que el libro es auditable por sí solo.

**Sesgo conocido y asumido**: el equilibrio se mide con la evaluación
estática de Vigía, así que el libro está equilibrado *según Vigía*. Como
candidato y base juegan las mismas posiciones con los dos colores, ese sesgo
se cancela en la comparación; lo que no puede afirmarse es que el libro sea
neutral para un motor ajeno.

### Identidad y deduplicación

Dos aperturas son la misma unidad estadística si llevan a la misma posición.
La identidad son los **cuatro primeros campos FEN** (tablero, turno,
enroque, al paso); los relojes no cuentan. La deduplicación ocurre **antes**
de barajar y conserva la primera aparición en el orden original, de modo que
no depende de la semilla.

Si el libro tiene menos posiciones únicas que parejas pedidas, el banco se
planta: repetir aperturas reintroduce exactamente la correlación que las
parejas venían a eliminar.

Ampliar el libro es la vía natural para poder decidir mejoras pequeñas, y
por eso se pasó de 256 a 2.000.

### Cuántas parejas hacen falta de verdad

Aquí llegó a estar escrito que 2.000 posiciones bastaban para decidir
diferencias de +2–3 Elo. **Es falso, por un factor de ocho.** Lo que sigue
está medido sobre las 1.000 parejas de `027-tt-quiescencia`, no estimado:

| resolución buscada | parejas | partidas | a 2 CPUs, 100 ms/jugada |
|---|---:|---:|---:|
| ±12 Elo | 1.000 | 2.000 | 3 h |
| ±10 Elo | 1.518 | 3.036 | 4,6 h |
| ±5 Elo | 6.072 | 12.143 | 18,6 h |
| ±3 Elo | 16.866 | 33.731 | 51,5 h |
| ±2 Elo | 37.947 | 75.895 | 116 h |

El intervalo se estrecha con la raíz de las parejas, así que **ganar un
factor 2 de resolución cuesta un factor 4 de partidas**. Esa es la razón de
que los bancos serios midan en decenas de miles de partidas y no en miles.

Dos consecuencias prácticas:

- El **libro es el techo duro**: no se pueden pedir más parejas que
  posiciones únicas tenga (repetir aperturas reintroduce la correlación que
  las parejas venían a eliminar). Con 2.000 posiciones el techo es ±8,7 Elo,
  se disponga del tiempo que se disponga. Para bajar de ahí hay que generar
  un libro mayor primero — la fuente de `apertura.txt` tiene 3,8 millones de
  posiciones y generar 20.000 cuesta 17 segundos.
- Antes de lanzar una tanda conviene **mirar en esta tabla si la pregunta
  cabe en el presupuesto**. Un contraste `elo0=0, elo1=5` sobre una mejora
  real de +12 Elo necesitó más de 2.000 parejas para cruzar la frontera; una
  de +3 Elo no es contestable en una noche.

**Cuidado con el tamaño del origen.** La carga deduplica por identidad FEN4,
y esa deduplicación fue cuadrática hasta que se arregló: con las 27.925
posiciones del libro viejo costaba un par de segundos y nadie lo notó, pero
con un fichero de 3,8 millones no terminaba en horas. Hoy son 17 segundos.
Si alguna vez vuelve a tardar de forma desproporcionada, el sospechoso es
ese, y hay un test que lo vigila
(`loading_a_large_source_does_not_degrade_quadratically`).

---

## 8. Decisiones de diseño y por qué

### El árbitro usa las reglas del propio motor

`banco` enlaza contra la biblioteca `vigia` y decide mate, ahogado, las
cincuenta jugadas, repetición triple y material insuficiente con
`vigia::movegen` y `vigia::eval::is_insufficient_material`, sin fiarse de lo
que digan los procesos.

**Contrapartida asumida**: si el generador de jugadas tuviera un fallo, el
árbitro tendría el mismo fallo. Se acepta porque los perft y la suite de 210
tests del motor cubren esa parte, y porque la alternativa (una segunda
implementación de las reglas) ya demostró ser peor: hubo tres copias
distintas de la regla de material insuficiente y llegaron a discrepar sobre
K+A vs K+A en casillas del mismo color.

### Las tablas por regla se aplican solas

Repetición triple y cincuenta jugadas se adjudican automáticamente, sin
preguntar al motor. UCI no tiene comando para reclamarlas, así que cualquier
alternativa exige inventarse un convenio. En un banco A/B los dos bandos son
el mismo motor o parientes cercanos, de modo que un convenio elaborado no
compra precisión y sí añade una fuente de discrepancia entre ejecuciones.

### El abandono adjudicado exige que los dos motores coincidan

Para adjudicar una partida perdida, el que pierde debe verse por debajo de
`−resign_cp` **y** el que gana por encima de `+resign_cp`, durante
`resign_plies` jugadas consecutivas cada uno. Con un solo lado bastaría el
pesimismo de un motor mal calibrado para regalar partidas salvables.

Las reglas duras se comprueban **antes** que la adjudicación, así que un
mate nunca queda tapado por un abandono.

### Un hueco en las puntuaciones rompe la ventana

Si un motor devuelve `bestmove` sin haber anunciado ningún `info … score`
utilizable, esa jugada cuenta como `None` y **corta** la racha, en lugar de
saltarse el hueco y unir puntuaciones no consecutivas. Unir huecos equivale
a adjudicar sobre una ventana que nunca se cumplió: la partida se corta
antes de tiempo y el resultado deja de corresponder a las reglas
declaradas.

También se descartan las líneas `multipv` distintas de 1 y las puntuaciones
marcadas `lowerbound`/`upperbound`.

### El SPRT se evalúa sobre el prefijo contiguo

Con varios workers las parejas terminan desordenadas. Si el LLR se evaluara
sobre "todo lo que hay hecho", la decisión dependería de qué worker acabó
antes, es decir, de la carga de la máquina, y dos ejecuciones idénticas
podrían parar en sitios distintos.

Por eso el LLR se evalúa solo sobre el prefijo contiguo (0, 1, 2, … sin
huecos). Una pareja que acaba antes de tiempo espera en memoria. Al cruzar
una frontera, las parejas especulativas calculadas más allá **se
descartan**: no entraron en el cálculo y guardarlas invitaría a mirarlas.

Comprobado: la misma tanda A/A con 1 worker y con 4 produce ficheros de
partidas byte a byte idénticos.

### Sin dependencias

El motor no tiene dependencias y el banco tampoco. SHA-256, JSON, el
generador aleatorio, la estadística y el SAN están escritos aquí, con sus
tests. Si validar una mejora exigiera instalar Python, `python-chess` o
SciPy, el banco dejaría de ser reproducible por sí solo y pasaría a depender
del estado de una máquina concreta.

---

## 9. Qué se tomó del banco de GPT y qué no

El motor Crepúsculo (`../GPT`) tiene su propio sistema, documentado en
`GPT/docs/SELFPLAY.md`. Se revisó antes de construir este. Su carpeta no se
ha tocado.

**Ideas adoptadas** (son correctas y el motivo es bueno):

- La pareja como unidad estadística y el LLR pentanomial de Fishtest.
- Que un hueco de puntuación rompa la ventana de adjudicación en vez de
  saltarse — GPT lo descubrió auditando sus propios resultados de 0.32, y es
  un fallo fácil de cometer.
- Deduplicar aperturas por los cuatro primeros campos FEN, antes de barajar.
- Sellar binarios y configuración con hashes en un manifiesto, y negarse a
  continuar si algo cambió.
- Que la decisión secuencial sea real y no una suma a posteriori.
- Anunciar el número de CPUs antes de una tanda larga.

**Diferencias deliberadas**:

| | GPT (Crepúsculo) | Aquí (Vigía) |
|---|---|---|
| Lenguaje | Python + `python-chess` | Rust, dentro del propio crate |
| Reglas | `python-chess` (implementación independiente) | las del propio motor |
| Continuación | segmentos encadenados + `selfplay_combine.py` | un directorio reanudable |
| Integridad | cadena de hashes por pareja | firma del experimento + prefijo contiguo verificado |
| Tablas por regla | convenio `engine-selected-move-nonwinning` | automáticas |
| Abandono | un lado | los dos lados deben coincidir |
| Alcance | solo autojuego | + banco de velocidad y suites EPD |

La cadena de hashes por pareja y el combinador estricto de GPT dan garantías
frente a manipulación que aquí no se replican, porque el modelo de amenaza
es distinto: una máquina, un operador, y un fichero `parejas.jsonl` que se
verifica entero (prefijo contiguo, índices correlativos y cajón recalculado
desde los resultados) cada vez que se lee.

---

## 10. Procedimiento para validar una mejora

1. **Antes de tocar nada**, comprobar que el estado de partida está limpio:
   ```bash
   cargo test --release
   cargo clippy --release --all-targets   # 0 avisos
   ```
2. Implementar **una sola** mejora. Nunca varias a la vez: si el resultado
   sale mal no se sabe cuál fue.
3. Si el cambio es de solo velocidad → `banco velocidad`. Si los nodos
   cambian, no lo era: sigue por el paso 4.
4. `banco sprt` contra la release cerrada anterior, con `elo0=0, elo1=5`.
   Avisar antes de cuántas CPUs se van a ocupar.
5. Leer el resultado con honestidad. `acepta_h0` con esas hipótesis no
   significa "empeora", significa "no llega a +5 Elo".
6. Si el resultado es malo y no se sabe por qué, mirar `partidas.pgn`
   (las puntuaciones por jugada están ahí) y pasar una suite EPD.
7. Anotar el `id` del experimento y su decisión en
   `docs/Documentacion_tecnica.md`. El directorio de resultados se queda
   fuera del repositorio, pero el manifiesto permite repetirlo.

### Higiene estadística

- **Una hipótesis por experimento.** Lanzar el mismo cambio con tres pares
  de hipótesis y quedarse con el que sale bien es hacer trampas.
- **No mirar y parar.** El SPRT ya decide cuándo parar; cortar una tanda a
  mano porque "va bien" invalida el α declarado.
- **No reutilizar el libro entre segmentos** de un mismo experimento.
- **Semilla y libro se declaran antes**, no después de ver el resultado.

---

## 11. Estado y resultados registrados

### Verificaciones del propio banco

| qué se comprobó | resultado |
|---|---|
| A/A determinista (`banco humo`, 8 parejas, 20.000 nodos) | pentanomial `[0,0,8,0,0]`, 0,00 Elo exacto |
| Misma tanda A/A con 1 worker y con 4 | ficheros de partidas byte a byte idénticos |
| Reanudación desde una tanda truncada a 3 parejas | resultado final idéntico a la tanda completa |
| Reanudar con otro control de búsqueda | rechazado por firma distinta |
| `banco velocidad` de un binario contra sí mismo | nodos idénticos en las 12 posiciones; ±4 % de ruido en nodos/segundo |
| Suite completa | 339 tests (210 motor + 121 banco + 8 harness antiguo), 0 avisos de clippy |

### Experimentos registrados

| id | qué compara | control | resultado |
|---|---|---|---|
| `humo-A-contra-A` | 0.25-dev contra sí mismo | 20.000 nodos | `[0,0,8,0,0]`, 0,00 Elo |
| `025dev-vs-024` (nodos) | 0.25-dev vs 0.24 | 25.000 nodos | **inválido**: 0.24 no respeta `go nodes`. Ver §3.4 |
| `025dev-vs-024` | 0.25-dev vs 0.24 | `movetime` 100 ms | 128 parejas: **−17,7 Elo** (IC 95 % −53,7 … +18,0), LLR −0,30, **`continuar`** |

La tercera fila es la primera medición seria del proyecto y su lectura está
desarrollada en `docs/Documentacion_tecnica.md` §8.6. En resumen: la prueba
**no decidió**, y la cifra de +77 Elo que figuraba para 0.25 frente a 0.24
no se reproduce a este control. Como `banco velocidad` muestra que 0.25-dev
es un ~16 % más lento en nodos/segundo que 0.24, a 100 ms por jugada esa
lentitud pesa; el dato antiguo era a 300 y 800 ms.

Pendiente: repetir a 300 y 800 ms, y ampliar el libro por encima de 256
posiciones para poder decidir diferencias pequeñas.

---

## 12. Mapa del código

`src/bin/banco/`:

| fichero | contenido |
|---|---|
| `main.rs` | CLI y despacho de comandos |
| `stats.rs` | SPRT pentanomial, MLE por ecuación secular, Elo e IC, LOS |
| `run.rs` | orquestador: workers, prefijo contiguo, manifiesto, reanudación |
| `config.rs` | lectura y normalización de la configuración, firma |
| `arbitro.rs` | reglas de la partida y adjudicación |
| `motor.rs` | proceso UCI: saludo, opciones, `go`, parseo de `info` |
| `libro.rs` | carga, deduplicación, barajado y filtrado de aperturas |
| `san.rs` | notación algebraica estándar |
| `pgn.rs` | exportación a PGN |
| `velocidad.rs` | banco de nodos y nodos/segundo |
| `epd.rs` | suites EPD |
| `json.rs` | JSON mínimo, determinista |
| `sha256.rs` | SHA-256 |

121 tests propios, incluidos en `cargo test --release`.

El harness antiguo `src/bin/selfplay.rs` sigue compilando y funcionando,
pero está **superado**: sus 8 aperturas y 16 partidas no distinguen +20 Elo
del ruido. Se mantiene porque su documentación histórica lo cita.
