# Vigía — Documentación técnica

**Versión:** 0.25.0 (`Cargo.toml`).
**Lenguaje:** Rust, edición 2021, sin dependencias externas
(`[dependencies]` vacío en `Cargo.toml`).
**Protocolo:** UCI.
**Autoría:** motor escrito íntegramente por Claude (Sonnet, "hermano
Sonnet") en Claude Code; el usuario asesora pero todas las decisiones
técnicas las toma el modelo. Fable actúa como revisor senior, entregando
hallazgos como documentos markdown (`Revisión_*.md`, `Correcciones_*.md`)
que Sonnet implementa. Desde 0.25.0 se suman revisiones externas de otros
modelos, entregadas como `docs/Rev_*.md`. Ver `src/ReadMe.md` para el
prompt original y `docs/Info_humano.md` para recursos externos que el
usuario ha puesto a disposición (motor propio antiguo *Gilipol*,
comparativa de motores, evals HCE de referencia, Alexander 8.3) por si en
algún momento hace falta inspiración adicional.

Este documento unifica el estado *actual* del motor a partir de la
lectura del código fuente y del histórico de `Autorrevisión.md` /
`Plan_de_desarrollo.md` / mensajes de commit. Esos ficheros siguen en la
raíz del proyecto como diario de desarrollo con más detalle narrativo y
las razones de cada decisión; este documento es el resumen técnico de
referencia, no un sustituto.

---

## 1. Estructura del proyecto

```
Cargo.toml          # crate "vigia", lib + 2 binarios, sin dependencias
src/
  lib.rs             # re-exporta todos los módulos como pub mod
  main.rs            # fn main() { vigia::uci::run(); }  — el motor real
  types.rs           # Color, PieceType, Square, CastlingRights, Move, MoveFlag
  bitboard.rs         # Bitboard(u64) + operaciones e iterador
  board.rs           # Board, FEN, make/unmake, hash Zobrist incremental
  zobrist.rs          # claves Zobrist generadas en tiempo de compilación
  movegen.rs           # generación de jugadas, gives_check, SEE, perft
  eval.rs              # evaluación (HCE, tapered), 2384 líneas
  kpk.rs               # oráculo exacto Rey+Peón vs Rey
  search.rs             # búsqueda: PVS/negamax, TT, poda, Lazy SMP
  uci.rs                # protocolo UCI + comando extra "eval"
  bin/selfplay.rs        # harness de autojuego para medir fuerza
tools/calibration/        # pipeline Python de calibración del eval (ver §9)
docs/                       # esta documentación + revisiones externas Rev_*.md
Release/                      # binarios .exe congelados por versión (referencia)
book/komodo.bin                 # libro polyglot para GUIs externos (sin uso en código)
```

`lib.rs` existe para que tanto `main.rs` (el binario UCI) como
`bin/selfplay.rs` (el arbitro de autojuego) reutilicen exactamente las
mismas reglas de tablero/generación/búsqueda/evaluación sin duplicar
código.

**Versionado:** no se usan tags de git; el histórico real de versiones
se lleva con ejecutables numerados en `Release/`. Cada versión publicada
corresponde a un commit con mensaje `X.Y.Z: descripción`.

---

## 2. Representación del tablero (`board.rs`, `bitboard.rs`, `types.rs`, `zobrist.rs`)

- **Híbrida bitboard + mailbox.** `Board` guarda `pieces: [[Bitboard; 6]; 2]`
  (un bitboard por color y tipo de pieza) y además `mailbox: [Option<Piece>; 64]`
  para consultar `piece_at` en O(1) sin recorrer bitboards.
- `Bitboard(pub u64)` implementa las operaciones habituales (set/clear/count/
  lsb/msb/pop_lsb) y el trait `Iterator` (recorre las casillas activas de
  menor a mayor índice).
- `types.rs` define `Color`, `PieceType` (con `PieceType::ALL`), `Square(u8)`
  (a1=0 … h8=63, little-endian rank-file), `CastlingRights(u8)` (4 bits, con
  parseo FEN estricto que rechaza tokens duplicados o malformados),
  `MoveFlag` (14 variantes: normal, doble avance, enroque corto/largo,
  captura, *en passant*, 4 promociones, 4 promociones con captura) y
  `Move { from, to, flag }`.
- Campos de `Board`: `side_to_move`, `castling`, `en_passant: Option<Square>`,
  `halfmove_clock`, `fullmove_number`, `hash: u64` (Zobrist mantenido
  incrementalmente en cada `make_move`/`unmake_move`).

### 2.1 Validación y saneamiento de FEN

`Board::from_fen` es la única frontera de confianza entre una entrada
arbitraria y un motor cuyo camino caliente asume invariantes en lugar de
comprobarlas. Reforzada en 0.25.0 tras las tres familias de FEN
adversarial documentadas en `docs/Rev_GPTSol.md` §P0-03, que el parser
aceptaba y que terminaban en jugada ilegal o en `panic`.

Se **rechaza** (devuelve `Err`, y UCI ignora el comando sin romperse):

- Filas que no suman 8 columnas, huecos fuera de `1..=8`, dos dígitos
  seguidos (`44`) o un `0`.
- Cualquier número de reyes distinto de uno por bando.
- **Peones en la primera u octava fila.** El generador calcula
  `from.rank() + dir` sin comprobar límites y `kpk.rs` indexa una tabla de
  seis filas de peón: `P3k3/8/8/8/8/8/8/4K3 w - - 0 1` se aceptaba y
  provocaba un `index out of bounds`.
- **Reyes en casillas adyacentes.**
- **El rey del bando que *no* mueve en jaque.** Posición imposible desde
  la que el generador ofrecía capturar al rey rival
  (`4k3/4Q3/8/8/8/8/8/4K3 w`, `Qxe8`), dejando el tablero sin rey y
  matando en silencio el hilo de búsqueda.
- `fullmove_number == 0`.
- Casilla *al paso* en una fila imposible para el color activo.

Se **sanea** (se acepta la posición y se descarta la afirmación
incoherente, igual que ya se hacía con los derechos de enroque):

- Derechos de enroque cuyo rey o torre no están en su casilla de origen
  (`sanitize_castling_rights`).
- Casilla *al paso* en la fila correcta pero sin doble avance detrás: se
  exige que la casilla objetivo esté vacía, que la de origen del doble
  avance esté vacía y que el peón rival esté justo pasado el objetivo.
  Sin esto, `4k3/8/8/4P3/8/8/8/4K3 w - d6 0 1` generaba `exd6` sin peón
  que capturar (un peón cambiando de columna gratis) y
  `4k3/8/3B4/3pP3/8/8/8/4K3 w - d6 0 1` desincronizaba mailbox y
  bitboards hasta hacer `panic` en `make_move`.

`halfmove_clock` y `fullmove_number` se incrementan con `saturating_add`,
para que una FEN con contadores cerca de `u16::MAX` no desborde.

Como red final, `movegen::is_in_check` devuelve `false` si no hay rey en
lugar de hacer `panic`, y `kpk::probe` devuelve `Draw` para coordenadas
fuera de su dominio: un hilo de búsqueda que muere es peor fallo que una
rama extra que en juego real nunca se toma.

### 2.2 Hash Zobrist

- **"En passant capturable"** (desde 0.22.0): el hash de la casilla *al
  paso* solo se incluye si existe de verdad un peón capaz de ejecutar esa
  captura (`is_en_passant_capturable`), a través de un segundo campo
  interno `en_passant_hash_square` distinto del `en_passant` visible en la
  FEN. Esto evita que dos posiciones idénticas a efectos prácticos generen
  hashes distintos, lo que rompería la detección de repetición y la TT.
- `compute_hash_from_scratch` **deriva** el componente *al paso* del estado
  visible (`en_passant` + tablero) en vez de leer el campo cacheado. Leerlo
  hacía tautológico justo el test que más valía la pena: una
  desincronización entre caché y posición real aparecía en el hash
  incremental y en el "recalculado", y se cancelaba. El campo cacheado
  sigue existiendo y sigue siendo lo que togglean make/unmake, por la
  razón documentada en su propio comentario.
- **zobrist.rs**: claves generadas en tiempo de compilación con un PRNG
  `splitmix64` como función `const`, sembrado con una constante fija.
  Tablas: pieza×color×casilla, lado a mover, 4 claves de enroque, 8 claves
  de columna *al paso*.

### 2.3 make/unmake

Reversibles vía un struct `Undo` que guarda todo lo necesario para
deshacer (pieza capturada, enroque/al paso/reloj/hash previos).
`make_null_move` / `unmake_null_move` pasan el turno sin mover pieza,
usados por la poda de *null move*.

---

## 3. Generación de jugadas (`movegen.rs`)

- **Ataques deslizantes por rayos clásicos** (no *magic bitboards*): tablas
  de rayos en las 8 direcciones precalculadas en tiempo de compilación;
  `positive_ray_attacks`/`negative_ray_attacks` usan el truco lsb/msb-XOR
  para truncar un rayo en el primer bloqueador. Caballo/rey/peón usan
  tablas de ataque precomputadas igualmente en tiempo de compilación.
- **Pseudo-legal + filtro de legalidad**, no generación legal-only directa:
  `generate_pseudo_legal_moves` genera todo; `legal_moves_scratch` filtra
  **in place** (`retain`) por make/unmake + detección de jaque propio sobre
  el tablero del llamador, sin clonar y sin reservar un segundo `Vec`;
  `generate_legal_moves` es un envoltorio que clona el tablero una vez,
  para llamadores sin `&mut Board` a mano (UCI, selfplay).
- **`gives_check(board, mv)`** (0.25.0): responde si una jugada da jaque
  *sin jugarla*, con la misma respuesta que `make_move` + `is_in_check` +
  `unmake_move`. Cubre jaque directo desde la casilla de destino (con la
  pieza promocionada, no con el peón), jaque descubierto por deslizantes
  contra la ocupación resultante, y los tres casos que tocan más casillas
  que `from`/`to`: *en passant* (vacía también la del peón capturado),
  enroque (reubica una torre que puede dar el jaque ella misma) y
  promoción. Existe porque las decisiones de poda ocurren *antes* de jugar
  la jugada: exentar los jaques de LMP/futility pagando un make/unmake por
  candidato anularía el ahorro que justifica la poda. Un test recorre
  árboles completos de 5 posiciones de referencia comparando cada
  respuesta contra make/unmake.
- **Jugadas especiales**: enroque con comprobación de casillas libres en la
  ruta de la torre y de que el rey no está en jaque ni pasa por ni acaba en
  casilla atacada; *en passant* comparando contra `board.en_passant`;
  promoción generando las 4 piezas posibles.
- **SEE** (`static_exchange_eval`): algoritmo clásico de intercambio,
  usando `attackers_to` (recalcula atacantes deslizantes sobre una
  ocupación decreciente para revelar ataques de rayos X) y
  `least_valuable_attacker`. Maneja correctamente que el rey no puede
  "recapturar" en una casilla que el rival sigue atacando (0.22.0), el
  valor añadido de una promoción de la jugada inicial (0.18.0) y —desde
  0.25.0— **una promoción que ocurre durante la cadena de recapturas**: un
  peón que recaptura llegando a la última fila corona, lo que el algoritmo
  ignoraba por completo. `1R2k3/P7/8/8/1r6/8/8/4K3 b`, donde `...Rxb8` se
  responde con `axb8=D`, se valoraba como un cambio igualado. La lista de
  ganancias es un array fijo en la pila, no un `Vec`: SEE se llama para
  cada captura en la ordenación *y* otra vez en el filtro de quiescencia,
  así que era una reserva de heap por captura y por nodo.
  **Limitación conocida y aceptada**: SEE no comprueba clavadas en la
  cadena de recapturas — simplificación estándar del algoritmo,
  documentada en el propio código. Ver `docs/MejorasPendientes.md` para
  por qué no se ha levantado.
- **Perft**: verificado contra posición inicial y 5 posiciones de
  referencia adicionales a profundidad 3–4 en la suite normal; variantes
  profundas existen como tests `#[ignore]` (`cargo test --release --
  --ignored`).
- Test de consistencia de hash Zobrist: recorre un árbol de jugadas
  comparando el hash incremental contra el recalculado desde cero.

---

## 4. Evaluación (`eval.rs`)

Evaluación clásica hecha a mano (HCE, no NNUE), con interpolación de fase
(*tapered eval*) para el PST de rey y la seguridad del rey. Punto de
entrada principal: `evaluate(&Board) -> i32` (centipawns en perspectiva de
blancas); `evaluate_relative` da la perspectiva del lado a mover para
negamax.

Existe además `evaluate_breakdown`, que devuelve el desglose término a
término más el factor de escala de final aplicado. Duplica deliberadamente
la lista de términos de `evaluate` en vez de llamarla (`evaluate` corre en
cada nodo de búsqueda y no puede permitirse reservar memoria); un test fija
que ambas sumen exactamente lo mismo. Este desglose alimenta tanto el
comando UCI `eval` como el pipeline de calibración (§9).

### Términos sumados (blancas menos negras)

1. **Material** — valores clásicos `P=100, N=320, B=330, R=500, Q=900, K=0`.
2. **PST** (tablas posición-por-pieza) — estilo Michniewski para
   peón/caballo/alfil/torre/dama (reflejadas para negras vía `sq ^ 56`); el
   rey tiene tablas separadas de medio juego y final, interpoladas
   linealmente por `game_phase` (0–24).
3. **Movilidad** — casillas "seguras" alcanzables por pieza, ponderada por
   tipo. Peones y rey excluidos. Es pseudo-legal, no legal, por velocidad.
4. **Estructura de peones** — doblados (−15), aislados (−15), retrasados
   (−10), bonus por peones conectados/en falange, bonus por peón pasado
   según avance, más un término de distancia al rey para pasados, activo
   solo en fases bajas.
5. **Pareja de alfiles** — +30 fijo con 2+ alfiles.
6. **"Bad bishop"** — penaliza peones propios en el color de casillas del
   propio alfil, doblado si además están "trabados" por un peón rival.
7. **Seguridad del rey** — suma de tres señales, todas *tapered* por fase:
   refugio de peones propios, tormenta de peones rivales (por distancia), y
   el término de "peligro" reescrito en 0.25.0 (ver más abajo).
8. **Mop-up** — solo activo con fase baja (≤12) y ventaja material grande
   (≥400 cp): empuja al rey perdedor hacia el borde y premia al rey ganador
   por acercarse.
9. **Torres en columna abierta/semiabierta** — +20 / +10.
10. **Torre en séptima** — +20 por torre en la 7ª fila, solo si hay un
    objetivo real (rey rival en la última fila o peón rival en la 7ª).
11. **Avanzada de caballo (*outpost*)** — +20 por caballo defendido por un
    peón propio en una casilla que ningún peón rival podrá desafiar nunca.
    Desde 0.25.0 "desafiar" exige distancia de columna **exactamente 1**:
    un peón rival en la *misma* columna no puede capturar al caballo, solo
    quedar bloqueado por él, y sin embargo anulaba su propia avanzada
    (`4k3/4p3/8/4N3/3P4/8/8/4K3 w` puntuaba 0).
12. **Final de peones** — activo solo en fase 0 con ≥2 peones en total:
    carrera peón-vs-rey (regla del cuadrado, con tempo real en plies),
    calidad de pasados (protegido/exterior), casillas clave y oposición, y
    mayoría de peones por flanco. El bonus de "pasado exterior" compara
    ahora la columna del pasado contra la media de las demás **en unidades
    de `count`**, sin truncar la división primero: la división entera
    redondea hacia la columna a, de modo que dos posiciones espejo
    horizontal daban resultados distintos (`4k3/8/8/8/PP1P4/8/8/4K3` daba
    +8 donde su propio espejo daba 0).
13. **Tempo** — ±12 fijo para el lado a mover.
14. **Amenazas** — ver más abajo.
15. **Oráculo exacto KPK** — cuando `phase==0` y queda exactamente un peón
    en el tablero, `evaluate` **sustituye** la suma heurística por el
    resultado exacto de `kpk::probe` (§5): 0 si es tablas probadas, o
    `2000` más un pequeño matiz si es victoria probada.

### Peligro del rey (reescrito en 0.25.0)

El término crecía con el cuadrado de un contador ponderado, lo cual es
correcto, pero el contador estaba mal construido: multiplicaba los pesos
por `(enemy_attacks.queens & ring).count()`, es decir, por **cuántas
casillas del anillo cubrían las damas entre todas**, no por cuántas piezas
atacaban. Una sola dama que batiera tres casillas del anillo aportaba 12
unidades, `12²·2 = 288`, recortado al tope de 150 — de modo que la segunda,
tercera y cuarta pieza atacante no cambiaban la evaluación en absoluto.
Medido en `docs/Rev_Opus5.md` §B3: dama sola +92, dama+caballo +113,
dama+caballo+alfil +113, dama+caballo+alfil+torre +113. La seguridad del
rey era en la práctica un interruptor de dos posiciones.

Ahora:

- `attack_info_for` acumula, en la misma pasada en que ya recorría cada
  pieza, **cuántas piezas distintas** baten el anillo del rey rival y su
  peso combinado (menor 1, torre 2, dama 4). No cuesta ataques deslizantes
  adicionales.
- Un solo atacante puntúa 0: una pieza cerca del rey rival es actividad
  normal, no un ataque.
- A las unidades por pieza se les suma un término lineal pequeño por
  casilla del anillo cubierta, que es la señal que la fórmula anterior
  medía por accidente.
- El tope sube de 150 a 500 cp antes del atenuado por fase. Con 150 el
  motor nunca podía justificar un sacrificio por un ataque; los motores
  comparables dejan que este término llegue a 400–600.

### Amenazas (revisado en 0.25.0)

Penaliza piezas atacadas por una pieza estrictamente más barata. La
versión anterior asumía que estar defendido eliminaba la amenaza para las
piezas menores (defendida por un peón ⇒ penalización 0) y, al mismo
tiempo, ignoraba las defensas por completo para torres y damas. Las dos
cosas estaban mal por la misma razón: si un peón captura un caballo
defendido y recapturamos, seguimos habiendo cambiado un caballo por un
peón. Ahora cada caso tiene dos valores, "colgada" y "defendida":

| Víctima | Atacada por | Colgada | Defendida |
|---|---|---|---|
| Menor | peón | −45 | −25 |
| Torre | peón o menor | −35 | −18 |
| Dama | peón, menor o torre | −40 | −20 |
| Peón | cualquiera | −12 | 0 |

"Defendida" se calcula con la unión de todo lo que defiende el color
víctima, rey incluido. Una respuesta completa necesitaría SEE por pieza
amenazada, demasiado caro para un término que corre en cada nodo; queda
documentado como límite conocido (`docs/MejorasPendientes.md`).

### Factor de escala de final (`endgame_scale_factor`)

La suma anterior se multiplica al final por `escala/64`. Casos:

- Material insuficiente (misma función que usa la búsqueda, §6) → `0/64`.
- Final de alfiles de color opuesto puro → `12/64`.
- **Regla de "winnability" sin peones**, `pawnless_drawish_scale`: un bando
  sin peones y con ventaja de a lo sumo una pieza menor no puede forzar la
  victoria (barrer el tablero acaba en material insuficiente). Introducida
  en 0.24.0 tras medir que K+A vs K+P puntuaba +330 donde cuatro motores
  oráculo dan 0 de forma unánime.

  **Corregida en 0.25.0**: elegía el "bando fuerte" mirando solo el
  material bruto y escalaba la evaluación **entera**, que incluye la
  ventaja del *otro* bando. En `8/8/8/8/8/2ppp3/4k3/K1B5 w` —alfil blanco
  contra tres peones ligados negros a un paso de la sexta— el material es
  +30 para blancas, así que la regla concluía "blancas no pueden ganar" y
  aplanaba a −19 una posición que la búsqueda valora en −1107. No es
  cosmético: `static_eval` alimenta razoring, RFP, null-move y futility, de
  modo que el motor podaba agresivamente justo las ramas que le habrían
  enseñado la verdad. Ahora la regla solo se aplica si el bando favorecido
  por la suma cruda *es* el bando que no puede ganar. Además se desactiva
  si el rival tiene un peón a uno o dos pasos de coronar: el argumento
  "siempre puedo entregar la pieza menor por el último peón" deja de ser
  evidente ahí.

---

## 5. Oráculo Rey+Peón vs Rey (`kpk.rs`)

Tablebase exacta para K+P vs K, autocontenida (solo depende de
`types::{Color, Square}`), construida por relajación de punto fijo:

- Canonicalización por simetría: el peón se trata siempre como blanco
  avanzando hacia la fila 8, con columna restringida a a–d → 24 casillas
  de peón × 64×64 casillas de reyes × 2 lados a mover ≈ 200.000 estados.
- Se construye una sola vez de forma perezosa (`OnceLock`), cacheada para
  toda la vida del proceso.
- API pública: `probe(...) -> Outcome`, más `init()` para forzar la
  construcción.

### Corrección crítica de 0.25.0: coronar no es ganar

`strong_transitions` marcaba **toda** coronación como victoria absoluta.
Es falso: si el rey débil está adyacente a la casilla de coronación y el
rey fuerte no la defiende, la dama recién nacida cae de inmediato y queda
rey contra rey. Y como `classify` devuelve `Win` en cuanto encuentra una
transición ganadora, el error se **propagaba hacia atrás** por relajación
a todas las posiciones desde las que se alcanzaba esa coronación.

Un solucionador KPK independiente escrito para la revisión
(`docs/Rev_Opus5.md` §B1) midió el alcance: de 165.676 estados canónicos
válidos, **5.435 (3,3 %) decían "gana" siendo tablas**, en las cuatro
columnas y en todas las filas, incluidas posiciones con el peón todavía en
la segunda. Cero errores en la dirección contraria. Dos ejemplos:

- `8/8/8/1P2k3/8/8/8/K7 w` (regla del cuadrado): 1.b6 Rd6 2.b7 Rc7
  3.b8=D+ Rxb8, tablas de libro. Vigía daba +2044 estático y necesitaba
  16 plies de búsqueda para desmentirse.
- `8/8/8/8/8/8/P5k1/K7 b`: tablas clásicas del peón de torre. Vigía daba
  −2028 incluso a profundidad 16 y —lo grave— la PV mostraba al rey negro
  yendo a d4/c5 en lugar de a b7: como todas las hojas valían −2000, la
  búsqueda no tenía ningún gradiente hacia el plan salvador.

Esto era peor que no tener oráculo, porque `evaluate` **sustituye** toda
la heurística por su veredicto: cambiaba una estimación aproximada por una
certeza falsa de ±2000 cp.

La corrección modela la coronación de verdad:

1. La pieza nueva sobrevive solo si el rey débil no puede alcanzarla
   (`chebyshev(bk, promo) > 1`) o si el rey fuerte la defiende
   (`chebyshev(wk, promo) == 1`) — la misma condición que Stockfish
   codifica en su propio bitbase KPK. Si no sobrevive, la transición es
   `Draw`.
2. Si sobrevive, se comprueba además el ahogado. Como el bando fuerte
   elige la pieza de coronación, la posición solo es tablas si **tanto**
   una dama **como** una torre (que cubre estrictamente menos casillas y
   sigue dando mate por sí sola) dejan ahogado al rey débil. Con eso el
   veredicto es exacto, no una cota optimista.

Tras la corrección, ambos ejemplos puntúan 0 y hay tests de regresión para
los dos, más uno que confirma que una coronación inalcanzable para el
defensor sigue siendo victoria (la corrección no debe pasarse de frenada)
y otro para una casilla de coronación defendida.

### Coste de construcción

`build_table` reutiliza un único buffer de transiciones en vez de reservar
un `Vec` por estado y por barrido. Esas reservas eran la mayor parte de
los ~90–110 ms que costaba la primera evaluación de un final de peones —
un coste que `OnceLock::get_or_init` no puede interrumpir ni con el flag
`stop` ni con el presupuesto de tiempo, de modo que un `go movetime 1` en
frío se pasaba dos órdenes de magnitud (medido en `docs/Rev_GPTSol.md`
§P1-07: 104 ms de motor, 109 ms de pared) y, con Lazy SMP, aparcaba a
todos los hilos ayudantes en la misma inicialización. Además, `isready`
llama ahora a `kpk::init()`: es el handshake que el protocolo ofrece
precisamente para esto y una GUI está obligada a esperar el `readyok`.
Medido tras el cambio: la misma posición bajo `go movetime 1` responde en
0 ms y alcanza profundidad 7.

**Alcance conocido**: cubre únicamente K+P vs K, y es WDL sin DTZ, así que
es exacto en el subjuego *sin* la regla de 50 jugadas. No hay soporte
Syzygy — decisión abierta, no bloqueante (`docs/MejorasPendientes.md`).

---

## 6. Búsqueda (`search.rs`)

**Algoritmo**: negamax con alpha-beta y PVS (*Principal Variation
Search*).

### Estados terminales

Orden de precedencia corregido en 0.25.0: **un mate en el tablero manda
sobre cualquier reclamación de tablas**. El código comprobaba primero
`halfmove_clock >= 100` y devolvía 0, de modo que `7k/5Q2/6K1/8/8/8/8/8 w
- - 99 1` respondía `cp 0` a `Qg7#` — una victoria inmediata tirada a la
basura en una posición legal. `terminal_draw_score` centraliza ahora la
decisión y solo paga el test de mate en el camino raro en que una regla de
tablas ya se ha disparado (el material insuficiente no puede ser mate, y
una repetición implica que la posición tuvo continuaciones antes, así que
únicamente el reloj de 50 puede coincidir con un mate real).

La misma función se usa en `negamax` y en quiescencia.

### Detección de repetición

Repetición **doble** dentro del árbol + historial real de la partida
(deliberadamente no triple; asimetría intencional respecto al árbitro del
harness, que sí exige triple). Desde 0.25.0 el escaneo va hacia atrás **de
dos en dos plies y solo hasta donde alcanza el reloj de 50 jugadas**: una
posición solo puede repetir otra con el mismo lado a mover (cualquier otra
tiene un hash distinto) y nunca a través de la última captura o avance de
peón. Ese acotado es lo que hace la comprobación asequible también en
quiescencia, donde antes no se llevaba el camino en absoluto y un
perpetuo construido con evasiones de jaque quedaba invisible.

`path_start` marca un punto por debajo del cual no se mira: se eleva al
entrar en el subárbol de un *null move* (§ poda).

### Material insuficiente

Una única definición (`eval::is_insufficient_material`) compartida por la
evaluación, la búsqueda y el árbitro del harness, que llevaban tres copias
casi idénticas — que es como llegaron a discrepar sobre K+A vs K+A. Cubre:
sin peones, torres ni damas, y o bien a lo sumo una pieza menor en total,
o bien **un alfil por bando en casillas del mismo color** (ninguno puede
atacar nunca la casilla donde está el otro; es una posición muerta real,
no un juicio). Se queda ahí a propósito: dos caballos, dos alfiles o la
pareja de alfiles pueden forzar mate en algunas líneas. K+N+N vs K es
tablas con juego perfecto pero **no** es posición muerta reglamentaria, así
que tampoco entra: eso es conocimiento de tablebase, no una regla de
material.

### Tabla de transposición

- Array de tamaño potencia de dos, indexado por `hash & mask`, con la clave
  de 64 bits revalidada en cada sondeo. Un *slot* por índice, sin *buckets*:
  el reemplazo es *depth-preferred* solo **dentro de una misma generación y
  para la misma clave**; una colisión de clave distinta, o una entrada de
  una generación anterior, se sobrescriben sin comparar profundidad. Es
  menos selectivo de lo que "depth-preferred" sugiere y está anotado como
  tal (GPT P2-03); pasar a *buckets* de 4 con prioridad combinada va junto
  con quitar el `Mutex` global, ver `docs/MejorasPendientes.md`. Persistente entre llamadas `go` de
  la misma partida (se limpia en `ucinewgame`), protegida por un `Mutex` con
  *locking* por llamada.
- **Sensible al reloj de 50 jugadas** (0.25.0). El hash excluye
  `halfmove_clock` a propósito (dos posiciones que solo difieren en ese
  contador *son* la misma posición para repetición), pero el valor práctico
  de una posición sí cambia cerca del límite. Con una TT persistente,
  llenar la tabla desde `4k3/8/8/8/8/8/8/3QK3 w - - 98 1` y consultarla
  después con reloj 0 devolvía las mismas tablas: una dama de ventaja
  convertida en 0 cp. Cada entrada guarda ahora el contador (saturado a
  100) y su *score* solo se reutiliza como cota si el contador coincide o
  si la regla de 50 queda fuera del alcance del subárbol que la entrada
  resume (`clock + 2·depth + 8 < 100`). La *jugada* de la entrada se sigue
  usando siempre para ordenar.
- **Sin cortes de TT en nodos PV** (0.25.0). El score sería correcto, pero
  la línea terminaría ahí: la PV se propaga durante la búsqueda (ver
  abajo) y un nodo que retorna sin buscar ninguna jugada no tiene línea que
  entregar a su padre. Es además el tratamiento estándar.
- La generación se avanza ahora en el **coordinador**, antes de lanzar
  ningún hilo. Hacerlo desde dentro del hilo principal dejaba una ventana
  en la que un ayudante que arrancara antes escribía entradas con la
  generación anterior, que el avance marcaba acto seguido como rancias: una
  carrera cuyo resultado dependía del planificador del sistema.

### Variación principal

Tabla triangular propagada **durante** la búsqueda (`Context::pv` /
`pv_len`), no reconstruida después caminando la TT. La reconstrucción podía
devolver una línea que contradecía el score con el que se imprimía
(`score mate 2` con una PV de siete jugadas sin mate), y bajo Lazy SMP la
jugada de *ponder* (`pv[1]`) podía pertenecer a una línea distinta de la
elegida. Un test comprueba en varias posiciones que la PV empieza siempre
por `bestmove` y que cada jugada de la línea es legal en su turno.

### Orden de jugadas

Jugada de TT primero; luego capturas **y promociones** por MVV-LVA
ajustado por SEE (las de SEE negativo se empujan por debajo de todas las
tranquilas); luego dos *killer moves* por ply; luego jugadas tranquilas por
historia plana + historia de continuación.

Las promociones entran en la puntuación de "ruidosas" desde 0.25.0. El
generador emite caballo, alfil, torre y dama en ese orden, así que sin ese
término una coronación tranquila a dama puntuaba los mismos 0 que
cualquier tranquila y, con orden estable, quedaba *detrás* de las tres
subpromociones.

La ordenación puntúa cada jugada una sola vez en un buffer reutilizable
del `Context` y ordena ese buffer, en lugar de `sort_by_cached_key`, que
cacheaba bien pero reservaba una tabla temporal en cada nodo del árbol
principal. El índice original forma parte de la clave de orden, de modo que
las jugadas con la misma puntuación conservan el orden de generación
exactamente como haría una ordenación estable.

### Historia

Historia plana (`history[from][to]`) e historia de continuación
(`cont_history[pieza_prev][to_prev][pieza][to]`), ambas con "gravedad"
(`v += bonus − v·|bonus|/16384`).

El *malus* a las tranquilas ya probadas se aplica desde 0.25.0
**independientemente de qué jugada causara el corte**, sea captura o
tranquila: esas tranquilas se ordenaron por delante de la jugada que
funcionó, y ese es exactamente el error de ordenación que las tablas de
historia existen para desaprender. Restringirlo a cortes tranquilos dejaba
que jugadas tranquilas inútiles conservaran puntuaciones infladas
indefinidamente en posiciones tácticas.

El bonus y los killers se condicionan ahora a `is_quiet` (que excluye
promociones) y no a `!is_capture`, unificando el criterio con el de
`tried_quiets`.

### Historia de corrección de peones

Sesgo aprendido del eval estático, indexado por hash Zobrist de estructura
de peones, actualizado por media móvil exponencial (16384 entradas,
saturada a ±300). **Desde 0.25.0 la clave incluye el lado a mover**: la
corrección se aprende y se consume en perspectiva de quien mueve, de modo
que un nodo con blancas a mover y otro con negras y el mismo esqueleto de
peones compartían cubo y escribían errores de signo opuesto, cancelándose
o reforzando el error en vez de corregirlo.

### Poda y reducciones

- *Null-move pruning* con reducción dinámica `R = 2 + prof/3 +
  min((eval−beta)/200, 3)`, condicionado a `eval ≥ beta` y protegido contra
  jaque y posiciones solo-de-peones. **Nunca dos nulos seguidos** y el
  subárbol del nulo abre una frontera de repetición nueva: dos pases
  consecutivos reproducen exactamente el hash de dos plies más arriba (las
  dos claves de turno se cancelan) y, con ambas posiciones apiladas, la
  verificación se puntuaba a sí misma como tablas. El término de tempo
  (±12) rompe la antisimetría que en teoría haría incompatibles las dos
  condiciones de nulo, así que el caso es real.
- *Late Move Reductions* (LMR) con tabla logarítmica
  `0.75 + ln(prof)·ln(índice)/2.5`, a partir de la 4ª jugada y profundidad
  ≥3, sobre tranquilas que no dan ni reciben jaque.
- *Late Move Pruning* (LMP): a profundidad ≤8 se dejan de probar jugadas
  tranquilas más allá de `(3+prof²)`, dividido por 2 cuando la posición
  **no** está mejorando. **El contador es de tranquilas realmente
  buscadas**, no el índice en la lista ordenada completa: con la lista
  encabezada por capturas, "la 3ª jugada" y "la 3ª tranquila" son cosas muy
  distintas, y en una posición táctica con ocho capturas plausibles la
  forma antigua podaba *todas* las tranquilas a profundidad ≤2.
- *Futility pruning* a profundidad ≤2, márgenes `[0,200,300]`.
- **LMP y futility exentan las jugadas que dan jaque**, vía
  `movegen::gives_check`. LMR ya lo hacía; LMP y futility las descartaban
  sin más, lo que puede tirar un mate tranquilo situado más allá del
  umbral.
- *Razoring* a profundidad ≤3, margen `300×profundidad`, confirmado por
  quiescencia.
- *Reverse futility pruning* a profundidad ≤8, margen
  `75×(profundidad − mejorando)`. El signo estaba invertido: el margen
  encogía cuando la posición **no** mejoraba, es decir, la poda se volvía
  más fácil justo cuando la señal era menos fiable (y a profundidad 1 sin
  mejora el margen era cero). Ahora es al revés, como en los motores de
  referencia: una evaluación en ascenso hace más probable que una búsqueda
  real confirmara el corte, así que se exige menos evidencia.
- *Internal Iterative Reduction* (IIR): sin jugada de TT a profundidad ≥4,
  se trata el nodo como una ply menos profundo.
- *Extensiones singulares*: búsqueda de verificación (media profundidad,
  excluyendo la jugada de TT) a profundidad ≥6. Si esa búsqueda no llega a
  probar **ninguna** jugada (la de TT es la excluida y el resto cae en
  LMP/futility) devuelve ahora `alpha` en lugar del centinela `-INF`, que
  hacía `score < singular_beta` trivialmente cierto y regalaba la
  extensión.
- *Extensiones por jaque*: un ply más, **topadas a `ply < 2 ×
  profundidad_de_la_iteración`**. Sin el tope, una cadena larga de jaques
  forzados podía extenderse hasta `MAX_PLY` sin relación alguna con la
  profundidad pedida.
- *Delta pruning* en quiescencia (margen 200 cp sobre el valor de la pieza
  capturada) y filtrado de capturas por SEE≥0. Las **promociones quedan
  exentas del filtro SEE**: un peón a un empujón de coronar es exactamente
  la clase de jugada que no debe quedar sin resolver en el horizonte, y SEE
  valora mal la pieza coronada.

### Quiescencia

- Genera la lista legal completa **primero** y solo después la filtra a
  jugadas ruidosas, para que "no hay capturas que valga la pena leer" y "no
  hay ninguna jugada legal" sigan siendo distinguibles. Confundirlas hacía
  que un **ahogado** se puntuara con la evaluación estática: desde
  `8/8/8/8/8/8/2Q5/k2K4 w - - 0 1` el motor *elegía* `Kd2`, ahogando a las
  negras, y anunciaba más de diez peones de ventaja (medido en
  `docs/Rev_GPTSol.md` §P0-02).
- Corte *stand-pat* cuando no hay jaque; con jaque se buscan todas las
  evasiones legales, sin *stand-pat*.
- **Fail-soft** como `negamax`: devolvía `alpha`, tirando la diferencia
  entre "falló bajo por poco" y "falló bajo por una dama", que es
  información que la TT del padre podía aprovechar.

### Gestión de tiempo y límites

- Presupuesto blando/duro por jugada; el blando decide si arranca una nueva
  iteración, el duro es el corte comprobado cada 2048 nodos — un sondeo
  periódico, no un corte instantáneo: entre dos comprobaciones el motor
  puede pasarse. La única operación que llegaba a bloquearlo de forma
  visible (la construcción de la tabla KPK) está resuelta en §5. Usa
  `movestogo` como divisor cuando se proporciona; si no, divisor fijo de
  *sudden death* (20). `movetime` fija blando=duro. Duro tope en 3× el
  blando.
- **`go nodes` arreglado** (0.25.0). Un `go nodes N` sin más caía en el
  máximo por defecto de 6 plies, así que `go nodes 1000000` terminaba en
  profundidad 6 con unos 4.000 nodos; ahora un presupuesto de nodos cuenta
  como presupuesto igual que un reloj o `infinite`. Y el contador es
  **global a todos los hilos** (un `AtomicU64` compartido) reconciliado cada
  64 nodos, no cada 2048: antes cada hilo recibía el presupuesto entero
  para sí, de modo que `go nodes 2048` con 4 hilos buscaba unos 4×2048.
  Medido tras el cambio: `go nodes 1` consume 24 nodos (antes ~2048) y
  `go nodes 200000` con 4 hilos se queda en el entorno de 200.000 en total.
  Sigue siendo un techo aproximado, no un corte exacto.

### Lazy SMP

Un hilo principal (único que emite líneas `info`) más hilos ayudantes con
profundidad inicial escalonada, cada uno con sus propias tablas de
killers/history/continuation history; solo se comparten la TT y el
contador de nodos.

`SearchResult` lleva ahora un flag `complete`. La primera iteración de un
hilo se publica aunque se aborte —hace falta *alguna* jugada legal que
jugar—, pero describe una búsqueda sin terminar: sin el flag, un ayudante
que arrancaba en profundidad 2 y no la completaba podía ganarle al hilo
principal la elección de `bestmove` por su campo `depth`.

Los ayudantes vigilan **su propio** flag de aborto, que el coordinador
levanta en cuanto el hilo principal termina. Antes se esperaba a que cada
ayudante agotara *su* presupuesto antes de anunciar `bestmove`: tiempo de
reloj de partida gastado en un resultado que se tira. El flag es propio y
no el `stop` compartido porque levantar `stop` también liberaría la espera
de *ponder*, que solo deben liberar `ponderhit` o un `stop` real de la GUI.

### Constantes clave

`MATE_SCORE=30000`, `MATE_THRESHOLD=29000`, `MAX_PLY=128`,
`DEFAULT_TT_SIZE_MB=64`.

---

## 7. Protocolo UCI (`uci.rs`)

**Comandos estándar**: `uci`, `isready`, `debug on|off`, `setoption`,
`ucinewgame`, `position [startpos|fen ...] [moves ...]`, `go`
(subcomandos `depth`, `movetime`, `wtime`/`btime`/`winc`/`binc`,
`movestogo`, `nodes`, `infinite`, `ponder`, `searchmoves`, `mate N`),
`stop`, `ponderhit`, `quit`. Los comandos desconocidos o entradas
malformadas se ignoran sin *crashear* nunca — cada caso tiene su test.

`isready` construye la tabla KPK si aún no lo está (§5).

`searchmoves` que no nombra ninguna jugada legal produce ahora `None` y no
`Some(vec![])`: la restricción se ignora entera de forma explícita en vez
de llegar a la búsqueda y ser descartada en silencio ahí. Se ha preferido
ignorarla a responder `bestmove 0000`, porque un token mal escrito de una
GUI no debe costar una jugada jugable en una posición que tiene muchas.

La búsqueda corre en un hilo aparte para no bloquear la lectura de
`stop`/`isready`. El historial real de partida (`history: Vec<u64>`) es
independiente del árbol de búsqueda.

**Comando extra `eval`**: imprime la evaluación estática sin búsqueda, una
línea por término de `eval::evaluate_breakdown`, más el factor de escala de
final si se aplicó, terminando con `Evaluation: <cp> (white side)` (formato
conservado por compatibilidad con scripts).

**Opciones UCI anunciadas**:

| Opción | Tipo | Rango / default | Efecto |
|---|---|---|---|
| `Hash` | spin | 1–1024 MB, default 64 | Redimensiona la TT de verdad |
| `Clear Hash` | button | — | Vacía la TT |
| `Threads` | spin | 1–16, default 1 | Hilos ayudantes Lazy SMP |
| `Ponder` | check | default false | Habilita `go ponder`/`ponderhit` |
| `Variety` | check | **default false** | Desempate aleatorio en la raíz |

`Variety` es nueva en 0.25.0 y recoge la que era conducta fija: entre las
jugadas de raíz cuyo score exacto está a ≤4 cp de la mejor se elegía una al
azar. Ahora está **apagada por defecto**, por dos razones. La primera es
fuerza: no hay motivo para jugar algo que no es lo mejor que se ha
encontrado. La segunda es medición: con ella encendida, la misma posición a
la misma profundidad no da el mismo recuento de nodos ni el mismo resultado
de match, que es justo lo que un harness de fuerza necesita. Además, la TT
guarda siempre la jugada que *ganó* el score, no la alternativa
cosmética — antes contaminaba la ordenación de la iteración siguiente y la
de todos los hilos hermanos. Un test fija que con la opción apagada la
búsqueda es determinista (misma jugada y mismo recuento de nodos).

`MAX_HASH_MB` está deliberadamente en 1024: se detectó y corrigió un bug
real donde pedir 4096 MB podía casi doblar la memoria reservada por
redondeo hacia arriba del tamaño de tabla. `Threads` está topado en 16
porque la TT usa un único `Mutex`.

---

## 8. Harness de autojuego (`src/bin/selfplay.rs`)

Binario independiente usado como herramienta propia de medición de fuerza.
Uso:

```
selfplay <motor_a> <motor_b> [movetime_ms] [max_plies]
```

- Lanza ambos motores como procesos UCI reales, con hilo lector dedicado.
- Librillo fijo de 8 aperturas cortas, cada una jugada dos veces con
  colores intercambiados.
- El propio harness arbitra con las reglas reales del motor: jaque mate,
  ahogado, regla de 50 jugadas, **repetición triple real** (asimetría
  intencional respecto a la repetición doble heurística del árbol),
  material insuficiente (ahora vía `eval::is_insufficient_material`, la
  misma función que usa el motor), tope de 300 plies y derrota por
  incomparecencia.
- Imprime el marcador y una estimación de diferencia de Elo.

**Limitación importante, ahora explícita**: 8 líneas × 2 colores = 16
partidas tienen un error estándar del orden de ±150–200 Elo. Las cifras
"+394 Elo" de 0.24.0 y "−89 Elo" de 0.23.0 son, estadísticamente, casi el
mismo dato. El harness sirve como *smoke test* ("¿esto ha roto algo
gordo?"), no como medición. Ver `docs/MejorasPendientes.md`: SPRT, paralelismo y un libro más
ancho son el prerrequisito para cualquier afinado serio.

Medición de 0.25.0 contra `Release/Vigia 0.24.exe`, dos controles de
tiempo, reportada tal cual y sin redondear a favor:

| movetime | Resultado (G-P-T) | Puntuación | Elo estimado |
|---|---|---|---|
| 300 ms | 6-5-5 | 53,1 % | +22 |
| 800 ms | 9-3-4 | 68,8 % | +137 |
| **agregado** | **15-8-9** (32 partidas) | **60,9 %** | **+77** |

La lectura honesta: a 300 ms el resultado es indistinguible de la
paridad, y ni siquiera el agregado de 32 partidas es una medición sólida.
Lo que sí puede afirmarse es que **no hay regresión**, lo cual no era
obvio con treinta y tantos cambios simultáneos, y que la tendencia es
mejor cuanto más largo el control — consistente con la mejora medida en
nodos por profundidad (`docs/MejorasPendientes.md`) y con que los arreglos de finales necesitan
profundidad para notarse. Lo que *no* puede afirmarse es una cifra de Elo.

---

## 9. Historia y decisiones relevantes

- **Fase 0** — housekeeping (commits al día, 0 avisos de clippy).
- **Fase 1** — opciones UCI mínimas.
- **Fase 2** — infraestructura de testing de fuerza: librería + binarios,
  harness `selfplay`.
- **Fase 3** — rendimiento en un solo hilo. Encontró el bug de
  sobre-reserva de memoria de `Hash`.
- **Fuera de fase** — ensanchado progresivo de ventanas de aspiración
  (revisión externa de Qwen).
- **Fase 4** — paralelismo Lazy SMP, con el cambio de *locking* de la TT
  como prerrequisito no anticipado.
- **Fuera de fase** — `Ponder`.
- **0.22.0** — gran pasada de eval, hash Zobrist de *en passant*
  capturable, comando `eval`.
- **0.23.0** — primera medición con el harness: resultado negativo,
  pendiente de diagnóstico.
- **0.24.0** — calibración cuantitativa del eval contra motores oráculo
  (Stockfish 18, Obsidian 16.15, Berserk 14, Caissa 1.25) y 5 técnicas de
  búsqueda de consenso extraídas de una comparativa de 23 motores. Pipeline
  conservado en `tools/calibration/` (requiere `python-chess` y rutas
  locales a los motores oráculo; no reproducible sin ese entorno).
- **0.25.0 — pasada de corrección a partir de tres revisiones externas.**
  Ver `docs/Rev_Opus5.md`, `docs/Rev_GPTSol.md` y `docs/Rev_Gemini36F.md`.
  Los hallazgos aceptados están implementados y documentados en las
  secciones anteriores; §10 resume el mapa completo y `docs/MejorasPendientes.md`/`docs/Descartados.md` argumentan lo
  descartado. En una frase: la versión no añade una sola técnica nueva de
  búsqueda o evaluación, y en cambio arregla dos atajos que sustituían una
  estimación por una certeza falsa (el oráculo KPK y el factor de escala de
  final), dos estados terminales mal ordenados (mate vs. regla de 50,
  ahogado en quiescencia), un término de seguridad del rey que era un
  interruptor binario y una frontera de FEN que aceptaba posiciones desde
  las que el motor generaba jugadas ilegales o moría. Medición contra 0.24
  congelado: 32 partidas, 60,9 % (§8) — sin regresión, sin cifra de Elo
  defendible.

---

## 10. Mapa de los hallazgos de las revisiones

Implementados, con la sección donde se explica cada uno:

| Origen | Hallazgo | Dónde |
|---|---|---|
| Opus B1 | Oráculo KPK: coronar ≠ ganar (5.435 estados) | §5 |
| Opus B2 · GPT P1-04 · Gemini 2.2 | Escala de final aplicada al bando equivocado | §4 |
| GPT P0-01 · Opus B6 | Regla de 50 jugadas antes que el mate | §6 |
| GPT P0-02 | Ahogado invisible en quiescencia | §6 |
| GPT P0-03 · Opus B10 | FEN adversarial: peón en fila 1/8, EP fantasma, captura de rey | §2.1 |
| Opus B3 · GPT P2-10 | Peligro del rey contaba casillas, no atacantes | §4 |
| Opus B4 · GPT P2-13 | Amenazas ignoraban (o sobrevaloraban) la defensa | §4 |
| Opus B5 | LMP contaba jugadas totales, no tranquilas | §6 |
| GPT P1-02 | LMP/futility podaban jaques; signo de `improving` en RFP | §6 |
| GPT P1-01 | TT reutilizaba scores con otro reloj de 50 | §6 |
| GPT P1-03 · Gemini 2.4 | SEE ignoraba promociones en la cadena de recapturas | §3 |
| GPT P1-06 | `go nodes` ni respetaba la profundidad ni el total | §6 |
| GPT P1-07 | Construcción perezosa de KPK bloqueaba >100 ms | §5 |
| GPT P1-09 | Corrección de eval mezclaba los dos turnos | §6 |
| GPT P1-10 | PV reconstruida desde TT contradecía el score | §6 |
| Gemini 2.1 · Opus B9 | Ayudantes Lazy SMP no se detenían | §6 |
| Gemini 2.3 | Sin malus de historia si el corte era una captura | §6 |
| Gemini 3.1 · Opus P2 | `vec!` en el heap dentro de SEE | §3 |
| Gemini 3.2 · Opus P3 | Reservas redundantes en `legal_moves_scratch` | §3 |
| Gemini 3.3 · Opus P3 | `sort_by_cached_key` reservaba por nodo | §6 |
| GPT P2-01 · Opus | Aleatorización de raíz desacoplaba score, PV y TT | §7 |
| GPT P2-02 | Carrera de generación de TT en Lazy SMP | §6 |
| GPT P2-04 | Nodo de frontera contado dos veces | §6 |
| GPT P2-05 · Opus B7 | Dos *null moves* seguidos ⇒ repetición falsa | §6 |
| GPT P2-06 | Quiescencia sin detección de repetición | §6 |
| GPT P2-07 | Sin corte beta en la raíz durante fallos de aspiración | §6 |
| GPT P2-08 · Opus | Extensiones de jaque sin tope | §6 |
| GPT P2-09 | Promociones tranquilas mal ordenadas | §6 |
| GPT P2-11 | Outpost anulado por un peón del mismo archivo | §4 |
| GPT P2-12 | Asimetría horizontal del pasado exterior | §4 |
| GPT P2-14 | K+A vs K+A del mismo color no era material insuficiente | §6 |
| GPT P2-16 | `searchmoves` vacío buscaba todas las jugadas | §7 |
| GPT P3-01 | Parseo FEN poco estricto, contadores desbordables | §2.1 |
| GPT P3-02 | `compute_hash_from_scratch` no recalculaba EP | §2.2 |
| Opus B8 | Verificación singular podía devolver `-INF` | §6 |
| Opus (menor) | Quiescencia fail-hard mezclada con negamax fail-soft | §6 |
| Opus (menor) | Killers/historia para promociones tranquilas | §6 |
| GPT P2-04 | Iteración inicial abortada publicada como completa | §6 |

Los tres informes coincidieron además en que la documentación de 0.24.0
describía mal la fórmula de LMP (decía "divisor 2 si mejora"; el código
hacía —y hace— lo contrario). Corregido en §6.

---

## 11. Hallazgos descartados o aplazados

Este documento refleja el estado *actual* del motor. Las propuestas de
revisión que se han aplazado para más adelante están en
`docs/MejorasPendientes.md`; las que se han descartado explícitamente, con
su motivo, están en `docs/Descartados.md`.

---

## 12. Cómo verificar el estado del código

```bash
cargo test --release              # suite completa (210 tests del motor, 8 del harness)
cargo test --release -- --ignored # + perft profundos (lentos a propósito)
cargo clippy --release --all-targets   # debe quedar en 0 avisos
```

Para comparar fuerza contra una versión anterior:

```bash
cargo build --release
./target/release/selfplay "target/release/vigia.exe" "Release/Vigia 0.25.exe" 300
```

Para inspeccionar el eval estático de una posición sin buscar, desde el
propio motor UCI:

```
position fen <FEN>
eval
```
