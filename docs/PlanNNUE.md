# Plan de NNUE de Vigía

**Estado:** decidido y en ejecución: fases 0 y 1 hechas, fase 2 en marcha (seguimiento en
`docs/MejorasPendientes.md`). **Versión de partida:** 0.28.0 (`dde715a`). **Base de todas las
comparaciones:** `Release/Vigia 0.28.exe` congelada.

Las referencias a línea concreta son de 0.28 y se moverán; el nombre de la función es lo
que manda.

---

## 1. La decisión, en un párrafo

Se implementa **Atalaya-256**: una red `772 → 2×256 → 1` con rasgos planos de pieza y
casilla (sin cubos de rey), acumulador incremental `i16` de doble perspectiva, activación
SCReLU cuantizada y una capa de salida directa troceada en cubos por número de piezas,
empotrada en el binario con `include_bytes!`. Se elige **por datos y por verificabilidad,
no por velocidad**: el presupuesto de velocidad no discrimina entre las candidatas que se
estudiaron —todas caben con holgura y todas entran con crédito de nps, no con deuda—,
mientras que los otros dos ejes sí separan. Con el corpus que Vigía puede generarse a sí
mismo en una noche de máquina (25 M posiciones filtradas, que por la correlación
intra-partida medida valen unas **2,2 M muestras efectivas**), 201.992 parámetros son el
punto donde red y datos se saturan a la vez: N=128 deja capacidad pagada sin usar y
HalfKP con cubos de rey se queda en 3,4 muestras efectivas por parámetro (1,8 en su cubo
más pobre). Y con rasgos que no dependen de ninguna otra pieza, el acumulador **nunca se
recalcula durante una búsqueda** —una jugada de rey cuesta exactamente lo mismo que una de
peón—, lo que elimina de raíz el único riesgo del proyecto que nadie ha podido cuantificar
todavía. La HCE no desaparece del todo: sobreviven las tres cosas que son *exactas* y
cuestan casi nada (el oráculo KPK, el factor de escala de final y la detección de material
insuficiente), y se tira todo lo que es *aproximado*, que es el 94 % de su coste.

---

## 2. El presupuesto

### 2.1 La base medida

Todo lo de esta sección está medido en la máquina de trabajo (Ryzen 9 9950X), con el
binario tal y como se compila hoy: `lto = true`, `codegen-units = 1`, sin `.cargo/config.toml`
y sin `RUSTFLAGS`. `rustc --print cfg` confirma **sse, sse2, sse3 y nada más**: no hay
SSSE3 (`pmaddubsw`) ni SSE4.1 (`pmulld`).

| magnitud | valor | cómo se midió |
|---|---|---|
| tiempo por nodo | **657,3 ns** (1.521.400 nps) | `banco velocidad --profundidad 12 --hash 32 --hilos 1`, 12 posiciones, 5.645.908 nodos |
| fracción del nodo que es evaluación | **37,3 %**, IC [32,6 %, 42,0 %] | 10 pasadas pareadas con la evaluación duplicada bajo `black_box` |
| coste de la HCE | **245,2 ns/nodo** = 305,8 ns/evaluación | derivado de los dos anteriores |
| evaluaciones por nodo | **0,802** | contadores atómicos, profundidad fija 12 |
| `make_move` por nodo | **0,927** (= 1,157 por evaluación) | ídem |
| derateo microbanco → búsqueda | **×1,48** | 206,2 ns aislado frente a 305,8 ns dentro de la búsqueda, misma función |
| Elo por nps | **2,26 Elo / 1 %**, 175 Elo al doblar | §8.7 de `Documentacion_tecnica.md`, dos controles de tiempo, 16.144 partidas |

De ahí sale el presupuesto acordado. Aceptar una caída del 30 % de nps es aceptar
`T' ≤ 657,3 / 0,70 = 939,0 ns` por nodo, es decir **526,9 ns/nodo para toda la ruta de
evaluación** (frente a los 245,2 de hoy), o **657 ns por evaluación**. La restricción
exacta, con los dos términos a sus ritmos respectivos, es:

```
0,927 · C_acumulador  +  0,802 · C_densa  +  0,802 · C_prefacio  ≤  526,9 ns/nodo
```

### 2.2 Los núcleos, medidos

| núcleo | ns crudos | notas |
|---|---|---|
| acumulador fundido, N=256, 2 perspectivas, 50 % capturas, tabla 384 KB | **25,8** | por jugada; copia padre→hijo fundida en el mismo bucle |
| el mismo con tabla de 1,25 MB | **29,6** | **+15 % solo por salirse de la L2** |
| capa de salida SCReLU, 2N=512, QA=127 | **46,4** | por evaluación |
| capa de salida CReLU, 2N=512 | 39,0 | la salida de emergencia |
| prefacio KPK + `game_phase` + `endgame_scale_factor` | ~13 | 11,13 medidos para el núcleo entero desde cero, 10,0 para KPK puro |
| reconstruir la lista de cambios desde `(board, mv)` | **5–15 — POR MEDIR** | se usa el peor caso |

Dos correcciones importantes respecto de las estimaciones que circulaban antes:

- **El ritmo de 6,8 MAC·ns⁻¹ no se aplica a la capa de salida.** Ese número se midió sobre
  una capa afín 512×32 escrita por filas, que es el caso patológico. Una capa `2N → 1`
  tiene una sola salida: es un producto escalar con reducción, vectoriza limpiamente y va
  a **11–13 elementos/ns crudos**, entre 2,2 y 2,8 veces más rápido. Diseñar con 6,8
  MAC·ns⁻¹ lleva a elegir la red demasiado pequeña por la razón equivocada.
- **El desplazamiento de la SCReLU no está medido.** Los 46,4 ns son sin el `>>4` que la
  cuantización exige (§3.4). Se presupuestan **51,0 ns** (+10 %) hasta que se mida.

### 2.3 La cuenta de Atalaya-256

```
C_acumulador = 25,8 (medido) + 15,0 (reconstrucción, peor caso, POR MEDIR)
             = 40,8 ns crudos × 1,48 = 60,4 ns por make_move
             × 0,927 make_move/nodo                      =  56,0 ns/nodo

C_densa      = 51,0 ns crudos × 1,48 = 75,5 ns por evaluación
             × 0,802 evaluaciones/nodo                   =  60,5 ns/nodo

C_prefacio   = 13,0 ns crudos × 1,48 = 19,2 ns por evaluación
             × 0,802                                     =  15,4 ns/nodo
                                                            ──────────
RUTA DE EVALUACIÓN NUEVA                                    131,9 ns/nodo
RUTA DE EVALUACIÓN ACTUAL (HCE)                             245,2 ns/nodo
```

| | valor |
|---|---|
| tiempo por nodo | 657,3 − 245,2 + 131,9 = **544,0 ns** |
| nodos por segundo | 657,3 / 544,0 = **+20,8 %** |
| Elo (ley logarítmica, 175 al doblar) | **+48 Elo de CRÉDITO** |
| Elo (ley lineal, 2,26 por punto) | +47 Elo — coinciden |
| presupuesto consumido | 131,9 / 526,9 = **25,0 %** |

Con el mejor caso de la reconstrucción (5 ns en vez de 15) sale +23,9 % de nps y **+54
Elo**. Se cita **+48**, el conservador.

### 2.4 Cuánto tiene que ganar la red para que el trato salga

Esta es la cifra que gobierna el proyecto entero, y no tiene nada que ver con la deuda de
−68 Elo del planteamiento inicial: **esa deuda no existe**.

- La red entra con **+48 Elo** puestos en la mesa antes de evaluar mejor ni una posición.
- El SPRT de aprobación es `elo0 = 0, elo1 = 5` (§8, fase 5).
- Por tanto: **la red puede evaluar hasta unos 43 Elo PEOR que la HCE de 2.384 líneas y
  aun así el cambio se aprueba.** Todo lo que evalúe mejor se suma encima.

Que una red de 202.000 parámetros destilada de búsquedas de profundidad ~7,5 quede 43 Elo
por debajo de la HCE actual solo es concebible si hay un fallo de implementación, y para
eso está la sección 7.

### 2.5 Sensibilidad al riesgo que no está medido

El acumulador se midió con la tabla caliente. En la búsqueda real cada jugada toca
columnas distintas y compite con una TT de 128–256 MB. Si el coste real del acumulador es
`k` veces el medido:

| k | ns/nodo | Δ nps | Elo |
|---|---|---|---|
| ×1 (medido) | 131,9 | +20,8 % | **+48** |
| ×2 | 187,9 | +9,7 % | +23 |
| ×3,02 | 245,2 | **0,0 %** | 0 — empate con la HCE |
| ×4 | 299,9 | −8,0 % | −21 |
| ×8,05 | 526,9 | −30,0 % | −68 — el techo acordado |

**Atalaya-256 aguanta un factor 8 de error en lo único que no está medido antes de tocar
el techo, y un factor 3 antes de perder velocidad neta.** Ese margen —y no la cifra de
+48— es la razón de elegir N=256 y una tabla de 386 KiB que cabe en la L2 de 1 MiB de un
núcleo Zen 5.

### 2.6 Lo que esta cuenta *no* dice

- **0,802 y 0,927 se van a mover.** Son constantes del árbol, y el árbol lo dibuja la
  evaluación: entre el 58 % y el 79 % de las evaluaciones de negamax mueren en un corte
  inmediato por RFP (`search.rs:1264`), y ese porcentaje es función directa de qué números
  devuelve la evaluación. Un desplazamiento del 10 % en evaluaciones/nodo son ±6 ns/nodo,
  ±2 Elo. **Ninguna de estas cifras se da por buena hasta pasar `banco velocidad` con el
  acumulador de verdad dentro del motor** (fase 1).
- Todo está medido **a un hilo y sin TT compitiendo**. Un SPRT corre hasta 16 partidas
  simultáneas. Los nps reales serán peores; cuánto, **por medir**.

---

## 3. La arquitectura, exacta

### 3.1 Rasgos: 772 por perspectiva

**768 de pieza × casilla**, relativos a la perspectiva, con las dos perspectivas
compartiendo la misma tabla de pesos:

```rust
// persp: 0 = blancas, 1 = negras.
// Square es rank*8 + file (A1 = 0), luego el espejado vertical es sq ^ 56.
#[inline(always)]
fn feature_index(persp: usize, piece_color: usize, kind: usize, sq: usize) -> usize {
    let s = sq ^ (56 * persp);            // orientación de la perspectiva
    let theirs = (piece_color != persp) as usize;
    theirs * 384 + kind * 64 + s          // 0..767
}
```

Dos operaciones y ni una consulta a tabla. El rey es `kind = 5`: **una pieza más**, no un
índice. Un XOR y dos sumas en tiempo de ejecución.

**4 de derechos de enroque**, índices 768..771: `768 + relative_side * 2 + kingside`, donde
`relative_side` es 0 si el derecho es del bando que mira. Es el único dato de la posición
que 768 rasgos planos no pueden ver por ningún camino —un rey en g1 con torre en f1 es
idéntico a un rey en g1 que todavía puede enrocar— y es exactamente la clase de
información de seguridad del rey que más cuesta aprender de forma aditiva. Cambia como
mucho una vez por bando y por partida, y es un rasgo de una sola entidad, así que **no
rompe la invariante de §4**.

**La invariante que define la arquitectura, y que hay que poder enunciar en una línea:**
*ningún rasgo depende de más de una pieza, luego toda jugada cambia como mucho 4 rasgos
por perspectiva y el acumulador nunca se recalcula desde cero durante una búsqueda.*

Activos por posición: entre 5 y 36 por perspectiva (32 piezas + 4 banderas).

### 3.2 Topología

```
        772 rasgos ─┐ (perspectiva del que mueve)   772 rasgos ─┐ (la del rival)
                    ▼                                            ▼
             acumulador i16 [256]                      acumulador i16 [256]
                    └─────────────┬──────────────────────────────┘
                        concatenado [512]  (el que mueve primero, siempre)
                                  ▼
                       SCReLU:  t = (clamp(a,0,127)²) >> 4
                                  ▼
              cubo = CUBO[popcount(occupied)]   → 1 de n_cubos vectores
                                  ▼
                        producto escalar i32 → centipeones
```

| bloque | forma | tipo | parámetros | bytes |
|---|---|---|---|---|
| pesos del transformador (FT) | 772 × 256, disposición **[rasgo][dimensión]** | i16 | 197.632 | 395.264 |
| sesgos del FT | 256 | i16 | 256 | 512 |
| pesos de salida | 8 × 512 | i16 | 4.096 | 8.192 |
| sesgos de salida | 8 | i32 | 8 | 32 |
| **total** | | | **201.992** | **404.000 B** |

Fichero empotrado: 404.000 + 128 B de cabecera = **404.128 B ≈ 394,7 KiB**. El binario de
hoy son 1,26 MB; pasa a ~1,66 MB. Doce reentrenamientos sucesivos son ~4,7 MB de historia
de git.

**Tabla de pesos del FT en RAM: 386 KiB, compartidos e inmutables entre los 16 hilos.**
Cabe en la L2 de 1 MiB de un núcleo junto con la pila de acumuladores (129 KiB por hilo).
Ese es el argumento de caché entero, y está respaldado por la única medida que existe del
efecto: el mismo bucle cuesta 25,8 ns con tabla de 384 KB y 29,6 ns con tabla de 1,25 MB.

**Los cubos de salida cuestan cero tiempo** (se evalúa uno) y multiplican por `n_cubos` la
capacidad de la única capa que ve la fase de la partida. Pero cuestan **datos**: son los
únicos pesos de la red que no comparten información entre sí. Por eso el mapa de cubos
**no está compilado en el motor**: es una `tabla CUBO[33]` (número de piezas → índice de
cubo) que viene en la cabecera del fichero de red, junto con `n_cubos`. El entrenador mide
la ocupación real sobre el corpus y **fusiona con su vecino cualquier cubo que no llegue a
1 M de posiciones** (≈ 90.000 muestras efectivas, ≈ 175 por parámetro del cubo). El motor
no sabe nada de la regla: lee 33 bytes.

Valor de partida, si la ocupación lo permite: `CUBO[p] = min(7, (p−1)/4)`, `n_cubos = 8`.

**No hay capas ocultas densas, y no es pereza: es aritmética.** Una capa 512→32 cuesta
2.485 ns derateados, **3,8 veces el presupuesto entero de una evaluación**. Ni con AVX2
cabría (607 ns, el 92 % ella sola). Acumulador → salida, y punto.

### 3.3 Prefacio y postfiltro

La función de evaluación completa es:

```
1. si game_phase(board) == 0 y hay exactamente un peón:
       return kpk_exact_score(board)                        [tablabase exacta, ~10 ns]
2. raw = red(acc, side_to_move)                             [centipeones]
3. return (raw * endgame_scale_factor(board, raw)) / 64
```

- **El oráculo KPK se queda delante.** Es análisis retrógrado resuelto, no una opinión;
  ninguna red destilada de autojuego va a igualar «esto es tablas con certeza» con la torre
  equivocada. Y sale gratis: el atajo de `eval.rs:166-173` corta antes de calcular nada.
- **`endgame_scale_factor` se queda detrás**, con la salida de la red como `raw`. Sus tres
  predicados (`is_drawn_by_insufficient_material`, `pawnless_drawish_scale`,
  `is_pure_opposite_colored_bishops_ending`) son predicados sobre el **tablero**, no sobre
  las tripas de la HCE, así que funcionan igual. Se le pasa `board` completo, **no un
  resumen de contadores**: `pawnless_drawish_scale` llama a `has_nearly_promoting_pawn`
  (`eval.rs:709`), que pregunta por el *rango* de los peones del bando débil, y eso no es
  información de recuento. Quitar el factor de escala reintroduciría el fallo documentado
  en `eval.rs:726-729`.
- `is_insufficient_material` (`eval.rs:651`) es **regla de juego**, no evaluación: sigue
  donde está, en `terminal_draw_score`.
- Desaparecen de la ruta caliente `tempo_score` (`eval.rs:248`) y `mop_up_score`
  (`eval.rs:327`). Aviso: el comentario de `search.rs:1279-1285` razona sobre el término de
  tempo para explicar por qué dos null moves consecutivos no coinciden; la guarda
  `null_at_ply` sigue en pie, así que nada se rompe, pero **ese comentario hay que
  reescribirlo**.

### 3.4 Cuantización

**Todo `i16`. Medido:** sin SSSE3 no existe `pmaddubsw` y LLVM ensancha `i8` a `i16` de
todas formas — la misma capa densa da 1.688 ns en i8 y 1.679 ns en i16, empate; con AVX2 el
i8 es un 32 % *peor*. La danza u8×i8 de Stockfish es una optimización de un ISA que este
binario no tiene. Un solo tipo, y el entrenador se simplifica.

| bloque | tipo | escala |
|---|---|---|
| `W_ft`, `b_ft`, acumulador | i16 | **QA = 127** |
| `W_out` | i16 | **QB = 32** |
| `b_out`, suma de salida | i32 | QA²·QB/16 = **32.258** |

```
v = clamp(a, 0, 127)                    // pmaxsw + pminsw, i16
u = v * v                               // pmullw, ≤ 16.129, cabe en i16 CON SIGNO
t = u >> 4                              // psraw, ≤ 1.008
S = b_out[cubo] + Σ_j t_j · W_out[cubo][j]        // i16×i16→i32, pmaddwd
cp = (S + signo(S) · 16.129) / 32.258   // división, NO desplazamiento
```

**QA = 127 y no 255** porque con 255 el cuadrado llega a 65.025 y hay que desempaquetar a
i32, doblando el número de vectores en el bucle más caliente de la evaluación. Con 127 la
cadena entera se queda en 8 carriles por vector. Se pierde un bit de resolución por
elemento, promediado sobre 512 términos. Es una decisión específica de SSE2: si algún día
se compila con AVX2, hay que volver a medir QA = 255. **Desde 0.30 los núcleos corren con
AVX2 y AVX-512**: queda pendiente en `docs/MejorasPendientes.md`.

**El `>> 4`** acota `t ≤ 1.008` y deja el peor caso conservador de la salida tres órdenes
de magnitud dentro de i32, sin recurrir a i64 (que partiría por la mitad el ancho SIMD).
Es exactamente lo que hace Stockfish con su `(x*x) >> 19`; el desplazamiento extra lo
compensa el entrenador.

**División y no desplazamiento en la escala final.** `>>` aritmético trunca hacia −∞ y
rompe la antisimetría `eval(p) = −eval(p con los colores cambiados)`, de la que depende la
búsqueda. Una división por constante es un multiply-high y un shift, una vez por
evaluación.

**K, la escala de la sigmoide, no entra en la inferencia.** Solo aparece en la pérdida del
entrenador, que compara `σ(pred_cp/K)` con `σ(cp_etiqueta/K)`. Como σ es inyectiva y las
etiquetas son puntuaciones de búsqueda de Vigía en centipeones de Vigía, **la salida de la
red queda en centipeones por construcción**. K va en la cabecera del fichero para que se
autodescriba y para el test de escala, pero el motor no lo lee para evaluar.

Esto es lo que salva los ocho márgenes de poda ya calibrados: RFP 75/ply
(`search.rs:111`), razoring 300/ply (`:106`), futility `[0,200,300]` (`:92`), delta 200
(`:97`), la ventana de aspiración y `CORRECTION_MAX = 300` (`:135`). Es el riesgo más
subestimado del cambio a red —que evalúe mejor y el motor salga más débil porque las podas
se desajustan todas a la vez— y se cierra por diseño, no por suerte.

### 3.5 Cotas de desbordamiento, demostradas

Stockfish no comprueba nada y confía en el entrenador. Aquí sale gratis hacerlo bien, y son
dos tests que se ejecutan **con la red cargada**:

**T1 — acumulador.** Como mucho 36 rasgos activos por perspectiva (32 piezas + 4 banderas).
Para cada dimensión `j` de 0..256:

```
b_ft[j] + Σ (los 36 mayores W_ft[·][j] positivos)  <  32.767
b_ft[j] + Σ (los 36 menores W_ft[·][j] negativos)  > −32.768
```

El entrenador lo garantiza recortando `|w| ≤ 6,0` tras cada paso del optimizador, es decir
`|W| ≤ 762`: `36 × 762 = 27.432`, con margen. El test es la red de seguridad, no el
mecanismo.

**T2 — salida.** Para cada cubo `b`:

```
Σ_j |W_out[b][j]| · 1.008 + |b_out[b]|  <  2³¹
```

Es decir `Σ|W_out| < 2.130.000`, o `|w_out|` medio < 130 con QB=32. Una red real tiene los
pesos de salida en `[−2, +2]`: margen de unas 65 veces.

Con T1 y T2 el desbordamiento es **imposible por aritmética**, no por costumbre. Y como
`v*v ≤ 16.129` cabe en `i16` con signo, el código es correcto también en build de
depuración, donde Rust entra en pánico al desbordar.

### 3.6 Formato del fichero

Cabecera de 128 B, todo little-endian:

| campo | bytes | contenido |
|---|---|---|
| magic | 8 | `VIGIANN1` |
| versión de arquitectura | 4 | rasgos, N, n_cubos, QA, QB empaquetados; **el cargador aborta si no cuadra con las constantes compiladas** |
| K | 2 | la escala de la sigmoide con la que se entrenó (documental) |
| n_cubos | 1 | 1..8 |
| CUBO[33] | 33 | número de piezas → índice de cubo |
| sha256 del corpus | 32 | el fichero de datos con el que se entrenó |
| posiciones vistas | 8 | |
| sha256 de los pesos | 32 | lo verifica el cargador |
| reserva | 8 | ceros |

Luego los pesos en crudo. **Sin LEB128**: comprimiría un ~15 % de 394 KiB a cambio de 50
líneas de descompresor y su riesgo. A Stockfish, con 98 MB, sí le compensa.

**`include_bytes!` da un `&[u8]` sin alineación garantizada.** La decodificación es con
`i16::from_le_bytes` a montículo (~1 ms al arrancar), **jamás con `transmute`**, que sería
el primer `unsafe` de un `src/` que entonces no tenía ninguno (los primeros llegaron en
0.30: las llamadas a los núcleos AVX2 y AVX-512, detrás de la comprobación de la CPU). Y la estructura se arma en un
`Vec` que se convierte a `Box<[i16]>`: `Box::new([[i16; 256]; 772])` materializa 386 KiB en
la pila antes de moverlos al montículo y revienta en build de depuración.

---

## 4. El acumulador incremental

### 4.1 Dónde vive

**En `Context` (`search.rs:444`), nunca en `Board`.** `Board` es `Clone` y se clona por
hilo (`uci.rs:468`, `search.rs:832`), en perft y en el árbitro del banco; meterle 129 KiB
convertiría cada clon en una copia enorme y contaminaría código que no quiere evaluar nada.
En `Context` sale **un acumulador por hilo sin una sola línea de sincronización**, porque
Lazy SMP ya construye un `Context` entero por hilo y solo comparte la `Tt`
(`search.rs:476-481`).

```rust
pub struct EvalState {
    net: &'static Net,          // obtenido una vez por búsqueda, no por nodo
    acc: Box<[i16]>,            // (MAX_PLY+1) · 2 · 256 = 66.048 i16 = 129 KiB
}
```

`MAX_PLY = 128` (`search.rs:17`) → 129 ranuras. Con 16 hilos, 2,1 MB en total.

**Array indexado por ply, no pila con push/pop.** `acc[ply+1]` se escribe leyendo
`acc[ply]`, con `split_at_mut` para el préstamo.

### 4.2 Dónde se engancha

`push` se llama **antes** de cada `make_move`, para que sea función pura de `(board, mv)` y
no dependa de los campos privados de `Undo` (`board.rs:93-95`, `captured` y
`capture_square` son privados):

| sitio (0.28) | qué es | qué se hace |
|---|---|---|
| `search.rs:994` | bucle raíz | `ctx.eval.push(board, mv, ply)` antes del `make_move` |
| `search.rs:1419` | bucle de negamax | ídem |
| `search.rs:1692` | bucle de quiescencia | ídem |
| `search.rs:1295` | null move | `ctx.eval.copy(ply)` — copia la ranura, sin cambios |

Y las cuatro lecturas pasan a `ctx.eval.evaluate(board, ply)`:

| sitio | qué es |
|---|---|
| `search.rs:1221` | `raw_eval` del nodo interior (el consumidor dominante: razoring, RFP, NMP, futility, `improving`) |
| `search.rs:1652` | `stand_pat` de quiescencia |
| `search.rs:1151`, `search.rs:1575` | topes `ply >= MAX_PLY`. **Cero ejecuciones medidas.** Siguen llamando a `eval::evaluate_relative`, que sigue compilando |

### 4.3 Las invariantes

**I1 — `acc[ply+1]` es función pura de `acc[ply]` y de `(board_antes, mv)`.** Corolario:
**no hay nada que deshacer**. `Undo` no crece, `unmake_move` no se toca, y un `return`
temprano por `ctx.aborted` (`search.rs:1447`, `:1450`) o por corte no puede dejar el estado
inconsistente, porque cada nodo escribe su propia ranura desde la del padre.

**I2 — una actualización por `make_move`, no por recursión.** En `search.rs:1439-1451` un
solo `make_move` alimenta hasta **tres** llamadas a `negamax` (scout con reducción LMR,
re-búsqueda a profundidad completa, re-búsqueda con ventana completa). Enganchar en la
recursión sería un bug silencioso.

**I3 — nodos que comparten ply comparten ranura, y está bien.** `negamax` con `depth == 0`
llama a `quiescence_inner` con el **mismo** ply (`search.rs:1200-1206`), y el razoring
(`search.rs:1243`) llama a quiescencia también con el mismo ply y el mismo tablero. Es la
misma posición.

**I4 — un hilo, un `Context`, un acumulador.** Sin sincronización, sin `Arc`, sin atómicos.

**I5 — refresco solo en la raíz de cada `go`, una vez por hilo.** 36 rasgos × 2
perspectivas × 256 = ~18.400 sumas, del orden de 1 µs. **Durante la búsqueda: cero
refrescos, en ninguna posición, jamás.** Es la consecuencia directa de la invariante de
§3.1, y es la que hace que una jugada de rey cueste lo mismo que una de peón — en un final
donde el 45,9 % de las jugadas son de rey (medido).

**I6 — `board.rs` no cambia**, salvo un refactor de cinco líneas: extraer de
`update_castling_rights` (`board.rs:426`) una función pura

```rust
pub(crate) fn next_castling_rights(rights: CastlingRights, from: Square, to: Square,
                                   moving: Piece) -> CastlingRights
```

que usan tanto `make_move` como `nnue::push`. **No se duplica la lógica de los derechos: se
comparte**, porque duplicarla es exactamente donde estaría el bug.

### 4.4 Coste por tipo de jugada

| jugada | rasgos que cambian / perspectiva | columnas |
|---|---|---|
| tranquila | 2 | 4 |
| captura (al paso incluido) | 3 | 6 |
| promoción | 2, o 3 con captura | 4 ó 6 |
| **jugada de rey** | **2** | **4** |
| enroque | 4 (rey + torre) | 8 |
| cambio de derechos de enroque | +1 ó +2 | +2 ó +4 |

Media ponderada medida en el árbol (~45–50 % de capturas, porque la quiescencia son casi
todo capturas y `order_moves_in_place` las pone delante en negamax): ~5 columnas.

### 4.5 Las tres reglas de escritura

**No son estilo: son entre un 60 % y un 400 % del rendimiento, y están medidas.** Van en
`Documentacion_tecnica.md` §4 con un comentario grande encima del código, porque no son
evidentes y es lo primero que se pierde en una refactorización sin que falle ningún test.

**R1 — el bucle del acumulador se escribe con `zip` de iteradores, nunca con índices, y la
copia padre→hijo se funde con la actualización.** Indexar cuesta hasta 2,7×; copiar y luego
modificar añade ~10 ns por jugada gratis.

```rust
let (sub, add) = (&net.ft[i_sub], &net.ft[i_add]);   // subrebanadas FUERA del bucle
for (d, ((s, a), b)) in dst.iter_mut().zip(src.iter().zip(add).zip(sub)) {
    *d = *s + *a - *b;
}
```

**R2 — `N` es constante de compilación y las rebanadas son `&[i16; 256]`**, obtenidas con
`try_into().unwrap()`, no `&[i16]`. LLVM tiene que conocer la longitud para desenrollar y
no generar prólogo escalar.

**R3 — la reducción de la salida se escribe como `s += (u as i32) * (w as i32)` con
`u, w: i16`.** Es el patrón exacto que LLVM colapsa en `pmaddwd`. Escribirlo con los
operandos ya en i32 haría emitir `pmulld`, **que es SSE4.1 y este binario no lo tiene**:
LLVM lo emularía con `pmuludq` y barajados, multiplicando el coste por 3–4 sin que fallara
ni un test. Hay que verificar el desensamblado una vez (`--emit asm`, buscar `pmaddwd`,
`pmullw`, `pmaxsw`, `psraw`) y anotarlo en la documentación.

### 4.6 Lo que se descarta aquí

**La actualización perezosa** (anotar el cambio en `make_move` y rehacer la cadena solo si
alguien evalúa). Con 0,927 `make_move` y 0,802 evaluaciones por nodo, el techo del ahorro
es **13,5 %** de una ruta que ya consume el 25 % del presupuesto: 2–3 Elo. A cambio, una
máquina de estados con `computed`/`dirty` que puede desincronizarse en silencio. No
compensa. Si algún día la red crece, se reconsidera con su propio experimento.

---

## 5. Los datos

### 5.1 Por qué un generador nuevo, y por qué no es por velocidad

El banco **no puede** ser la fuente, y no por lentitud: se midió que el 99,4 % de su tiempo
de worker ya está dentro de la búsqueda del motor. Las razones son otras cuatro:

1. **Determinismo.** `main.rs:210-223` lo dice: con control por nodos, un hilo y `Variety`
   apagado, Vigía contra sí mismo da **una sola partida por apertura**. Con
   `vigia-20000.epd` el techo absoluto es 20.000 partidas, para siempre.
2. **Adjudicación.** El 70 % de las partidas del corpus existente terminan por abandono
   (`resign_cp: 900`): apenas hay finales, y la etiqueta de resultado es circular.
3. **Formato.** `parejas.jsonl` es «la única fuente de verdad» de un experimento
   (`run.rs:21-22`); no lleva posiciones y no debe cargar con 1,2 GB de datos.
4. **API.** `search::search()` (`search.rs:770`) devuelve `pv` y `complete`
   (`search.rs:216`), que la vía UCI descarta y que hacen falta para filtrar bien.

### 5.2 El generador

`src/bin/generador.rs`, enlaza la biblioteca `vigia` y llama a `search::search()` **en
proceso**. Un `Board` y una `Tt` por hilo, limpiada por partida.

- **Aperturas: k plies aleatorios desde la posición inicial**, k ∈ [8, 16], PRNG sembrado
  (mismo estilo que `libro::Rng`), filtrando la posición resultante por `|eval| ≤ 200` y
  no en jaque. **No se usa el libro `Probon_Gem`**: con esto el corpus pasa a ser 100 % de
  cosecha propia, que es la condición del proyecto, y de paso se evita heredar el sesgo de
  `|eval HCE| ≤ 90` de la evaluación que se va a jubilar. El rango de k es más ancho de lo
  habitual a propósito: con la correlación medida en §5.5, lo que compra información es la
  diversidad **entre** partidas.
- **Presupuesto por nodos, no por tiempo:** `go nodes 25000`, que gasta de verdad ~14.600
  nodos (el límite blando consume un **57 % estable** del presupuesto en este rango,
  medido) y llega a **profundidad ≈ 7,5**.
- **Sin adjudicación por abandono.** Hasta mate, ahogado, repetición, 50 jugadas o 300
  plies. Es la única forma de tener finales de verdad y una etiqueta WDL no circular.
- **Un fichero por hilo con su propia sub-semilla**, para que «misma semilla → mismo corpus
  byte a byte» siga siendo cierto con hilos.
- Puede usar los **32 hilos lógicos**: a nodos fijos el resultado no depende del reloj, así
  que el argumento del SMT que obliga a 16 workers en el banco no aplica. Cuánto sube de
  verdad con 32 hilos y 32 TT compitiendo por L3 está **por medir** (prueba de 2 minutos).
  **Hay que avisar al usuario antes de ocupar la máquina 10 horas.**

#### Cambios al ejecutarlo (fase 2)

El generador existe (`generador datos`) y difiere de lo escrito arriba en cuatro
puntos, todos deliberados:

- **Las aperturas salen de `C:/Ajedrez/Probon_Gem/apertura.txt`, no de jugadas al
  azar.** Son 3.825.105 posiciones distintas y realistas, desde la segunda jugada
  hasta el medio juego, así que cada partida del corpus empieza en una distinta.
  Lo que da información a la red, según la medición de §5.5, es la variedad entre
  partidas; y 8–16 jugadas al azar producen aperturas que no aparecen en ninguna
  partida real, capacidad gastada en posiciones que la red nunca va a ver. Se
  consideraron también los libros Polyglot del usuario: un libro con pesos repite
  sus líneas principales, que es lo contrario de lo que se busca. Las etiquetas
  siguen siendo búsquedas de Vigía; la apertura solo decide dónde empieza la
  partida. Sobre cada posición se juegan además entre 0 y 2 jugadas al azar.
- **Se excluyen las 22.247 posiciones de los libros del banco.** Se muestrearon de
  ese mismo fichero, y entrenar sobre ellas sería entrenar sobre las aperturas con
  las que después se mide la red.
- **Una apertura que la primera búsqueda ya ve decidida (|puntuación| ≥ 400) se
  descarta**, en lugar de filtrar por la evaluación estática, que habría heredado
  el sesgo de la HCE.
- **Las partidas terminan con `vigia::rules`**, las mismas reglas que usa el
  árbitro del banco, subidas a la biblioteca para eso. Sin adjudicación, con tope
  de 300 plies que cuenta como tablas.

Cada hilo usa su propia subsemilla y una tabla de transposición de 16 MB que se
vacía al empezar cada partida, así que **la misma semilla da los mismos ficheros
byte a byte**: comprobado con dos tandas de 2 hilos. Ritmo medido en el primer
trozo: **1,84 M de registros por hora con 8 hilos**, 230.000 por hilo (el plan
estimaba 226.000).

### 5.3 Formato

Registro fijo de **32 bytes**, `memmap` desde el entrenador:

```
u64  ocupadas    8 B   bitboard de casillas ocupadas
u8   piezas[16] 16 B   un nibble por casilla ocupada, en orden de bit ascendente
                       0..5 = P N B R Q K blancas, 8..13 = negras
u8   meta        1 B   bit0 turno, bits1-4 enroques, bit5 hay al paso
u8   ep          1 B   casilla al paso, o 64
i16  cp          2 B   puntuación de búsqueda, punto de vista del que mueve
u16  mejor       2 B   la jugada, codificada como el Move del motor
u8   resultado   1 B   0 pierde / 1 tablas / 2 gana, el que mueve
u8   ply         1 B   ply dentro de la partida, saturado a 255
```

Autocontenido (no hay que re-jugar nada), alineado, 2,2× más compacto que FEN en texto.
Cabecera de 128 B con `magic`, versión, **sha256 del binario generador**, presupuesto de
nodos, semilla y número de registros — la doctrina de `manifiesto.json` aplicada al corpus.

El byte de `resultado` va desde el principio aunque λ = 1,0 (§5.6): cuesta 1 B y evita
regenerar 1,2 GB para probar el otro λ.

#### Cambios al ejecutarlo (fase 2)

- **Dos bits nuevos en `meta`**: el 6 marca que el que mueve está en jaque y el 7
  que la búsqueda no terminó. Son dos filtros de §5.4 que no se pueden deducir
  después sin reimplementar ajedrez, y el entrenador no reimplementa ajedrez.
- **Todo se escribe en bruto y se filtra al entrenar** (`tools/nnue/dataset.py`),
  para que probar otro filtro no obligue a regenerar horas de datos.
- La jugada va como `origen | destino << 6 | tipo << 12`, con el tipo de
  `MoveFlag`; y el anexo `hce-NN.bin` lleva la escala de final (0 ó 64, que es lo
  único posible con los amortiguadores apagados) y el número de piezas. Una
  aserción en compilación rompe el build si alguien activa los amortiguadores sin
  revisar ese contrato.
- Cabecera `VIGIADT1` de 128 bytes: versión, nodos, semilla, hilo, sha256 del
  fichero de aperturas, sha256 del generador y número de registros.

El formato está contrastado con **dos oráculos independientes** sobre un corpus de
humo: Python reconstruye desde los registros exactamente las posiciones que
escribió Rust, y las marcas del registro —jugada legal, captura, promoción, en
jaque— coinciden con **python-chess**, otra implementación de las reglas, que se
usa solo como oráculo de test.

### 5.4 Filtrado

Porcentajes **medidos** sobre las 1.179.400 posiciones del corpus existente. Se descarta la
posición si:

| regla | coste | por qué |
|---|---|---|
| el que mueve está en jaque | 9,17 % | la red nunca se consulta en jaque (`search.rs:1219`, `:1649`) |
| la mejor jugada es captura | 21,82 % | la puntuación viene del final de una secuencia táctica, no de esta posición |
| la mejor jugada es promoción | 0,30 % | ídem |
| la puntuación es de mate | 0,77 % | se tiran, no se recortan: los mates los encuentra la búsqueda |
| `\|cp\| ≥ 1500` | 0,81 % | por encima, el gradiente solo enseña a distinguir «ganado» de «muy ganado» |
| aplica el atajo KPK | 0,051 % | ya está resuelto; no se gasta capacidad en ello |
| `endgame_scale_factor == 0` | ~0 % | el gradiente sería idénticamente cero |
| `SearchResult::complete == false` | ~0 % | gratis de comprobar |
| plies de la apertura aleatoria | todos | no están etiquetados |

**Supervivencia: 69,16 %.** Hay que presupuestar en bruto y contar en útiles.

**Medido sobre el corpus nuevo** (12.000.572 registros de la semilla 1): la
supervivencia es del **60,05 %**, no del 69,16 %.

| filtro | corpus viejo (adjudicado) | corpus nuevo |
|---|---|---|
| la mejor jugada es captura | 21,82 % | 20,23 % |
| el que mueve está en jaque | 9,17 % | 12,23 % |
| la puntuación es de mate | 0,77 % | **7,17 %** |
| `\|cp\| ≥ 1500` | 0,81 % | **5,17 %** |
| aplica el atajo KPK | 0,051 % | **1,24 %** |
| la mejor jugada es promoción | 0,30 % | 0,75 % |
| búsqueda incompleta | ~0 % | 0,00002 % (2 registros) |
| `endgame_scale_factor == 0` | ~0 % | 0 % |

Los cuatro que se disparan son los mismos cuatro: **las partidas ahora se juegan
hasta el mate**. El corpus viejo cortaba por adjudicación en cuanto una partida
estaba decidida, y por eso casi no tenía mates, ni finales de rey y peón, ni
puntuaciones enormes. Hay que presupuestar el **60 %**.

Sobre los tres trozos (36,0 M): **60,01 %**, y cada filtro en la misma cifra al
segundo decimal que en el primero. El corpus es homogéneo, como debe ser: tres
tandas iguales con semillas distintas.

No se filtra por número de piezas ni se tiran finales (son justamente lo escaso), y **no se
submuestrea un ply de cada dos**: la correlación se trata dimensionando, no tirando datos
(§5.5).

### 5.5 Cuántas, y qué valen de verdad

| | |
|---|---|
| ritmo medido | 226.000 pos/h/worker a `nodes 25000` |
| con 16 workers | 3,61 M pos/h |
| objetivo | **36 M brutas → 25 M filtradas** |
| **tiempo de máquina** | **≈ 10 horas, 16 workers** |
| tamaño en disco | 1,15 GB (hay 1,2 TB libres) |

**Pero 25 M posiciones no son 25 M muestras independientes, y eso está medido.** Sobre el
corpus existente:

- autocorrelación de `cp` dentro de una partida, desfase 2 plies: **ρ = 0,980**; restringido
  a `|cp| < 300`, que es donde vive el gradiente útil: **ρ = 0,909**;
- longitud de decorrelación (ρᵏ = 0,5): **14,5 plies**;
- descomposición de varianza: desviación típica total 384,9 cp, **intra-partida 270,4 cp**
  → **el 50,7 % de la varianza de la etiqueta está entre partidas, no dentro**.

Con ~278.000 partidas de ~90 plies útiles y una etiqueta independiente cada ~14,5 plies,
salen unas 8 extracciones independientes por partida: **≈ 2,2 millones de muestras
efectivas** (banda 1,7–2,8 M). Un factor de 10 respecto del número nominal.

Ese es el número con el que se dimensiona la red, y es lo que fija N=256:

| arquitectura | parámetros | muestras efectivas / parámetro |
|---|---|---|
| 768 → 2×128 → 1 | 98.689 | 22,3 — capacidad pagada sin usar |
| **Atalaya-256** | **201.992** | **10,9 — el punto elegido** |
| 772 → 2×384 → 8 cubos | 302.984 | 7,3 — justo |
| HalfKP 4 cubos → 2×256 | 656.129 | 3,4 (y **1,8** en su cubo más pobre) |

Cobertura de rasgos, que aquí no es el cuello: 25 M × 2 perspectivas × ~24 piezas / 772
filas = **1,55 M activaciones por fila**. Hasta el rasgo más raro se ve decenas de miles de
veces.

**La autocorrelación hay que volver a medirla sobre el corpus nuevo** (el ρ = 0,909 se midió
en un corpus con el 70 % de partidas adjudicadas): **POR MEDIR**, en la fase 2. Y la
longitud de partida sin adjudicación se estima en 125–140 plies frente a los 97,1 del
banco: **POR MEDIR** también, y afecta directamente a las 10 horas.

**Medido en la fase 2** (semilla 1: 12,00 M brutas, 7,21 M útiles, 104.972
partidas):

| | corpus viejo (adjudicado) | corpus nuevo |
|---|---|---|
| ρ a desfase 2 con `\|cp\| < 300` | 0,909 | **0,885** |
| longitud de decorrelación | 14,5 plies | **11,4 plies** |
| desviación típica de la etiqueta útil | 384,9 cp | **497,3 cp** |
| … intra-partida | 270,4 cp | **494,2 cp** |
| varianza que está entre partidas | 50,7 % | **1,2 %** |
| longitud de partida | 97,1 plies | **114,3 plies** (mediana 109) |

Las dos primeras filas salen algo mejor de lo previsto. Las otras dicen una cosa
que no se esperaba: **sin adjudicación, una sola partida recorre casi todo el
rango de puntuaciones**, así que la variedad ya no está entre partidas sino
dentro de cada una. No cambia la cuenta de muestras efectivas, que solo usa la
longitud de decorrelación, pero sí explica por qué la etiqueta dispersa más.

**Muestras efectivas: 633.000 por trozo de 12 M**, o sea 0,053 por posición
jugada. El dimensionado de §5.5 pide ~2,2 M (10,9 por parámetro):

| trozos de 12 M | brutas | útiles | efectivas | por parámetro |
|---|---|---|---|---|
| 1 | 12 M | 7,2 M | 0,63 M | 3,1 |
| **3** | 36 M | 21,6 M | **1,9 M** | **9,4** |
| 4 | 48 M | 28,8 M | 2,5 M | 12,5 |

**Acortar las partidas no compra nada**, contra lo que decía el criterio 4 de la
fase 2: las muestras efectivas son útiles ÷ longitud de decorrelación, y al
acortar bajan las dos a la vez; la cuenta da 0,053 por posición jugada se corte
donde se corte. Y las aperturas más variadas que traería tampoco: la variedad
entre partidas es el 1,2 % de la varianza. El único camino es **más trozos**.

**Hecho, y la predicción se cumple.** Los tres trozos suman 36.001.827 brutas,
21.603.026 útiles y **1.898.466 muestras efectivas: 9,40 por parámetro**, contra
las 10,9 con las que se eligió N = 256. ρ y la longitud de decorrelación no se
mueven al juntar los tres (0,885 y 11,4 plies).

### 5.6 Objetivo de entrenamiento

**λ = 1,0: solo puntuación, peso cero al resultado de partida en la primera red.**

```
pred_cp = (endgame_scale_factor(board, red_cp) / 64) · red_cp
pérdida = media[ ( σ(pred_cp / K) − σ(cp_etiqueta / K) )² ]
```

Tres decisiones, todas deliberadas:

1. **La pérdida se calcula en el espacio [0,1] de puntuación esperada, no en
   centipeones.** En cp, un error de 50 trata igual una posición igualada —donde se decide
   la partida— que una con +900, donde da igual.
2. **La escala de final entra DENTRO de la pasada hacia delante**, como factor constante
   conocido por posición. La red se entrena **sabiendo** que la van a amortiguar y puede
   compensar. La alternativa —dividir la etiqueta por la escala— amplificaría el ruido por
   16 en las posiciones de escala 4/64.
3. **Peso cero al WDL en v1**, y aquí el proyecto se aparta del guion habitual con un
   número: en el corpus con adjudicación, el resultado **es** la evaluación, y se ve en que
   el ajuste de `σ(cp/K)` da K = 140 cp, una curva absurdamente empinada. Con el generador
   nuevo (sin adjudicación) el WDL sí es información independiente: λ = 0,75 es el
   experimento v1.2, no el v1.

**K se ajusta sobre el corpus nuevo.** No se hereda el 140, que es un artefacto.

**Medido:** **K = 160,7** sobre 5 M etiquetas útiles de la semilla 1 (error
0,08012; con K = 200 sale 0,08101 y con K = 100, 0,08410 — el mínimo es plano).
El 140 heredado no estaba lejos en valor, pero venía de un corpus donde el
resultado *era* la evaluación; este sale de partidas jugadas hasta el final, con
un 25,1 % de tablas entre las posiciones útiles.

### 5.7 Validación que ya está pagada

Las **815.632 posiciones supervivientes** de
`banco/resultados/028-velocidad-en-elo-estimacion/parejas.jsonl`, etiquetadas a
**profundidad media 10,66** —más profundas de lo que va a producir el generador—, se usan
como *holdout* y **nunca entran en el entrenamiento**.

**Con una advertencia que hay que respetar:** ese holdout está etiquetado con búsquedas
cuya evaluación en las hojas **es la HCE**. El error de la HCE y el de la etiqueta están
correlacionados por construcción, así que la HCE tiene ventaja estructural en esa métrica.

- **Sirve** para comparar redes candidatas entre sí, y para detectar si d ≈ 7,5 es el
  cuello (si el error sobre las posiciones a d=10,66 se estanca muy por encima del de
  entrenamiento, hay que subir el presupuesto de nodos del generador).
- **NO sirve** para vetar el SPRT. Una red genuinamente mejor puede salir peor en esa
  métrica. El veredicto lo da `banco sprt` y nada más.

---

## 6. El entrenador

Vive en **`tools/nnue/`**, en Python, con el precedente de `tools/calibration/`. **Esto no
viola la regla de cero dependencias**: la regla es del crate Rust (`[dependencies]` vacío en
`Cargo.toml`, y así sigue). `tools/` ya es Python hoy.

| fichero | qué hace |
|---|---|
| `dataset.py` | `memmap` del binario de 32 B, decodifica a listas de índices de rasgos, barajado por permutación de índices |
| `model.py` | `EmbeddingBag` disperso 772→256, SCReLU, cubos de salida, escala de final en la pasada hacia delante |
| `fit_k.py` | ajusta K sobre el corpus por mínimos cuadrados sobre `σ(cp/K)` |
| `buckets.py` | mide la ocupación por número de piezas y **emite la tabla `CUBO[33]` y `n_cubos`**, fusionando los cubos por debajo de 1 M de posiciones |
| `train.py` | Adam, lote 16.384, coseno, ~30 épocas, **recorte de pesos del FT a `\|w\| ≤ 6,0` tras cada paso** (garantiza T1) |
| `quantize.py` | cuantiza a i16, verifica T1 y T2 en Python **antes** de exportar, y escribe el `.bin` con su cabecera |
| `golden.py` | genera y verifica el vector dorado (§7) |
| `material_net.py` | construye a mano la red de solo material de la fase 1, sin entrenar nada |

La red es tan pequeña que **entrena en CPU en horas, no días**: se pueden probar tres
semillas y quedarse con la mejor por validación antes de gastar una sola partida.

**Python nunca reimplementa ajedrez.** Las posiciones vienen decodificadas del binario que
escribió Rust; la escala de final viene en un fichero paralelo `hce.bin` de 4 B por
registro (escala `u8` + cubo `u8` + 2 B de reserva) que escribe el **mismo generador en
Rust**, con el código del motor. Es la misma doctrina que el manifiesto del banco.

Exportación: `nets/atalaya-256-<sha8>.bin`, 404.128 B, que se empotra con `include_bytes!`.
**La red va empotrada y no en fichero aparte** porque la firma de todo experimento del banco
incluye el sha del binario: con la red fuera, dos tandas podrían ser «el mismo binario» con
redes distintas, rompiendo en silencio la reproducibilidad.

---

### 6.1 Lo que existe (fase 3, antes de tener corpus)

| fichero | qué hace |
|---|---|
| `dataset.py` | lee el corpus con `memmap`, decodifica a rasgos de cada perspectiva y aplica los filtros de §5.4 como máscaras |
| `corpus_stats.py` | los criterios de la fase 2: supervivencia a cada filtro, tabla `CUBO[33]` con la regla del millón, partidas, autocorrelación y muestras efectivas (sustituye al `buckets.py` previsto) |
| `fit_k.py` | ajusta K sobre el corpus, sin heredar el 140 del corpus del banco |
| `train.py` | el modelo y el entrenamiento en PyTorch sobre la GPU (RTX 5060 Ti, 16 GB); validación por partidas enteras |
| `quantize.py` | escribe `atalaya-256-<sha8>.bin` con las cotas comprobadas y mide el error de cuantización |
| `check_red.py` | con `generador evaluar`: la red entrenada en el motor contra Python, entero a entero, y su escala frente a la HCE |

**El modelo flotante es el motor sin redondear.** El acumulador vive en unidades
de activación (1,0 son 127 enteros) y la salida es directamente centipeones, así
que cuantizar es multiplicar y redondear. Los recortes de pesos (`|w_ft| ≤ 6`,
`|w_out| ≤ 127` cp) son las cotas T1 y T2 garantizadas por construcción.

Probado de punta a punta sobre un corpus de humo: 3 épocas en la GPU, error de
cuantización de 0,29 cp de media y 0,9 cp de máximo, y la red cuantizada, con 7
cubos y tabla real, idéntica entre el motor y Python en 200 posiciones.

**`generador evaluar` cierra un hueco del §7**: el vector dorado ata Rust y Python
con una red aleatoria de 8 cubos, y una red entrenada trae menos cubos y una tabla
de cubos de verdad, que el motor no había comprobado nunca.

### 6.2 La red que se guarda es la de la última época (corrección, 0.32)

Hasta aquí `train.py` guardaba la de **mejor validación**. Lo que lo destapó fue
λ = 0,50: la validación toca fondo en la **época 8** (0,026557) y la 60 vale
0,026877, así que el fichero guardado era una red de ocho épocas haciéndose pasar
por «el candidato de λ = 0,50».

**Y aquí se escribió que con λ = 1 la regla era inofensiva, «porque la curva es
monótona y el mínimo cae en la época 56 o 57 de 60». Es falso, y lo desmienten
los propios ficheros del repositorio**, que llevan dentro la época que se guardó:

| checkpoint | época guardada | dónde se usó |
|---|---:|---|
| `atalaya-v1-e60.pt` | 53 de 60 | la red de 0.29 y 0.30 |
| `atalaya-v1v2.pt` | 57 de 60 | **la red de 0.31** |
| `atalaya-v2-prefijo.pt` | 56 de 60 | candidato de `031-profundidad-de-etiqueta` |
| `atalaya-v2-control.pt` | 60 de 60 | base de `031-profundidad-de-etiqueta` |
| `atalaya-v2.pt` | **30 de 60** | base de `031-v2-contra-v1v2` |
| `atalaya-v1v2-lam0.75.pt` | 60 de 60 | la red candidata de 0.32 |

La curva de λ = 1 tampoco es monótona, y en el entrenamiento de v2 la regla vieja
mordió de verdad: guardó **la mitad del entrenamiento**. Las consecuencias sobre
lo ya publicado están anotadas en §7.1; ninguna cambia una decisión, pero sí
cambian a qué se atribuye lo medido.

Un número que también estaba mal y se corrige aquí: la diferencia entre la época
8 y la 60 de λ = 0,50 se escribió como «un 0,1 % peor» y es **un 1,2 %**
(0,026877 / 0,026557 = 1,0121). No es «ruido que aplana la curva»: comparado con
el 0,007 % que separa la época 56 de la 60 en λ = 0,75, lo que se ve en λ = 0,50
es un sobreajuste real al resultado desde muy pronto. Lo que sostiene la decisión
de guardar la última época no es esa cifra, sino las dos razones que quedan en
pie: el plan de tasa de aprendizaje termina en 0, y una regla que puede devolver
la época 8 —o la 30— no sirve para comparar dos redes entre sí.

Desde ahora se guarda **la última época terminada**, que además deja algo
utilizable si la tanda se corta; el plan de tasa de aprendizaje termina en 0, así
que esa es la red que se ha entrenado. La mejor validación se sigue imprimiendo,
con su época, pero no elige.

Y cada época se imprimen **dos** validaciones: la del objetivo con el λ pedido,
que no es comparable entre λ distintos porque cada una mide contra otra cosa, y
la de puntuación pura (λ = 1), que sí lo es. Ninguna de las dos aprueba una red:
eso es del banco.

## 7. Cómo se verifica que Rust y el entrenador coinciden

**Esta es la sección que evita que el proyecto se rompa en silencio.** Todos los tests que
suelen escribirse —incremental contra refresco, cotas, antisimetría, guardián de
velocidad— miran a Rust consigo mismo, y **ninguno falla si Python y Rust discrepan en el
índice de un rasgo, en el orden de los pesos, en el signo de la perspectiva o en el
redondeo**. Un desacuerdo de índice entrena una red perfectamente buena sobre un espacio de
rasgos permutado: no desborda, no va lenta, `banco velocidad` sale impecable, y lo único
que pasa es que el motor juega peor. Eso solo se ve tras 12.000 partidas.

### 7.1 El vector dorado, cruzado en los dos sentidos

Un solo oráculo no basta: si Python genera las FEN, los índices *y* el resultado esperado,
un bug de Python produce un dorado con el mismo bug y el test pasa. Hay que romper eso
haciendo que **cada lado sea oráculo de la mitad que el otro no puede fabricar**:

**Sentido A — los índices los manda Rust.** El generador escribe
`tools/nnue/indices.txt`: 4.096 FEN y, por cada una, la **lista ordenada de índices activos
de las dos perspectivas**. `dataset.py` los recalcula con su propio código y **falla si
difiere una sola entrada**. Esto ancla el índice de rasgos, la orientación de la
perspectiva, el rasgo relativo propio/ajeno y los cuatro de enroque.

**Sentido B — el forward lo manda Python.** `golden.py` escribe
`tools/nnue/golden.bin`: las mismas 4.096 FEN y, por cada una, **la suma `S` en i32 antes
de escalar, el índice de cubo, y el `cp` final**. Un test de `cargo test --release` lo lee
y exige **igualdad exacta, entero a entero**. Esto ancla la disposición
`[rasgo][dimensión]`, el endianismo de `include_bytes!`, el clamp, el `>>4`, la elección de
cubo vía `CUBO[33]`, la división final con su redondeo y el signo.

Las 4.096 FEN no son aleatorias: **1.024 de partidas reales del libro, 1.024 de finales con
≤ 8 piezas, 512 con derechos de enroque intactos, 512 con al paso disponible, 512 con
promoción inminente, 512 con material muy desequilibrado** y las 32 del suite de humo. Si
un caso no está en el dorado, no está probado.

### 7.2 Los demás tests, todos obligatorios

| test | qué atrapa |
|---|---|
| `acumulador_incremental_coincide_con_refresco` | árbol tipo perft de profundidad 4 desde 8 FEN que incluyan enroque, al paso, promoción, promoción con captura y pérdida de derechos; comparar `acc[ply]` contra `refresh()` **en cada nodo**. Atrapa el 100 % de los errores de delta y de signo |
| `red_no_puede_desbordar_el_acumulador` | T1 de §3.5, sobre la red cargada |
| `red_no_puede_desbordar_la_salida` | T2 de §3.5, cubo a cubo |
| `cabecera_coincide_con_la_arquitectura_compilada` | fichero truncado, red de otra topología, `n_cubos` incoherente |
| `sha_de_los_pesos_coincide_con_la_cabecera` | fichero corrupto o desalineado, sin evaluar nada |
| `evaluacion_es_antisimetrica` | `eval(p) == −eval(p espejada en color y fila)` — valida la orientación y el redondeo por división |
| `escala_de_final_se_aplica_a_la_salida_de_la_red` | que el postfiltro no se pierda en un refactor |
| **guardián de velocidad, en el banco** | cronometra `nnue::output` y falla fuera de banda. **La autovectorización no es un contrato**: un `if` dentro del bucle, un bounds check que no se elimine o una subida de rustc pueden multiplicar el coste por 4 sin que falle ni un test funcional |

### 7.3 Herramienta de diagnóstico

Opción UCI **`setoption name UseNNUE value true|false`** (por defecto `true` desde 0.29;
hasta entonces fue `false`, mientras la red no estaba aprobada). Permite medir
HCE contra red **con el mismo binario y el mismo sha**, que es el A/B más limpio posible.
**No rompe la reproducibilidad del banco:** `calcular_firma`
(`src/bin/banco/run.rs:256-268`) firma la configuración normalizada completa, `opciones`
incluidas, junto con los sha de los dos binarios y del libro. Es un caso distinto del de la
red en fichero externo, que sí sería invisible a la firma — por eso aquella va empotrada y
esta opción es legítima.

Y el comando `eval` (`uci.rs:185`): **la última línea sigue siendo, byte a byte,
`Evaluation: {cp} (white side)`**. La exige el test de `uci.rs:694` y, más importante, la
parsea `tools/calibration/calibrate.py:144` con la regex `Evaluation: (-?\d+)` para
comparar contra Stockfish, Obsidian, Berserk y Caissa. El desglose por términos se
sustituye por: camino tomado (`kpk_exact` / `red` / `material insuficiente`), salida cruda
de la red, cubo elegido, factor de escala si ≠ 64, y las dos cifras `HCE:` y `NNUE:`.
**Ganancia colateral: `calibrate.py` se convierte en el banco de regresión de la red contra
cuatro oráculos externos sin escribir una línea nueva.**

---

## 8. El plan por fases

Una fase cada vez. Ninguna empieza hasta que la anterior cumple su criterio.

### Fase 0 — Congelar el libro

`src/bin/banco/libro.rs:239` filtra las posiciones del libro con
`eval::evaluate(&board).abs() > filtros.max_cp`. Si la evaluación cambia, cambia el
conjunto de posiciones → cambia el sha del libro → **cambia la firma del experimento y las
tandas nuevas dejan de ser comparables con las de 0.28**.

`banco/libros/*.epd` ya está versionado en git. **Decisión: no se regenera ninguno de los
cuatro libros nunca más.** Se anota su sha256 en `BancoPruebas.md`.

**Criterio:** los cuatro sha anotados en la documentación. **Coste:** minutos.

---

### Fase 1 — Cerrar el bucle entero con la red más tonta posible

**Esta fase paga el riesgo de integración antes que el de arquitectura, y no gasta ni una
hora de generación de datos ni una época de entrenamiento.**

La red de la fase 1 es una **red de solo material construida a mano**, sin entrenar nada.
Se usan 64 parejas de neuronas simétricas: `a₀ = 64 + d`, `a₁ = 64 − d`, donde `d` es el
material del que mueve menos el del rival en unidades de 64 cp (`W_ft` = ±round(valor/64):
peón 2, caballo 5, alfil 5, torre 8, dama 14, rey 0), y `W_out = ±2.016` en esas 128
neuronas y 0 en las otras 128. Con SCReLU, `t₀ − t₁` es **lineal en `d`**, así que

```
cp ≈ 64 · d       (saturando a ±4.032 cp cuando |d| > 63)
```

Es decir: **para cualquier FEN se sabe a mano qué tiene que devolver la red.** Eso convierte
esta fase en la verificación más barata y más completa que existe: ejercita el formato de
fichero, `include_bytes!`, el cargador, las dos cotas, el acumulador incremental completo,
la inferencia, la cuantización, el vector dorado en los dos sentidos, `uci eval`,
`banco velocidad` y `banco sprt`. Y `material_net.py` la construye en 60 líneas.

**Criterios de aceptación:**

1. `cargo test --release` en verde, **incluidos todos los tests de §7**, y
   `cargo clippy --release --all-targets` en cero avisos.
2. El vector dorado en igualdad exacta en los dos sentidos.
3. `banco velocidad --motor <nuevo> --profundidad 12 --hash 32 --hilos 1`: la ruta de
   evaluación medida (derivada del nps y del reparto) tiene que caer **entre 92 y 172
   ns/nodo**, es decir ±30 % de los 131,9 previstos. *Los nodos NO serán idénticos —la
   evaluación ha cambiado—, así que aquí solo se lee la línea de nps.*

   **Corrección tras ejecutarlo: así no se puede.** Con la red de material, el árbol a
   profundidad 12 tiene la mitad de nodos que el de la HCE, y restar tiempos por nodo
   entre árboles distintos da una ruta de evaluación negativa. La forma buena es duplicar
   a propósito `evaluate` y `push` con nodos idénticos: ≈ 87 ns/nodo, criterio cumplido.
4. Una tanda corta de **256 parejas** contra 0.28 (`nodos: 50000` —no 25.000, que dispara el guardián del banco—, 4 workers, sin parada
   secuencial, `sprt: {elo0: 99.9, elo1: 100.1, alpha: 0.001, beta: 0.001}` como en
   `028-velocidad-en-elo-estimacion.json`): el intervalo de Elo tiene que quedar **entero
   por debajo de −150**, sin una sola jugada ilegal, sin pérdidas por tiempo y sin
   desconexiones. Una red de solo material tiene que perder mucho; si empata o gana, hay un
   bug en algún sitio y hay que encontrarlo antes de seguir.

   **Resultado:** −236,4 Elo, IC 95 % [−271,7, −204,9], en 512 partidas sin una sola
   anomalía (`028-nnue-fase1-material`). Criterio cumplido.

**Si el criterio 3 falla por abajo** (la evaluación cuesta más de 172 ns/nodo), se para
aquí: es el riesgo de caché materializándose, y se resuelve antes de generar un solo dato.

**Coste:** días de trabajo, ~30 minutos de máquina.

---

### Fase 2 — El generador y el corpus

```bash
./target/release/generador.exe --nodos 25000 --workers 16 --semilla <n> \
                              --brutas 36000000 --salida datos/atalaya-v1/
```

**Criterios de aceptación:**

1. **36 M brutas / ≥ 24 M filtradas** en ≤ 12 h con 16 workers. Avisar al usuario y esperar
   a que libere la máquina.
2. **Reproducibilidad:** dos tandas con la misma semilla producen ficheros idénticos byte a
   byte.
3. **Histograma de ocupación por número de piezas publicado**, y `CUBO[33]` / `n_cubos`
   fijados por `buckets.py` con la regla del millón.
4. **Autocorrelación medida sobre el corpus nuevo** (ρ a desfase 2 con `|cp| < 300`,
   longitud de decorrelación, reparto intra/entre partidas) y número de **muestras
   efectivas** anotado. Si sale muy por debajo de 2 M, se sube el número de partidas y se
   acortan (más aperturas aleatorias, menos plies por partida).
5. Longitud media de partida sin adjudicación anotada (la estimación de 125–140 plies era
   **por medir**; aquí se mide).

**Coste:** ~10 h de máquina con 16 workers.

**Cómo se está ejecutando.** El tope es de 8 CPUs, no 16, así que el corpus sale por
trozos de 12 M, uno por semilla, de ~6,5 h cada uno:

```bash
datos/atalaya-v1/generador.exe datos --aperturas C:/Ajedrez/Probon_Gem/apertura.txt \
    --excluir banco/libros --posiciones 12000000 --semilla 1 --hilos 8 \
    --nodos 25000 --salida datos/atalaya-v1/semilla-1
```

Tres semillas dan las 36 M brutas del criterio 1; las 24 M filtradas piden una cuarta,
porque la supervivencia real es del 60 % y no del 69 % (§5.4). El ejecutable se copia al directorio del
corpus antes de lanzar: su sha256 va en cada cabecera, y tiene que ser el de un fichero que
no cambie al recompilar. Los criterios 3 a 5 se miden ya sobre el primer trozo con
`tools/nnue/corpus_stats.py`; si las muestras efectivas salen muy por debajo de lo
previsto, se sabe antes de gastar los otros dos.

**Resultado del primer trozo (semilla 1, 12 M, 8 hilos, 6,43 h):**

1. **Volumen y ritmo:** 12.000.572 registros en 104.972 partidas, 233.457 por
   hora y hilo (el plan estimaba 226.000), 9.129 aperturas descartadas por venir
   ya decididas. Supervivencia 60,05 % → **7,21 M útiles**. Con esta
   supervivencia, las 24 M filtradas del criterio piden **cuatro** trozos, no
   tres; tres bastan para las ~1,9 M efectivas con las que se dimensionó la red.
2. **Reproducibilidad:** ✅, comprobada antes de lanzar con dos tandas de 2 hilos.
3. **Cubos:** con la regla del millón salen **4 cubos**, no 8:
   `CUBO[33] = [0]*9 + [1]*8 + [2]*8 + [3]*8`, o sea ≤ 8, 9–16, 17–24 y 25–32
   piezas. La ocupación por número de piezas es plana (130 k–380 k por valor) con
   un diente de sierra entre pares e impares —las recapturas: un número impar de
   piezas suele durar una jugada— y los picos esperables en 30 y 28.
4. **Autocorrelación y muestras efectivas:** en la tabla de §5.5. ρ = 0,885,
   decorrelación 11,4 plies, **633 k efectivas por trozo**.
5. **Longitud de partida sin adjudicación: 114,3 plies** (mediana 109, tope 300).
   Queda por debajo de la horquilla estimada (125–140) y por encima de los 97,1
   del banco. Ganan blancas el 38,8 %, negras el 38,5 %, tablas el 22,7 %.

**Resultado del corpus completo (3 trozos, 36 M, 18,95 h de máquina con 8 hilos):**

- **Criterio 1, volumen:** 36.001.827 brutas → **21.603.026 útiles**. El criterio
  pedía ≥ 24 M filtradas y se queda en 21,6 M. Se da por bueno: el número con el
  que se dimensionó la red no es el nominal sino el de **muestras efectivas**, y
  ese sí llega (1,90 M, 9,40 por parámetro). Un cuarto trozo compraría 12,5 por
  parámetro a cambio de 6,3 h; se deja en reserva por si la fase 5 dice que el
  cuello es la capacidad.
- **Criterio 2, reproducibilidad:** ya comprobada antes de generar.
- **Criterio 3, cubos: con 36 M la regla del millón ya no fusiona ninguno** y
  salen los **8 cubos** del reparto por defecto (≤ 4, 5–8, … , 29–32 piezas). Con
  12 M salían 4. La tabla viaja en la cabecera de la red, así que el número de
  cubos es un dato del corpus y no del código: cambiar de corpus puede cambiarlo,
  y el motor se entera al cargar.
- **Criterio 4: 1.898.466 muestras efectivas**, 9,40 por parámetro.
- **Criterio 5: 314.931 partidas** de 114,3 plies de media. Ganan blancas el
  38,9 %, negras el 38,4 %, tablas el 22,7 %.

Y la constante del entrenador: **K = 160,7** sobre 10 M etiquetas útiles, la misma
que daba el primer trozo solo.

---

### Fase 3 — Entrenar, cuantizar, exportar

**Criterios de aceptación:**

1. Vector dorado en verde, en los dos sentidos, con la red entrenada.
2. T1 y T2 verificados en Python antes de exportar y en Rust al cargar.
3. **Histograma de escala:** la desviación típica de `|cp|` de la red sobre las 2.000
   posiciones del libro congelado tiene que estar **dentro del ±15 %** de la de la HCE. Si
   se desvía, se corrige con **una sola constante de ganancia**, nunca re-sintonizando los
   ocho márgenes de poda.

   **Corrección antes de ejecutarlo:** sobre posiciones del corpus, no sobre el libro
   congelado. Ese libro se seleccionó con |eval HCE| ≤ 90, que comprime a propósito la
   dispersión de la HCE y haría la comparación injusta. `check_red.py` mide la razón de
   desviaciones típicas con |eval| < 1500.
4. **Referencia** (no veto, §5.7): MSE en espacio sigmoide sobre las 815.632 posiciones a
   d = 10,66, comparada con la de la HCE y entre las tres semillas entrenadas. Si el error
   sobre el holdout profundo se estanca muy por encima del de entrenamiento, el cuello es la
   profundidad de etiqueta y se anota para la fase 7.

**Coste:** horas de CPU, sin ocupar la máquina entera.

**Resultado (red `atalaya-256-6f8033fc`, 60 épocas a `lr` 2e-3 sobre los 36 M):**

- **Criterio 1, el vector dorado con la red entrenada:** el motor y Python dan los
  mismos enteros en **2.000 posiciones del corpus** (`generador evaluar` +
  `check_red.py`). Cubre lo que el dorado con red aleatoria no cubre: una tabla de
  cubos de verdad. El dorado clásico sigue en verde dentro de `cargo test`.
- **Criterio 2, T1 y T2:** comprobados en Python antes de escribir el fichero
  (`quantize.py` → `netfmt.check_bounds`) y otra vez en Rust al cargar. Error de
  cuantización: 0,80 cp de media, 4,2 de máximo.
- **Criterio 3, la escala:** razón **1,044** frente a la HCE, dentro del ±15 %. No
  hace falta constante de ganancia ni resintonizar ningún margen de poda. Medido
  sobre posiciones del corpus, por la corrección de arriba.
- **Criterio 4, la referencia:** sobre 20.000 posiciones de **partidas apartadas**,
  la red recorta el **22,9 %** de la pérdida de la HCE (0,008612 contra 0,011168;
  error medio 96,4 cp contra 108,1). Se mide con `tools/nnue/holdout.py`, que
  reproduce el reparto de validación del entrenador, en vez de con las 815.632
  posiciones a d = 10,66 que decía el plan: ese holdout profundo es del corpus
  viejo, con otras etiquetas y otro origen, y comparar contra él habría mezclado
  dos preguntas.

**Cuánto entrenamiento:** cuatro combinaciones, comparadas por pérdida de
validación y por el recorte sobre la HCE. 30 épocas a 1e-3 → 0,008934 (20,7 %);
60 a 1e-3 → 0,008770 (21,5 %); 30 a 2e-3 → 0,008774 (22,1 %); **60 a 2e-3 →
0,008750 (22,9 %)**. Que doblar la tasa iguale a doblar las épocas dice que el
cuello eran los pasos de optimización; que a partir de ahí sean décimas, que pasó
a serlo el corpus.

**Coste real:** 45 min de GPU por entrenamiento, minutos para cuantizar y
comprobar.

---

### Fase 4 — El coste real, antes de gastar partidas

```bash
./target/release/banco.exe velocidad --motor target/release/vigia.exe \
                           --contra "Release/Vigia 0.28.exe" \
                           --profundidad 12 --hash 32 --hilos 1
```

**Criterio:** nps **no peor** que el de 0.28. El presupuesto dice +20,8 %; el criterio duro
es 0 %, que corresponde a un factor 3 de error en el acumulador. Si cae por debajo, se para
y se diagnostica: no hace falta gastar 12.000 partidas para saber que va a perder.

Se repite con `--hilos 16` para ver el efecto de la presión de caché con los 16 procesos
que va a haber en el SPRT — ese número está **por medir** y esta es la primera vez que
existe.

**Coste:** minutos.

**Resultado:** el criterio se cumple con holgura, y por el lado bueno: la red no
cuesta nps, los **da**.

| | nodos/segundo del candidato | de 0.28 | razón |
|---|---|---|---|
| máquina libre | 2.097.758 | 1.375.373 | **1,53×** |
| ocho medidas a la vez | ~1.756.000 | ~1.289.000 | **1,36×** |

El presupuesto de §2 daba por bueno hasta −20,8 % y el criterio duro era 0 %.
La explicación está medida desde la fase 1: la ruta de la red cuesta ~87 ns por
nodo y la HCE ~248.

**Corrección al método.** El plan mandaba repetir con `--hilos 16` «para ver el
efecto de la presión de caché con los 16 procesos que va a haber en el SPRT», y
esa opción no hace eso: fija la opción UCI `Threads` del motor, y Vigía no tiene
búsqueda paralela. La presión de caché se mide como se ha hecho arriba, lanzando
**ocho comparaciones simultáneas**, que es la carga real de la tanda. Y el efecto
existe: la ventaja baja de 1,53× a 1,36×, porque los 404 KB de pesos compiten por
la caché y la HCE, que casi no toca memoria, sufre menos.

**Y esa velocidad se convierte en profundidad:** +0,60 plies de media en partidas
de verdad (`tools/analiza_tanda.py` sobre la sonda), contra los +0,52 que predice
el factor de ramificación 1,71 para un +36 %.

#### Una sonda antes de la fase 5, que el plan no preveía

128 parejas a `movetime` 100 ms (`029-atalaya-sonda`), 12 minutos con 4 CPUs:
**+200 Elo** [+157, +250], 168-53-35, **cero partidas anómalas**. No decide nada
—el intervalo es ancho a propósito— pero separa «esto va bien» de «esto está
roto» antes de pedir horas de máquina, y de paso da la estimación con la que se
dimensiona la tanda de la cifra.

---

### Fase 5 — SPRT-A: la aprobación

Con la escala de final **limitada a {0, 64}**: red + KPK + material insuficiente, sin los
amortiguadores 4/12/16.

```bash
./target/release/banco.exe sprt --config banco/configs/029-atalaya-256-A.json
```

```json
"candidato": { "nombre": "0.29-atalaya", "ruta": "target/release/vigia.exe",
               "opciones": { "Hash": 32, "Threads": 1 } },
"base":      { "nombre": "0.28", "ruta": "Release/Vigia 0.28.exe",
               "opciones": { "Hash": 32, "Threads": 1 } },
"libro":     { "fichero": "banco/libros/vigia-20000.epd", "semilla": <nueva>, "barajar": true },
"busqueda":  { "movetime_ms": 100 },
"partidas":  { "max_parejas": 4000, "workers": <lo que autorice el usuario, ≤16> },
"sprt":      { "elo0": 0.0, "elo1": 5.0, "alpha": 0.05, "beta": 0.05 }
```

**Criterio de aceptación: `acepta_h1`.** Recordatorio de método: `acepta_h0` con estas
hipótesis **no** demuestra que empeore, demuestra que no llega a +5 Elo. Y la magnitud que
reporte una prueba que para al cruzar está sesgada al alza: si hace falta la cifra, es una
tanda aparte con tope fijo.

**Por qué `movetime` y no `nodos`:** el cambio de evaluación mueve los nodos, así que un
control por nodos compararía cosas distintas. Con `movetime` se está midiendo exactamente
lo que interesa, que es fuerza por segundo de reloj — que es donde vive el crédito de +48
Elo.

**Coste:** ~3–6 h con 8 workers.

**Resultado: aprobada.**

| tanda | parejas | veredicto | Elo | IC 95 % |
|---|---|---|---|---|
| `029-atalaya-256-A` (decisión, para al cruzar) | 213 | **`acepta_h1`** | +266,7 (sesgado al alza) | [+231,6, +307,0] |
| `029-atalaya-256-A-estimacion` (tope fijo) | 2.700 | `continuar`, a propósito | **+250,4** | **[+240,6, +260,6]** |

Candidato `Release/Vigia 0.29-atalaya.exe` (sha256 `c721d6aa…`, red
`atalaya-256-6f8033fc`, `UseNNUE=true` en las opciones), base
`Release/Vigia 0.28.exe`, `movetime` 100 ms, 8 workers. En la tanda de la cifra:
3.884 ganadas, 966 tablas y 550 perdidas, pentanomial `[27, 109, 474, 683, 1.407]`.

Tres notas de método, las tres de las que conviene acordarse:

- **Parar al cruzar infló la cifra otra vez**, +16 Elo sobre la de tope fijo. Menos
  que en 0.28 (+37), porque con una ventaja tan grande la prueba cruza con más
  partidas detrás.
- **La sonda (+200) midió otra red**: `atalaya-256-659345d5`, la de 30 épocas,
  jugada desde `target/release/`, un binario que ya no existe. Las dos tandas de
  esta fase son del binario congelado con la red definitiva y son las únicas
  cifras que describen lo que se publica. Los intervalos de las dos redes se
  solapan: no hay evidencia de que una juegue mejor que la otra.
- **«La mejor de cuatro» no está avalada por su diferencia.** Las cuatro corridas
  comparten semilla y muestra de validación, y las tres primeras se separan un
  0,3 %: son indistinguibles. Lo que avala a la elegida es su propia tanda de
  2.700 parejas, no haber quedado primera.

#### Revisión adversarial antes de publicar

Cinco lentes independientes buscaron por qué el resultado podría no ser real
(banco injusto, contaminación entre corpus y libro, motor con la red, entrenador,
cifras mal citadas), y cada sospecha la revisó otro agente intentando refutarla.
**Ninguna explica el resultado.** Lo que salió:

| hallazgo | qué se hizo |
|---|---|
| al tocar `MAX_PLY`, `negamax` y la quiescencia evaluaban con la HCE aunque la red estuviera encendida | **corregido**: pasan por `ctx.evaluate`, con test. Sin efecto medible —esos nodos no aparecen en partidas normales—, pero era el único sitio donde «la red está encendida» era falso |
| `check_red.py muestra` sacaba posiciones de entrenamiento, así que el criterio 3 se midió sobre posiciones que la red había visto | **corregido**: el reparto de validación vive en `dataset.py` y lo usan los tres scripts (reproduce byte a byte el holdout anterior). Medida de nuevo sobre partidas apartadas: razón **1,052**, dentro del ±15 % |
| este plan decía `UseNNUE` «por defecto `true`» (§7.3) cuando era `false` | se vuelve cierto con 0.29, que la enciende por defecto |
| con `movetime`, el banco no vigila el tiempo de cada jugada: no hay guardián como el de `go nodes` | comprobado a mano que no pasó: candidato 99,57 ms por jugada de media y 103 de máximo, base 99,86 y 106. Queda como mejora del banco |
| el banco no comprueba que el motor anuncie las opciones UCI que se le mandan | **corregido** (0.32): el banco guarda los `option name` del saludo y aborta antes de jugar si falta alguna (`opciones_no_anunciadas`, `src/bin/banco/motor.rs`), §8 de `docs/BancoPruebas.md` |
| ~~`tablas_desde_jugada` se compara con el número de jugada del libro, que siempre es 1, así que la adjudicación de tablas casi nunca se dispara~~ | **el hallazgo era falso, y se retira** (0.32). `arbitro.rs` compara `board.fullmove_number` del tablero vivo, que arranca en 1 con la FEN del libro y llega a 40 en la jugada 40 de la partida, que es justo lo que se quería. Y la regla se dispara: **196 de 2.000 partidas** (9,8 %) en `032-lambda-075`, 470 de 5.000 (9,4 %) en `031-atalaya-v2-A-estimacion`. Tocarla cambiaría el criterio de una de cada diez partidas y haría incomparables las tandas ya registradas |
| la exclusión de los libros del banco se anuncia con el tamaño del conjunto, no con un recuento | comprobado que muerde (las claves del libro están literales en `apertura.txt`), y aun con fallo total serían ~1.200 registros de 36 M |

#### La de 0.32, que encontró más de lo previsto

Cuatro lentes —el guardián de opciones del banco, el cambio del entrenador, el
diseño del experimento de λ y la documentación contra el código—, cada hallazgo
revisado después por un escéptico con el encargo de refutarlo. **Ninguna toca el
resultado de λ**, pero dos tocan lo que se había escrito de 0.31:

| hallazgo | qué se hizo |
|---|---|
| el guardián de opciones UCI comparaba los nombres sin distinguir mayúsculas y luego **enviaba la grafía del fichero**, que el motor despacha de forma exacta: `usennue=false` pasaba la comprobación y se perdía igual | **corregido**: se envía la grafía que anunció el motor. Reproducido antes de arreglarlo: la tanda jugaba con la red encendida creyendo medir la HCE |
| el guardián miraba el nombre y no el valor, y en Vigía un `check` se resuelve con `v == "true"`, así que `UseNNUE=True` **apagaba** la red | **corregido**: el banco guarda el `type` de cada opción y valida el valor; la caja se normaliza (`True` → `true`, `AVX2` → `avx2`) porque eso es grafía, no error |
| la red de 0.31 es la **época 57** y la candidata de λ la **60**: no difieren solo en λ | **medido**, no supuesto: el control `032-epocas-de-cola` da +3,3 Elo [−8,6, +15,2] para esas tres épocas |
| `train.py` guardaba la de mejor validación, y aquí se escribió que con λ = 1 eso era inofensivo | **falso**: `atalaya-v2.pt` se quedó en la época 30 de 60 y es la base de `031-v2-contra-v1v2`. Corregido en §6.2 y §7.1; esos +8,7 Elo no separan corpus de entrenamiento |
| «un 0,1 % peor» entre la época 8 y la 60 de λ = 0,50 | son **1,2 %**. Corregido donde se repitió: el plan, el `train.py` y el mensaje del commit |
| el plan daba por pendiente una «mejora del banco» que no lo era: `tablas_desde_jugada` comparada con el número de jugada del libro | **el hallazgo original era falso y se retira**: compara el contador vivo del tablero y adjudica el 9,8 % de las partidas. Tocarlo habría roto la comparabilidad de todo lo medido desde 0.28 |

---

### Fase 6 — SPRT-B: los amortiguadores de final

Activar `PAWNLESS_LONE_MINOR_SCALE` (4/64), `OPPOSITE_COLORED_BISHOPS_SCALE` (12/64) y
`PAWNLESS_SMALL_EDGE_SCALE` (16/64). **Experimento separado, y no por escrúpulo: esas tres
constantes se calibraron al ruido de la HCE.** El 4/64 borra el 94 % de una evaluación que
la red podría estar acertando, y su predicado es candidato en el ~5 % de las posiciones
(medido, y probablemente más en un corpus con finales de verdad).

**Criterio:** `acepta_h1` con `elo0 = 0, elo1 = 5`, **o** `acepta_h1` con
`elo0 = −5, elo1 = 0` si lo que se quiere es certificar que no hacen daño. Si pierde, se
quedan desactivados y la red vive sin ellos.

---

### Fase 7 — La escalera, y **no** iba por profundidad de etiqueta (corregido en 0.31)

Con v1 aprobada y congelada como 0.29, el orden de los experimentos siguientes es, uno cada
vez:

1. **Etiquetas más profundas, no red más ancha.** La señal de entrenamiento es la
   *diferencia* entre lo que ve la búsqueda a d≈7,5 y lo que ve la evaluación estática en
   las hojas. Cuando v1 sustituya a la HCE, esa diferencia se estrecha por construcción:
   v1 ya sabe lo que la búsqueda le enseñó. ~~**Reentrenar con más posiciones a la misma
   profundidad no aporta casi nada.**~~ **Esto se midió y salió al revés**: el volumen es
   justo lo que compró los +64,9 Elo de 0.31 y la profundidad no compró nada medible; el
   resultado está más abajo, en «Resultado (0.31)». Se deja escrito porque era la tesis con
   la que se entró. Y aquí se cobra el crédito de velocidad: con +20,8 % de nps,
   `nodes 50000` cuesta en reloj casi lo que costaba `nodes 40000`.

   **Preparación de la ejecución (antes de 0.31).** `generador datos` tiene la opción
   `--evaluador hce|red`, que queda escrita en la cabecera (bytes 100–103, bit 0), y con
   `hce` —el defecto— sigue escribiendo exactamente lo mismo que el binario del corpus v1
   salvo su propio sha. Profundidad media a nodos fijos sobre 50 posiciones de partidas del
   corpus:

   | evaluador | nodos | profundidad |
   |---|---|---|
   | HCE (etiquetas v1) | 25.000 | 7,64 |
   | red | 25.000 | 7,78 |
   | red | **50.000** | **8,72** |
   | red | 100.000 | 9,80 |

   **Etiquetas de v2: la red a 50.000 nodos**, un ply más que v1 por el doble de nodos.
   Ritmo calibrado con 2 hilos: 162.600 registros por hora por hilo; con 8 hilos y la
   presión de caché que ya se midió en la fase 4, ~150.000.

   Una revisión adversarial del plan cambió tres cosas antes de lanzarlo:

   - **v2 cambia tres cosas a la vez** —el evaluador de las hojas, los nodos y las partidas,
     que las juega otro motor—, así que un `acepta_h1` no diría que el cuello era la
     profundidad, y el siguiente escalón (100.000 nodos) cuesta el doble. Se añade un
     **control**: 6 M con la red a 25.000 nodos y la **misma semilla** que el primer trozo
     de v2, que por construcción arranca sus partidas en las mismas aperturas. Una red
     entrenada con el control contra otra entrenada con los primeros 6 M de ese trozo
     aísla la profundidad.
   - **Entre «solo v2» y «v1+v2» no se elige por pérdida ni por holdout**: los dos se miden
     contra etiquetas de v2, que son el objetivo de «solo v2», y dependen de K. Se decide en
     el banco, con una tanda directa entre las dos redes.
   - **El reparto de validación dependía de la posición del fragmento en la lista**, así que
     con v1+v2 la validación de v2 caía en otras partidas. Ahora depende de su identidad
     (`(semilla − 1) · 8 + hilo`, de la cabecera), que reproduce exactamente el reparto de
     v1: la muestra de holdout de v1 sale idéntica byte a byte. Y un trozo cortado a media
     tanda —un reinicio forzado— ya se puede leer: el generador anota el recuento tras cada
     partida y el lector usa lo que está en los tres sitios.

   Orden de la ventana, con 8 CPUs y sin paradas:

   | paso | qué | tiempo |
   |---|---|---|
   | 1 | v2, semilla 11, 12 M a 50.000 nodos | ~10 h |
   | 2 | control, semilla 11, 6 M a 25.000 nodos | ~3 h |
   | 3 | v2, semillas 12 y 13 | ~20 h |
   | 4 | estadísticas, K y cuatro entrenamientos en la GPU (control, 6 M de v2, v2, v1+v2) | ~3 h |
   | 5 | banco: control contra 6 M de v2; v2 contra v1+v2; la mejor contra 0.30, decisión y cifra | ~4 h |
   | | **total** | **~40 h** |

   **Resultado (0.31): la red mejora +64,9 Elo, y no es por la profundidad.**

   El corpus v2 salió en 27,2 h con 8 CPUs: 36.001.612 posiciones a 50.000 nodos con la
   red, 21,8 M útiles (60,47 %), 302.259 partidas de 119,1 plies, 26,9 % de tablas (v1:
   22,7 %). Y una sorpresa que cambia la lectura de todo lo demás:

   | | v1 (HCE, 25.000 nodos) | v2 (red, 50.000) |
   |---|---|---|
   | ρ a desfase 2 con \|cp\| < 300 | 0,885 | **0,910** |
   | longitud de decorrelación | 11,4 plies | **14,8 plies** |
   | muestras efectivas | 1,90 M (9,40 por parámetro) | **1,48 M (7,31)** |
   | K | 160,7 | 158,6 |

   **Las etiquetas más profundas son más suaves**, así que se parecen más entre plies
   vecinos y el mismo número de posiciones vale menos. Por eso «v1+v2» dejó de ser una
   curiosidad y pasó a ser la candidata seria.

   Cuatro redes, todas **entrenadas** a 60 épocas y `lr` 2e-3, y cada una con la K de su
   corpus:

   | red | corpus | validación | época guardada |
   |---|---|---|---:|
   | control | 6 M, red a 25.000 nodos | 0,009870 | 60 |
   | prefijo | 6 M, red a 50.000 nodos (mismas aperturas) | 0,010031 | 56 |
   | v2 | 36 M a 50.000 | 0,008487 | **30** |
   | **v1+v2** | **72 M** | **0,008223** | 57 |

   **La última columna se añadió en 0.32 y es una corrección, no un detalle.** Aquí ponía
   «todas a 60 épocas», y entrenadas sí, pero guardadas no: el `train.py` de entonces
   escribía el checkpoint de mejor validación, y cada entrenamiento paró donde le tocó
   (§6.2). Dos comparaciones de arriba quedan tocadas:

   - **`031-v2-contra-v1v2` no aísla el corpus.** Enfrentó la red de v1+v2, de 57 épocas,
     contra la de solo v2, de **30**: media red. Los **+8,7 Elo** [−3,6, +21,0] miden el
     corpus *y* el doble de entrenamiento juntos, así que no sostienen «mezclar v1 con v2
     vale +8,7». Lo que sí queda en pie es la decisión, que era cuál de las dos publicar.
   - **`031-profundidad-de-etiqueta` compara 56 épocas contra 60**, y las cuatro de menos
     le tocan al candidato de etiquetas profundas. La diferencia es mucho menor que la
     anterior —es la cola, con `lr` por debajo de 1,2e-5— pero nadie la ha acotado, así que
     «con todo lo demás igual» es más de lo que se midió. La conclusión de que la
     profundidad no compra fuerza se apoya además en el tamaño del efecto y en su
     intervalo, no solo en ese contraste.

   Ninguna de las dos cambia lo que se publicó, porque las releases se deciden contra la
   release anterior con los dos binarios congelados. Cambian a qué se atribuye lo medido,
   que es justo lo que este documento existe para no confundir.

   Las dos grandes pasan las comprobaciones: idénticas entero a entero entre el motor y
   Python en 2.000 posiciones, y escala 1,077 y 1,066 frente a la HCE. Sobre 20.000
   posiciones de partidas apartadas de v2, la red de 0.30 recorta el 39,2 % de la pérdida
   de la HCE, la de v2 el 43,9 % y la de v1+v2 el 46,1 %.

   Y en el banco, que es quien decide:

   | tanda | qué compara | resultado |
   |---|---|---|
   | `031-profundidad-de-etiqueta` | etiquetas de 50.000 nodos contra 25.000, 6 M cada una | **+3,8 Elo** [−8,7, +16,4] en 1.000 parejas |
   | `031-v2-contra-v1v2` | v1+v2 contra solo v2 | +8,7 Elo [−3,6, +21,0], LOS 92 % |
   | `031-atalaya-v2-A` | v1+v2 contra 0.30 | **`acepta_h1`** en 377 parejas, +73,9 (sesgado al alza) |
   | `031-atalaya-v2-A-estimacion` | lo mismo, tope fijo | **+64,9 Elo** [+57,1, +72,7] en 2.500 parejas |

   **El control desmiente la tesis de este punto del plan.** Doblar el presupuesto de la
   etiqueta sube la profundidad de 7,78 a 8,72 plies y compra, a este tamaño de corpus,
   +3,8 Elo con el intervalo cruzando el cero. Lo que compró los +64,9 fue **más corpus y
   mejor etiquetado**: el doble de posiciones que 0.30 y etiquetas de un evaluador que ya
   era bueno. Consecuencias para lo que viene:

   - **El escalón de 100.000 nodos queda desaconsejado**: costaría el doble por corpus para
     ganar otro ply de etiqueta, y el ply que se acaba de comprar no se ha notado.
   - Lo que sí se ha notado es el volumen. La vía barata es **más posiciones a 25.000 o
     50.000 nodos**, que además salen al doble de velocidad.
   - Y queda una pregunta abierta que este experimento no separa: cuánto de los +64,9 es
     el evaluador de las hojas y cuánto son las partidas que juega una red mejor.
2. **λ = 0,75**, con el WDL del corpus sin adjudicación, que ahora sí es información
   independiente.

   **Resultado (0.32): +21,0 Elo [+13,3, +28,7], y es lo más barato que ha comprado este
   proyecto.** No hizo falta un solo corpus nuevo: el resultado de la partida lleva en el
   registro de 32 bytes desde la fase 2 (byte 30, desde el punto de vista de quien mueve) y
   las partidas del generador se juegan hasta el final, sin adjudicación. Lo único que
   cambia es una línea del objetivo de entrenamiento:

   ```
   objetivo = λ·σ(cp/K) + (1−λ)·resultado
   ```

   Dos redes entrenadas con todo lo demás idéntico a la de 0.31 —corpus v1+v2, K = 159,7,
   `lr` 2e-3, 60 épocas, misma semilla— y cuantizadas igual (error medio 0,84 y 0,90 cp,
   idénticas entero a entero entre el motor y Python en 2.000 posiciones, escala 1,057 y
   1,073 frente a la HCE).

   | tanda | qué compara | resultado |
   |---|---|---|
   | `032-lambda-075` | λ = 0,75 contra 0.31, cribado | +11,1 Elo [−1,0, +23,3] en 1.000 parejas |
   | `032-lambda-A` | lo mismo, decisión | **`acepta_h1`** en 1.049 parejas, +23,6 (sesgado al alza) |
   | `032-lambda-A-estimacion` | lo mismo, tope fijo | **+21,0 Elo** [+13,3, +28,7] en 2.500 parejas |
   | `032-epocas-de-cola` | el control: λ = 1 en la época 60 contra la 57 de 0.31 | +3,3 Elo [−8,6, +15,2] |
   | `032-lambda-050` | λ = 0,50 contra 0.31, cribado | +3,1 Elo [−9,1, +15,4] en 1.000 parejas |

   **El óptimo no está más abajo.** λ = 0,50 se entrenó a la vez que λ = 0,75, con todo lo
   demás igual, y se cribó después precisamente porque la pregunta se volvió obligatoria al
   ver los +21: si quitarle un cuarto del peso a la puntuación vale tanto, ¿por qué no la
   mitad? Porque se pierde casi toda la ganancia. Con tres puntos —λ = 1 de base, 0,75 con
   +21,0 y 0,50 con +3,1— la curva tiene el máximo entre 1 y 0,50, y 0,75 no está lejos de
   él. Lo que queda por probar es el tramo **0,8–0,9**, que es donde suelen aterrizar otros
   motores, y cuesta 2 h de GPU y 40 min de banco por punto.

   El criterio para elegir candidato se declaró **antes** de ver el cribado de 0,50: solo
   se cambiaba el candidato de 0.32 si ganaba el enfrentamiento directo con el intervalo
   entero por encima de cero (`032-lambda-050-contra-075.json`, que queda escrito y sin
   ejecutar). No hizo falta.

   **Qué redes quedan en `nets/`.** La que juega (`atalaya-256-ef81d9ad`) y la del control
   (`atalaya-256-1dbd0d27`, λ = 1 en la época 60). La segunda se queda porque no pertenece a
   ninguna etiqueta de versión y sin ella el control de las épocas no se puede repetir; las
   de releases anteriores sí se recuperan de su etiqueta (`git show 0.31:nets/...`), y la de
   λ = 0,50 (`atalaya-256-360bf4fc`), del commit que la introdujo. Y el binario que se
   publica es **el mismo que midió el banco**: `Vigia 0.32.exe` y el `Vigia 0.32-lam075.exe`
   de las tandas visitan nodos idénticos en las 12 posiciones del banco de velocidad, y solo
   se diferencian en la cadena de versión.

   **El control hacía falta** y lo pidió la revisión adversarial: la red de 0.31 es el
   checkpoint de la época 57 y la candidata el de la 60, porque el `train.py` viejo guardaba
   por mejor validación (§6.2). Entrenada una red λ = 1 hasta la época 60 con todo lo demás
   igual —y reproduciendo el entrenamiento de 0.31 en la sexta cifra, época a época—, esas
   tres épocas de cola no compran nada medible. Los +21,0 son de λ.

   Dos cosas que **no** explican la ganancia, medidas sobre las 5.000 partidas de la tanda
   de la cifra:

   - **No busca más hondo**: 11,934 plies de media contra 11,917. Acierta más a la misma
     profundidad, como la sonda de TT en quiescencia de 0.27.
   - **No juega más decidida**, que habría sido la explicación bonita para un objetivo que
     lleva dentro el resultado: el reparto de finales es el mismo que en 0.31 (66,2 % de
     abandonos adjudicados contra 65,5 %, 18,2 % de repetición triple contra 18,5 %).

   Y la métrica de validación va en contra, que es exactamente lo que se esperaba: contra
   etiquetas de puntuación, la red de λ = 0,75 recorta el 29,5 % de la pérdida de la HCE y
   la de 0.31 el 34,9 %. **Ajustar peor las etiquetas y jugar mejor no es una paradoja**: es
   la señal de que las etiquetas de una búsqueda de 8,7 plies no son la verdad, y de que el
   resultado de la partida aporta algo que ellas no tienen.

   **Lo siguiente no es afinar λ.** Al cerrar 0.32 se propuso probar el tramo 0,8–0,9
   «porque es donde suelen caer otros motores», y era folclore: una parábola por los tres
   puntos medidos (λ = 1 → 0, 0,75 → +21,0, 0,50 → +3,1) pone el máximo en **λ ≈ 0,74**, y
   con ella λ = 0,9 saldría unos 8 Elo por debajo de 0.32. Tres puntos con ±12 de ruido son
   poco para fiarse de la forma, pero lo que sí dicen es que lo que queda por ganar
   afinando λ es un puñado de Elo, y distinguir diferencias así cuesta miles de parejas.

   **El paso siguiente: el corpus v3.** Aquí se escribió que «la palanca con más evidencia
   detrás es el volumen, que en 0.31 sacó +64,9 Elo pasando de 36 a 72 M», y es justo lo
   que este mismo documento desmiente cuarenta líneas más arriba: esos +64,9 cambian tres
   cosas a la vez —volumen, evaluador de las hojas y nodos de etiqueta—. **La única medida
   directa del volumen es `031-v2-contra-v1v2`: +8,7 Elo [−3,6, +21,0]**, sin decisión y
   encima con las épocas desparejadas (57 contra 30).

   O sea que el volumen no está demostrado, está *sugerido*, y v3 existe para medirlo
   limpio: la comparación del paso 2, **v1+v2+v3 (108 M) contra v2+v3 (72 M)**, cambia el
   volumen y nada más, con las dos redes guardadas en la misma época. Con eso basta para
   justificar la ventana, sin apoyarse en una cifra que mide otra cosa. Y λ añade un motivo
   propio: ahora el resultado de la partida entra en el objetivo, así que partidas jugadas
   por un motor más fuerte dan mejor señal que las de la HCE (v1) o las de 0.30 (v2). A
   **25.000 nodos**, porque el control de 0.31 no vio nada en doblarlos (con su salvedad de
   épocas) y así sale al doble de ritmo.

   | paso | qué | tiempo |
   |---|---|---|
   | 1 | v3: 36 M con 0.32 (red `ef81d9ad`) a 25.000 nodos, semillas 21–23, 8 CPUs | **~13,7 h** |
   | 2 | K de v1+v2+v3 y dos entrenamientos a 60 épocas con λ = 0,75: v1+v2+v3 (108 M) y v2+v3 (72 M) | ~4,5 h de GPU |
   | 3 | banco: las dos mezclas entre sí; la mejor contra 0.32, decisión y cifra | ~4 h de 8 CPUs |

   **El ritmo con 8 hilos no es el de la calibración con 2, y a 25.000 nodos sí se nota**:
   348.777 registros por hora y por hilo calibrados con 2 hilos, **328.900 reales con 8**,
   un −5,7 %. En v2 la extrapolación se cumplió de sobra (162.600 calibrados, 166.070
   reales), pero v2 iba a 50.000 nodos: al partir el presupuesto por la mitad se doblan las
   posiciones por segundo y con ellas la presión de memoria que los ocho hilos se disputan.
   De ahí que la ventana sean 13,7 h y no las 13 que decía la primera cuenta.

   Las dos mezclas del paso 2 no son capricho: contestan de paso la pregunta que dejó
   abierta la corrección del punto 1 de esta misma fase —si v1, con partidas jugadas por la
   HCE, sigue aportando o ya estorba—, y esta vez con las dos redes guardadas en la misma
   época, que es justo lo que le faltó a `031-v2-contra-v1v2`.
3. **N = 384**, con la deuda ya cuantificada y el corpus ya dimensionado.
4. **Los 4 rasgos de enroque**, si se decidió dejarlos fuera de v1 por simplicidad.
5. **Re-sintonizar `CORRECTION_MAX = 300`** (`search.rs:135`), calibrado al ruido de la
   HCE. Y medir si la corrección por peones sigue aportando: si su magnitud media cae al
   ruido con red, quitarla son 16 XOR gratis por nodo.
6. **`-C target-cpu=x86-64-v3`**, que es probablemente la mejora de nps más barata que le
   queda al proyecto (cuadriplica la parte densa). Pero es un **experimento del banco por sí
   mismo**, cambia el binario que se distribuye en `Release/` y lo vuelve inejecutable en
   CPUs sin AVX2, y obligaría a fijar el target en `.cargo/config.toml` para que dos
   compilaciones en máquinas distintas sigan siendo comparables. Medido: `native` **no** es
   la respuesta; `x86-64-v3` sí.

   **Hecho en 0.30, y no con `-C target-cpu`**: los dos núcleos calientes se compilan
   también con AVX2 y con AVX-512 y el motor elige al arrancar. +13,6 % de nodos/segundo
   con AVX-512 y +10,1 % con AVX2, en tres pasadas alternadas con nodos idénticos; el
   binario entero con AVX-512 de verdad solo saca +0,5 % al despacho, y sigue
   arrancando en cualquier x86-64. Una corrección a lo de arriba: `x86-64-v4` no sirve
   para medir AVX-512, porque su ajuste hace que LLVM prefiera vectores de 256 bits. El
   precio son los primeros `unsafe` de `src/`, detrás de la comprobación de la CPU.
7. **HalfKP con cubos de rey y factorización**, cuando el corpus llegue a los 100 M.

---

## 9. Qué puede salir mal

| riesgo | dónde se detecta | qué se hace |
|---|---|---|
| **Caché fría del acumulador.** Los 25,8 ns se midieron con la tabla caliente; en la búsqueda real compite con una TT de 128–256 MB y, en un SPRT, con 15 procesos más | **Fase 1**, criterio 3, y **Fase 4** con `--hilos 16` | Aguanta ×3 antes de perder velocidad y ×8 antes de tocar el techo. Si aun así se sale: CReLU en vez de SCReLU (−8,8 ns/nodo, cambiar una constante) y luego N=128 |
| **Des-vectorización silenciosa** por una subida de rustc, un `if` en el bucle o un bounds check que no se elimine. No falla ningún test funcional | **Fase 1** y en cada compilación posterior, por el guardián de velocidad del banco | Revisar el desensamblado contra la lista de §4.5 R3 y restaurar la forma del bucle |
| **Desacuerdo de índices entre Rust y Python.** La red entrena bien sobre un espacio permutado; el motor juega peor y nada lo avisa | **Fase 1 y Fase 3**, por el vector dorado cruzado (§7.1) | Es *el* fallo que esta sección existe para impedir |
| **Escala de centipeones desalineada**, dejando los ocho márgenes de poda mal sintonizados a la vez | **Fase 3**, histograma contra el libro congelado | Una constante de ganancia global, nunca re-sintonizar ocho márgenes |
| **d ≈ 7,5 insuficiente** para destilar búsqueda útil | **Fase 3** (error estancado sobre el holdout a d=10,66) y **Fase 5** | Subir el presupuesto del generador a `nodes 50000`; es la fase 7.1 adelantada |
| **Cubos de salida con pesos de ruido**, porque autojuego produce muy pocas posiciones de ≤8 piezas | **Fase 2**, histograma de ocupación | La regla del millón de `buckets.py`: se fusionan. El motor no cambia, solo la tabla de 33 bytes |
| **Las frecuencias 0,802 / 0,927 se mueven** al cambiar la evaluación, y el presupuesto de §2 está calculado con las viejas | **Fase 4** | Recalcular; el margen de ×8 absorbe desplazamientos de esta magnitud |
| **Los amortiguadores de final hacen daño** porque están calibrados al ruido de la HCE | **Fase 6**, que existe exactamente para esto | Se quedan desactivados |
| **Desbordamiento del acumulador o de la salida** | **Fase 1 y Fase 3**, T1 y T2 al cargar | Recorte más agresivo en el entrenador |
| **La pila revienta en build de depuración** por construir arrays anidados | Primera compilación | `Vec` → `Box<[i16]>`, §3.6 |
| **El libro se regenera** y rompe la comparabilidad con 0.28 | **Fase 0** | Congelado y con sha anotado |
| **La red simplemente evalúa peor que la HCE** | **Fase 5** | Es el riesgo genuino y no se disfraza. El margen es que tiene que ser **más de 43 Elo peor** para perder. Si pasa, el corpus y el entrenador siguen sirviendo: se sube a N=384 con los mismos datos |

Y dos cosas que este plan **no** sabe y no finge saber: si la relación de **2,26 Elo por 1 %
de nps** se mantiene cuando la evaluación es cualitativamente distinta (se midió con la
HCE, y una red que evalúe mejor puede reordenar mejor y bajar el factor de ramificación, o
al revés), y si **SCReLU vale más que los ~4 Elo** que cuesta frente a CReLU en esta talla
de red. Las dos son extrapolaciones fuera de su rango de medida, y las dos son **por
medir**.

---

## 10. Lo que se descarta, y por qué

### 10.1 Las arquitecturas rivales

**HalfKP / HalfKA con cubos de rey** — *descartada por datos y por verificabilidad, no por
velocidad.*

Conviene decir primero lo que se aprendió de esa propuesta, porque desmiente lo que se
creía: **el refresco por movimiento de rey no es el acantilado que se suponía.** Con cuatro
cubos gruesos y espejo de columnas, solo el 46,3 % de las jugadas de rey cruzan cubo (el
8,99 % de todas), y los refrescos se concentran en el final, donde hay **11,0 piezas de
media**, no 32. El coste sale en 5,03 columnas por jugada frente a las 5,00 de una red
plana. El argumento clásico contra los cubos de rey es, en tiempo, falso.

Se descarta por otras tres razones:

1. **Datos.** 656.129 parámetros contra 2,2 M de muestras efectivas son 3,4 por parámetro,
   y el cubo más pobre (rey en campo contrario, o sea finales con el rey activo) se queda en
   **1,83 muestras efectivas por parámetro**. Eso no se ajusta, se memoriza. Para ponerlo en
   régimen habría que multiplicar el corpus por 6: 150 M brutas, ~60 h de generación.
2. **Verificabilidad.** El índice son tres transformaciones que interactúan —volteo de
   perspectiva, espejo horizontal según la columna del rey propio, y un cubo derivado del
   rey ya espejado— más una tabla de desplazamientos. Es la superficie Rust↔Python más
   grande posible, y su fallo es exactamente el silencioso de §7.
3. **Caché.** La tabla se multiplica por 4 (1,25 MB con 4 cubos, 10,5 MB con 32) y la única
   medida que existe del efecto dice **+15 % de coste del acumulador** solo por pasar de
   384 KB a 1,25 MB.

**Vuelve en la fase 7.7**, con factorización (entrenar con los 768 rasgos planos como
rasgos virtuales encima y plegarlos al final), que es la receta estándar contra exactamente
este problema y que la propuesta original no incluía. Y **los 772 rasgos planos de
Atalaya-256 son literalmente ese factorizador**: si v1 se estanca, sus pesos inicializan el
bloque compartido de la red con cubos, sin tirar corpus ni entrenador.

---

**N = 128 (la red mínima)** — *descartada por desaprovechar un corpus ya pagado.*

Es la propuesta más segura del lote y la que más margen deja (10 % del presupuesto, factor
10 de tolerancia al error). Pero 98.689 parámetros con 2,2 M de muestras efectivas son 22,3
por parámetro: entre la mitad y dos tercios de la capacidad que el corpus sostiene, sin
usar. Y llegar a N=256 desde ahí cuesta **otro SPRT completo** con los mismos datos.

**Se injerta de ella:** la disciplina de margen (elegir la talla por el factor de error que
aguanta, no por el número que sale de la hoja de cálculo), y el orden de fases —pagar el
riesgo de integración antes que el de arquitectura—, que es la mejor decisión de proceso de
todo el lote y es la fase 1 de este plan.

---

**N = 384 con 8 cubos** — *descartada por anchura, no por diseño.* Es la propuesta cuyo
diseño más se ha injertado aquí: el índice de dos operaciones, la invariante de «ningún
rasgo depende de más de una pieza», los cuatro rasgos de enroque, `next_castling_rights`,
`push` antes de `make_move`, QA=127 para que el cuadrado quepa en i16 con signo, la
advertencia de `pmaddwd` contra `pmulld`, `Box<[i16]>`, los cubos de salida con regla de
fusión, y la idea del factorizador. Su decisión de anchura se tomó sobre un ritmo denso
equivocado por un factor de 2,2 —que su propio texto sospechaba y dejó por escrito— y con
7,3 muestras efectivas por parámetro queda más justa que N=256 en el eje que de verdad
manda. **Es la fase 7.3.**

---

**Núcleo HCE híbrido (material congelado + escala + KPK, con la red aprendiendo el
residuo)** — *se injerta la mitad buena y se descarta el núcleo incremental.*

Lo que se toma, entero: el prefacio KPK, el postfiltro de escala, el filtrado de
entrenamiento de las posiciones ya resueltas, **meter la escala de final dentro de la pasada
hacia delante del entrenador** (que es la mejor idea de entrenamiento del lote: dividir la
etiqueta por la escala amplificaría el ruido por 16 en el 5 % de posiciones de escala
4/64), y **partir la validación en SPRT-A y SPRT-B**.

Lo que se descarta es el `Nucleo` de 16 bytes con material y contadores mantenido
incrementalmente, por tres razones:

1. **No puede reproducir `endgame_scale_factor`.** `pawnless_drawish_scale` llama a
   `has_nearly_promoting_pawn` (`eval.rs:709`), que pregunta por el *rango* de los peones
   del bando débil. Eso no está en los contadores. En cualquier K+A contra K+peón en sexta
   —la familia que motivó la regla— el núcleo devolvería 4/64 donde la HCE devuelve 64/64,
   aplastando al 6 % una evaluación que debe quedar intacta. Su propio test de equivalencia
   lo detectaría, pero el diseño no lo pasa, y su argumento de venta era que el núcleo es
   *exacto*.
2. **Duplica el estado incremental.** Dos cosas que mantener bajo make/unmake es el doble de
   superficie donde fallar, y el modo de fallo que la propia propuesta nombra —doble
   contabilidad silenciosa: si la red se suma dos veces o ninguna, todo sigue devolviendo
   números plausibles— es el peor que hay.
3. **Congelar el material no amplía la clase de hipótesis.** Una red de 768 rasgos planos
   representa el material exactamente en su parte lineal; congelarlo es una inicialización.
   Lo que compra es condicionamiento y eficiencia muestral, que en este régimen no es poco
   —el material solo explica el **55,5 % de la varianza** de la etiqueta, medido— pero no
   vale el precio de arriba. Si más adelante se quiere ese prior, la forma barata es
   **inicializar** los pesos del FT con los valores de pieza y dejar que se muevan.

La medición que deja y que sí vale para siempre: los trece términos posicionales de la HCE
—las 2.384 líneas, las dos `AttackInfo`— compran **11,6 puntos porcentuales** de varianza
explicada (del 55,5 % al 67,1 %) por **182 de los 193 ns** que cuesta la HCE. Es la peor
relación del proyecto, y es por eso que lo que se conserva es lo exacto y barato, y se tira
todo lo aproximado.

---

### 10.2 Las ideas de Stockfish 19 que no se toman

| idea | por qué no |
|---|---|
| **`full_threats`** (59.808 rasgos, un rasgo por pieza atacante × casilla × objetivo) | Hasta 80 columnas por jugada: **2.360 ns medidos** a N=1024, y además trabajo nuevo dentro de `do_move` para calcular las amenazas con descubiertas. Entre 2 y 6 veces fuera de presupuesto. Es el rasgo más caro de SF19 y el primero que hay que tirar |
| **`PP_3Wide`** (pares de peones) | Mover un peón toca hasta 15 pares por perspectiva = 30 columnas. A ~25 % de jugadas de peón son +22 % de presupuesto por una sola familia de rasgos |
| **Ancho 1024 y capas ocultas densas** (`1024→32→32→1`) | Una capa 512×32 cuesta **2.485 ns derateados = 3,8 veces el presupuesto entero de una evaluación**. Ni con AVX2 cabe (607 ns, el 92 % ella sola). Vigía puede permitirse *menos* red que Stockfish precisamente porque su nodo es barato: 657 ns es poco, y el 30 % de poco es poco |
| **Pesos i8** | Medido: sin SSSE3 no hay `pmaddubsw` y LLVM ensancha a i16 igualmente (1.688 vs 1.679 ns, empate); con AVX2 el i8 es un 32 % *peor*. La danza u8×i8 es una optimización de un ISA que este binario no tiene |
| **Cubos de rey, tablas Finny y camino híbrido** para el rey | 150 líneas, 32–64 KB por hilo y un diff de tableros cacheados para tapar un problema que la arquitectura elegida **no tiene**. Superficie de bugs pura en una primera red |
| **Compresión LEB128** | 394 KiB no justifican 50 líneas de descompresor. Con 98 MB, a Stockfish sí |
| **Doble activación sqr+clip concatenada, el atajo `fc_0[30]−fc_0[31]`, PSQT paralelo, `permute_weights`, `nnz_helper.h`** | Optimizaciones y afinados de una arquitectura que no es esta. `nnz_helper.h` son 171 líneas de `vpcompress`/`pext` |
| **La mezcla final con optimismo, complejidad y amortiguación por regla de 50** (`evaluate.cpp:50-72`) | Vigía no tiene esos términos, y añadirlos a la vez que la red rompería «una mejora cada vez» |

Lo que **sí** se lleva de SF19, y sale gratis: el índice de rasgos como suma de tablas sin
multiplicaciones, la doble perspectiva con el que mueve primero en la concatenación, el
acumulador `i16` con actualización incremental, el troceado de la salida en cubos por
número de piezas, la disciplina de cuantización con escalas fijas y compensación del
desplazamiento en el entrenador, y la validación de la cabecera contra la arquitectura
compilada.

### 10.3 Otras decisiones de descarte

- **Actualización perezosa del acumulador:** techo de ahorro 13,5 % (medido), a cambio de
  una máquina de estados que puede desincronizarse. Ver §4.6.
- **Un `trait` o un `dyn Fn` para conmutar HCE/red:** a 1,5 M evaluaciones por segundo, un
  salto indirecto por nodo es inaceptable. El conmutador es un booleano en `Context`, que es
  una rama perfectamente predicha.
- **`-C target-cpu`** en el mismo cambio que la red: experimento aparte (fase 7.6). Que
  Atalaya-256 **no lo necesite** —cabe con margen de ×8 en SSE2 puro, que es lo que producía el
  `Release/` hasta 0.29; desde 0.30 usa AVX2 o AVX-512 si la CPU los tiene— es una virtud de la propuesta, no una carencia.
- **Submuestrear un ply de cada dos** para combatir la correlación: tirar la mitad de los
  datos para arreglar un problema que se arregla dimensionando. La correlación se trata
  sabiendo cuántas muestras efectivas hay (§5.5), no descartando registros.
- **Usar el resultado de partida del corpus del banco como objetivo WDL:** el 70 % de esas
  partidas están adjudicadas por `resign_cp = 900`, así que el resultado *es* la evaluación
  que se quiere destilar. Se ve en que el ajuste da K = 140 cp.

---

## 11. Documentación que hay que actualizar al terminar

- **`Documentacion_tecnica.md` §4**: reescrito entero. **Las tres reglas de escritura de
  §4.5 van ahí en letra grande**, porque no son evidentes, valen más que cualquier SIMD y se
  pierden en la primera refactorización sin que ningún test lo note.
- **`BancoPruebas.md`**: la regla de proceso nueva — **`banco velocidad` antes de cada SPRT
  de la red**, no para aprobar (los nodos cambian) sino como guardián contra la
  des-vectorización silenciosa; y los sha de los cuatro libros congelados.
- **`MejorasPendientes.md`**: la sección NNUE, con el `id` de cada experimento y su
  decisión, y la escalera de la fase 7.
- **`Descartados.md`**: §10 entera, para que nadie vuelva a proponer `full_threats`, pesos
  i8 ni capas ocultas densas sin leer primero por qué no caben.
