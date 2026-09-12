# Vigía — Documentación técnica

**Versión:** 0.28.0 (`Cargo.toml`).
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
Cargo.toml          # crate "vigia", lib + 4 binarios, sin dependencias
src/
  lib.rs             # re-exporta todos los módulos como pub mod
  main.rs            # fn main() { vigia::uci::run(); }  — el motor real
  types.rs           # Color, PieceType, Square, CastlingRights, Move, MoveFlag
  bitboard.rs        # Bitboard(u64) + operaciones e iterador
  board.rs           # Board, FEN, make/unmake, hash Zobrist incremental
  zobrist.rs         # claves Zobrist generadas en tiempo de compilación
  movegen.rs         # generación de jugadas, legalidad por clavadas, gives_check, SEE, perft
  magic.rs           # magic bitboards: ataques deslizantes por tabla
  eval.rs            # evaluación clásica (HCE, tapered), 2384 líneas
  nnue.rs            # red NNUE Atalaya-256: cargador, acumulador e inferencia (§4)
  rules.rs           # reglas de fin de partida, compartidas por el árbitro y el generador
  kpk.rs             # oráculo exacto Rey+Peón vs Rey
  search.rs          # búsqueda: PVS/negamax, TT, poda, Lazy SMP
  sha256.rs          # SHA-256 sin dependencias: manifiestos del banco y cabecera de la red
  uci.rs             # protocolo UCI + comando extra "eval"
  bin/selfplay.rs    # harness antiguo de autojuego (superado, ver §8)
  bin/banco/         # banco de pruebas: SPRT, velocidad, EPD (ver §8)
  bin/generador.rs   # datos de la red NNUE: corpus de autojuego, vector dorado y evaluar una red
nets/
  material-256.bin   # la red empotrada: la de la fase 1, solo material, sin entrenar
banco/
  configs/           # configuraciones de experimento (JSON)
  libros/            # libros de aperturas, versionados y congelados
  resultados/        # salidas de las tandas (fuera del repositorio)
tools/calibration/   # pipeline Python de calibración del eval (ver §9)
tools/nnue/          # la red en Python: formato, vector dorado, lector del corpus, entrenador, cuantizador
tools/analiza_tanda.py  # profundidad por motor y finales de una tanda del banco
docs/                # esta documentación y el plan de la red (PlanNNUE.md)
Release/             # ejecutables congelados por versión, ignorados por git
datos/               # corpus de entrenamiento de la red, ignorado por git
```

`lib.rs` existe para que `main.rs` (el binario UCI), `bin/banco/` (el
banco de pruebas), `bin/generador.rs` (los datos de la red) y
`bin/selfplay.rs` (el harness antiguo) reutilicen
exactamente las mismas reglas de tablero/generación/búsqueda/evaluación sin
duplicar código. Que el árbitro del banco use el generador de jugadas del
propio motor tiene una contrapartida asumida —un fallo en `movegen` lo
tendría también el árbitro— y una ventaja demostrada: hubo tres copias
distintas de la regla de material insuficiente y llegaron a discrepar.

**Versionado:** cada versión publicada es un commit `X.Y.Z: descripción`
con su etiqueta de git (de `0.18` a `0.28`; no hay `0.23` porque es anterior
a la mudanza del repositorio), y su ejecutable congelado va a `Release/`,
que git ignora: se reconstruye desde la etiqueta, y cada manifiesto del banco
guarda el sha256 del que se usó.

Esta sección estaba caducada en cinco puntos y se corrigió en 0.28: decía que
no se usaban etiquetas (se dejaron de poner en 0.22 y se completaron en 0.28),
que los ejecutables vivían en `../zzRelease/` (ya no existe), que había un
`book/komodo.bin` (no está ni en disco ni en la historia de git), que `docs/`
contenía revisiones externas `Rev_*.md` (no están; §9 y §10 las siguen
citando), y contaba tres binarios.

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

## 3. Generación de jugadas (`movegen.rs`, `magic.rs`)

- **Ataques deslizantes por *magic bitboards*** (`magic.rs`, 0.26.0): una multiplicación, un desplazamiento y una lectura de tabla, en
  lugar del recorrido de cuatro rayos buscando el primer bloqueador en cada
  uno.
  Para cada casilla solo importan las casillas *intermedias* hasta el borde
  —lo que haya en el propio borde no cambia dónde se para la pieza—, así
  que la ocupación se enmascara a 12 bits como mucho para la torre y 9 para
  el alfil; el *magic* es un multiplicador que lleva cada una de esas
  ocupaciones a una entrada distinta de la tabla de la casilla. Dos
  ocupaciones pueden compartir entrada si su conjunto de ataques es el
  mismo (colisión constructiva); lo que la búsqueda de multiplicadores
  descarta es que dos ocupaciones con ataques *distintos* caigan en la
  misma. Tamaños estándar: 102.400 entradas para torre y 5.248 para alfil,
  861 KB de tablas que **se construyen con `const fn` en tiempo de
  compilación** (no hay inicialización en arranque, ni `OnceLock` ni
  `static mut` en el camino caliente, y el tiempo de compilación no se
  mueve: 17,2 s antes y después). Los 128 multiplicadores están empotrados
  como constantes y se rederivan en un test desde la semilla documentada
  (`0x0007_2C6D_1A3B_5F91`, alfiles a1..h8 y luego torres), de modo que no
  son números tomados de ningún sitio sino un resultado reproducible.
  Caballo/rey/peón siguen con tablas de ataque precomputadas.
- **Cómo se comprueba que la respuesta no ha cambiado**, que es lo único
  que hace legítimo saltarse `sprt`. Dos tests, deliberadamente distintos:
  - `magic.rs` recorre **de forma exhaustiva las 107.648 entradas** —cada
    casilla contra cada subconjunto de su máscara— y exige que la tabla
    contenga exactamente lo que devuelve `walk_attacks`, el recorrido de
    rayos con el que se rellena. Al ser el mismo recorrido, no dice nada
    sobre la lógica de los rayos; lo que demuestra es que el *hash* es
    perfecto, porque una colisión destructiva habría machacado uno de los
    dos conjuntos y la comparación fallaría.
  - `movegen.rs` conserva la implementación clásica anterior como
    `classical_reference`, compilada solo en pruebas: llega al conjunto de
    ataques por otro camino (una tabla de rayo por dirección y el truco
    lsb/msb-XOR para truncarla en el primer bloqueador), así que sí es un
    oráculo independiente. El test las compara casilla por casilla sobre
    ocupaciones de tres densidades distintas.

  Y por debajo de las dos, perft: los mismos recuentos de siempre, que no
  cuadrarían si algún ataque deslizante hubiera cambiado.
- **Medición** (`banco velocidad --profundidad 12`, ver §8): nodos
  **idénticos** en las 12 posiciones —es decir, la búsqueda visita
  exactamente lo mismo y el cambio es de solo velocidad— y entre +12,8 % y
  +18,7 % de nodos/segundo en tres ejecuciones (media ≈ +15 %), sobre un
  ruido de máquina de ±4 %. Medido en la máquina de trabajo del usuario
  (12 CPUs, MSVC), 0.26.0 congelada contra 0.25.0. El binario pasa de
  320 KB a 1,13 MB por las tablas.
- **Pseudo-legal + filtro de legalidad por clavadas** (0.28.0), no
  generación legal-only directa: `generate_pseudo_legal_moves` genera todo
  y `legal_moves_scratch` filtra **in place** (`retain`) sobre el tablero
  del llamador, sin clonar y sin reservar un segundo `Vec`.
  `generate_legal_moves` es el envoltorio que clona una vez, para
  llamadores sin `&mut Board` a mano (UCI, selfplay).

  Hasta 0.27 ese filtro *jugaba y deshacía* cada jugada pseudo-legal solo
  para preguntar si dejaba al rey propio en jaque. Desde 0.28 la respuesta
  se calcula por geometría:

  - `check_info` se computa **una vez por nodo** y trae tres cosas: la
    casilla del rey, las piezas que le dan jaque (`checkers`) y las piezas
    propias absolutamente clavadas (`pinned`). Las clavadas salen de los
    *snipers*, los deslizantes rivales que verían al rey en un tablero
    vacío: si entre uno de ellos y el rey hay exactamente una pieza y es
    nuestra, está clavada.
  - `legal_by_pins` decide cada jugada con cuatro reglas. Una jugada de rey
    es legal si su destino no está atacado **con el propio rey fuera de la
    ocupación** — dejarlo dentro le permite taparse a sí mismo el rayo del
    que huye, y `Ke5-e6` ante una torre en e1 saldría legal. Con jaque
    doble solo mueve el rey. Una pieza clavada solo puede viajar por su
    línea (`LINE`). Con jaque simple hay que capturar al que lo da o
    interponerse (`BETWEEN` más la casilla del que da jaque).
  - El enroque se deja pasar sin volver a filtrarlo: `try_add_castle` ya
    comprobó la casilla de origen y las dos que cruza el rey, y vaciar
    e1/h1/a1 no puede descubrir hacia f1/g1/c1/d1 ningún rayo que no
    pasara ya por e1.
  - **El *en passant* sigue pagando make/unmake**, y es la única excepción.
    Es la jugada que vacía una casilla que no toca, así que puede descubrir
    al rey por la fila a través del peón capturado y del capturador a la
    vez, y eso ninguna máscara tomada antes de jugar lo expresa. Es menos
    del 1 % de las jugadas generadas: midiendo con el filtro duplicado solo
    para rey y *en passant*, los dos juntos son ~2 % del tiempo de nodo.

  Dos tablas nuevas de 32 KB construidas con `const fn`: `BETWEEN[a][b]`
  (casillas estrictamente entre dos que comparten línea) y `LINE[a][b]` (la
  línea entera, extremos incluidos). Son `static` y no `const` a propósito,
  siguiendo lo que ya hace `magic.rs` con sus tablas grandes: un `const` de
  ese tamaño es un valor y no un sitio, y cada indexación podría
  materializar una copia de la tabla entera.

  `is_square_attacked` pasó a ser un envoltorio de una línea sobre
  `attacked_with_occ`, que acepta la ocupación como parámetro. Es lo que
  necesita el caso del rey, y se hizo así —en vez de copiar el detector—
  para que no haya dos rutinas de ataques que puedan divergir con el
  tiempo.

  Un detalle que hace correcta la regla del jaque simple sin ningún caso
  especial: **un caballo nunca está alineado con una casilla que ataca**
  (sus saltos son (1,2) y (2,1), que no caen ni en fila, ni en columna, ni
  en diagonal), así que `BETWEEN` es vacío para un caballo que da jaque y
  la máscara se reduce a "cómetelo", que es exactamente lo que hay que
  hacer. Con un peón que da jaque pasa lo mismo, por adyacencia.
- **Cómo se comprobó que la respuesta no ha cambiado** (0.28.0). Perft es
  un oráculo exhaustivo y lo pasa entero, profundos incluidos, pero solo
  compara totales: dos errores de signo opuesto en el mismo subárbol se
  cancelan y el recuento sigue cuadrando. El test que manda es
  `legality_by_pins_matches_the_make_unmake_filter_it_replaced`, que
  recorre nueve posiciones de referencia a profundidad 3–4 comparando la
  lista nueva contra la vieja **jugada a jugada y en orden**, no como
  conjuntos: el orden es parte de lo que tiene que valer, porque la
  búsqueda ordena esa lista y una lista permutada sería otra búsqueda.
  Más cinco tests dirigidos a los casos que rompen esto en la práctica: el
  rey huyendo por el rayo del que le da jaque, el jaque doble, el *en
  passant* que descubre al rey por la fila, el *en passant* que resuelve el
  jaque capturando al peón que lo da, y la pieza clavada que sí puede
  moverse por su clavada. Y una comprobación de las dos tablas contra la
  definición en prosa, barriendo los 4.096 pares de casillas.

  Verificado además **por mutación**: diez fallos plausibles introducidos a
  propósito en `check_info`/`legal_by_pins`, uno a uno, para ver cuáles
  cazan los tests. Siete caen, la mayoría por varios sitios a la vez. De
  los tres que no caen, dos son **mutantes equivalentes** —no cambian el
  comportamiento— y merece la pena dejar escrito por qué, porque explica
  que esas dos líneas son atajos y no requisitos:

  - *Marcar como clavadas también las piezas rivales* (`pinned | blockers`
    en vez de `& ours`) no se puede observar: `mv.from` es siempre una
    pieza nuestra, así que las casillas rivales añadidas al conjunto nunca
    se consultan. La restricción está por claridad, no por corrección.
  - *Quitar el atajo del enroque* y dejar que pase por la prueba de casilla
    atacada tampoco cambia nada: `try_add_castle` ya comprobó e1 (y c1/g1),
    y si e1 no está atacada no existe rayo por la primera fila que pueda
    alcanzar g1 al retirar el rey de la ocupación. El atajo ahorra trabajo;
    no tapa ningún caso.

  El tercero **sí era un hueco real, y del test, no del motor**: no
  detectar el jaque doble no lo cazaba nadie, porque la posición del test
  dirigido no le dejaba al bando en jaque ninguna pieza aparte del rey y
  "todas las jugadas salen de e8" se cumplía sola. Con una torre negra
  añadida (`4k3/r7/5N2/8/8/8/8/4R1K1 b`), `Ra7-e7` responde a uno de los
  dos jaques y sigue siendo ilegal — que es justo la jugada que dejaría
  pasar un filtro que solo mirase el primer jaque. Cazado ahora por el test
  dirigido y por el de equivalencia.
- **Medición** (`banco velocidad --profundidad 12 --hash 32 --hilos 1`, tres
  pasadas alternando el orden de los binarios para que una carga de fondo
  no caiga siempre sobre el mismo): nodos **idénticos** en las 12
  posiciones y **+29,7 %, +34,1 % y +32,5 %** de nodos/segundo frente a
  0.27 congelada, sobre un ruido de ±4 %, **con la máquina en reposo**.
  Repetidas después con el equipo ocupado dan de +13,6 % a +49,9 % para los
  mismos binarios, porque el comando mide A entero y luego B entero; la
  cifra que vale es la de las tres primeras. Detalle y consecuencia para el
  banco en §4 de `docs/BancoPruebas.md`.

  El techo estaba medido *antes* de escribir el código: duplicando a
  propósito el make/unmake de legalidad, el mismo árbol tardaba un 27,9 %
  más, luego el filtro era el 27,9 % del tiempo de nodo y una legalidad
  gratuita valía como mucho +38,7 %. La predicción declarada de antemano
  fue +25/+33 %, y salió dentro. En perft puro, que es 100 % generación, se
  ve el efecto entero: los dos perft profundos bajan de 0,12–0,14 s a
  0,03–0,06 s.

  **Y después, por primera vez en el proyecto, la cifra de Elo de un cambio
  de solo velocidad.** Con los magic bitboards de 0.26 se decidió no
  declararla, con el argumento de que si los nodos no se mueven no hay
  diferencia de juego que medir. Ese argumento es falso y conviene enterrarlo:
  los nodos no se mueven **a profundidad fija**, que es como mide `banco
  velocidad`, pero una partida se juega a **tiempo** fijo, y ahí el motor
  rápido busca más hondo. Medido en `028-velocidad-en-elo-estimacion`
  (§8.7): **+72,4 Elo, IC 95 % [+67,3, +77,6]** sobre 12.144 partidas, con
  +0,493 plies de profundidad media.
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

### La red NNUE (`nnue.rs`) — entrenada, y todavía apagada por defecto

Desde 0.28 el motor tiene una segunda evaluación, **Atalaya-256**, diseñada y
argumentada en `docs/PlanNNUE.md`. La red empotrada es
**`atalaya-256-6f8033fc`, entrenada con 36 millones de posiciones del propio
Vigía**. Sustituye a la de la fase 1 —construida a mano, solo material—, que era
el andamio para probar el bucle entero antes de gastar horas generando datos y
que sigue en `nets/` porque dos tests la cargan del fichero: es la única red cuya
salida se puede recalcular con lápiz.

Va **apagada por defecto** (`setoption name UseNNUE value true` la enciende)
mientras el SPRT de la fase 5 no la apruebe. El defecto es una decisión de
versión, no de código.

#### De dónde salen los pesos

| | |
|---|---|
| corpus | 36.001.827 posiciones de autojuego a 25.000 nodos, 314.931 partidas, 18,95 h con 8 CPUs |
| aperturas | una distinta por partida, de `apertura.txt`, excluidas las de los libros del banco, más 0–2 plies al azar |
| partidas | hasta el final, **sin adjudicación**, con las reglas de `rules.rs` |
| etiqueta | la puntuación de la búsqueda desde quien mueve; el resultado se guarda pero pesa cero (λ = 1) |
| filtrado | 21.603.026 útiles (60,01 %): fuera capturas, jaques, mates, `\|cp\| ≥ 1500`, KPK y escala cero |
| lo que valen | **1,90 M muestras efectivas** (ρ = 0,885 a desfase 2, decorrelación 11,4 plies): 9,40 por parámetro |
| entrenamiento | PyTorch en la GPU, 60 épocas a `lr` 2e-3, lote 16.384, K = 160,7, ~45 min |
| pérdida | `σ(pred/K)` contra `σ(cp/K)`, con la escala de final dentro de la pasada hacia delante |

**El modelo flotante es el motor sin redondear**: el acumulador vive en unidades
de activación (1,0 son 127 enteros) y la salida es directamente centipeones, así
que cuantizar es multiplicar y redondear, y los recortes de pesos del entrenador
son las cotas T1 y T2 por construcción. Todo el instrumental está en
`tools/nnue/` y el procedimiento, en `docs/PlanNNUE.md` §5–§7.

**Cuatro entrenamientos, no uno**: con el corpus fijo, lo único que quedaba por
elegir era cuánto entrenamiento. 30 épocas a `lr` 1e-3 dan 0,008934 de pérdida de
validación; 60 a 1e-3, 0,008770; 30 a 2e-3, 0,008774; 60 a 2e-3, **0,008750**, que
es la empotrada. Que 30 épocas al doble de tasa igualen a 60 a la mitad dice que
el límite eran los pasos de optimización; que a partir de ahí la mejora sea de
décimas dice que el límite pasó a ser el tamaño del corpus.

**La arquitectura.** `772 → 2×256 → 1`: 768 rasgos planos de pieza y casilla
vistos desde cada bando —propia o ajena, con la casilla espejada para las
negras— más los 4 derechos de enroque; un acumulador `i16` por perspectiva;
SCReLU cuantizada (`clamp(a, 0, 127)² >> 4`); y una capa de salida troceada en
8 cubos por número de piezas, con el mapa de cubos en la cabecera del fichero.
201.992 parámetros y 404.128 bytes, empotrados con `include_bytes!`.

**La invariante que lo sostiene todo:** ningún rasgo depende de más de una
pieza, así que una jugada cambia como mucho un puñado de rasgos y el acumulador
**nunca se reconstruye durante una búsqueda**. Una jugada de rey cuesta lo mismo
que una de peón. El único refresco completo es en la raíz, una vez por hilo.

**Qué se conserva de la HCE**, porque es exacto y barato: el oráculo KPK
*delante* de la red y la regla de material insuficiente *detrás* (escala 0).
Los tres amortiguadores de final (4/64, 12/64 y 16/64) van **desactivados** con
la red (`ENDGAME_DAMPERS`): se calibraron contra los errores de la HCE, y
activarlos es un experimento propio (fase 6 del plan).

**Dónde vive el estado.** En el `Context` de cada hilo, nunca en `Board`, y
como un array de acumuladores indexado por ply, no como una pila. La ranura
`ply+1` se escribe siempre desde la `ply` y la jugada que se va a hacer, así
que no hay nada que deshacer y `unmake_move` no se entera de que existe una
red. `push` va **justo antes de cada `make_move`** (raíz, negamax,
quiescencia) y `copy_parent` antes del movimiento nulo: una vez por jugada,
nunca una vez por llamada recursiva, porque una sola jugada alimenta hasta tres
`negamax` (el scout de LMR y sus dos re-búsquedas).

#### Las tres reglas de escritura

**No son estilo.** Valen entre un 60 % y un 400 % de la velocidad de la
evaluación, están medidas (§4.5 del plan) y **una refactorización que rompa
cualquiera de ellas no hace fallar ni un solo test funcional**:

- **R1.** Los bucles del acumulador se escriben con `zip` de iteradores, nunca
  con índices, y la copia del padre al hijo va fundida con la primera
  actualización.
- **R2.** `HIDDEN` es constante de compilación y las rebanadas son
  `&[i16; HIDDEN]`, no `&[i16]`, para que LLVM conozca la longitud.
- **R3.** La reducción de salida se escribe `sum += (t as i32) * (w as i32)` con
  `t` y `w` en `i16`: ese patrón exacto se convierte en `pmaddwd`. Con los
  operandos ya en `i32` saldría `pmulld`, que es SSE4.1, y este binario no lo
  tiene: LLVM lo emularía con `pmuludq` a tres o cuatro veces el coste.

**Comprobado en el binario de verdad.** `push` usa `paddw` y `psubw` (128 y 448
instrucciones), `refresh` usa `paddw` (320), y la salida de `evaluate_white`
es exactamente `pmaxsw`/`pminsw` (el clamp), `pmullw` (el cuadrado) y `pmaddwd`
(el producto escalar). **Cero `pmuludq`** en todo el binario.

**Y una trampa en cómo comprobarlo.** El ensamblador que emite la *biblioteca*
sola (`cargo rustc --release --lib -- --emit asm`) sale **completamente
escalar**: `push` y `refresh` sin una sola instrucción `xmm`, sumando con
`addw` elemento a elemento. Parece una desvectorización y no lo es: con
`lto = true` la optimización que cuenta ocurre al enlazar, y esa sí vectoriza.
Para comprobar R1–R3 hay que mirar el del **binario**:

```bash
CARGO_TARGET_DIR=<directorio aparte> cargo rustc --release --bin vigia -- --emit asm
# el fichero queda en <directorio>/release/deps/vigia.s, sin sufijo
```

#### Cómo se evita que se rompa en silencio

Todo dentro de `cargo test --release`:

| test | qué atrapa |
|---|---|
| el cargador | cabecera contra la arquitectura compilada, sha256 de los pesos y las cotas de desbordamiento T1 (acumulador) y T2 (salida), comprobadas **al cargar** |
| `the_incremental_accumulator_matches_a_refresh_at_every_node` | cualquier delta o signo erróneo, en cada nodo de árboles completos con enroque, al paso, promociones y torres capturadas en su esquina |
| `the_evaluation_is_antisymmetric_under_a_colour_flip` | orientación de la perspectiva, propia/ajena, enroque y redondeo. Con una red aleatoria, porque se cumple para cualquier peso |
| `the_material_network_scores_material_exactly_as_computed_by_hand` | la red de material —cargada de `nets/`— contra la cuenta a mano, rehecha en Rust. Sigue siendo el único sitio donde la salida de una red se compara con una fórmula cerrada |
| `the_embedded_network_plays_like_something_that_was_trained` | la guardia barata contra empotrar un fichero roto o aleatorio: inicial cerca de cero, dama de más ganando, simetría de color |
| `the_tooling_api_gives_the_numbers_the_search_uses` | que `generador evaluar` —lo que comprueban `check_red.py` y `holdout.py`— use exactamente el camino del acumulador de la búsqueda |
| `golden_vector_matches_the_python_forward_pass` | **un desacuerdo entre Rust y Python**, el fallo que ningún otro test ve: 4.096 posiciones cuyos índices escribió Rust y comprobó Python (sentido A), y cuya pasada hacia delante calculó Python y reproduce el motor entero a entero (sentido B) |
| `a_search_with_the_network_keeps_every_accumulator_in_step_with_the_board` | un `push` que falte en cualquiera de los cuatro enganches: en builds de test, **cada evaluación** de la búsqueda compara el acumulador con un refresco |

#### Medido

- **Con la red apagada, la búsqueda es exactamente la de 0.28**: nodos
  idénticos en las 12 posiciones de `banco velocidad`, y −0,8 % de nps, que es
  ruido.
- **La ruta de evaluación de la red cuesta ≈ 87 ns por nodo** (43,1 de
  `evaluate` y 43,6 de `push`), frente a ~248 de la HCE en la misma máquina.
  Medido duplicando a propósito cada una de las dos llamadas, con nodos
  idénticos en los tres binarios. Es un **suelo**, porque la segunda llamada
  encuentra la caché caliente. El plan presupuestaba 131,9.
- La red de material da **+63 % de nps** sobre la HCE en el mismo binario, pero
  esa cifra **no mide el coste de la evaluación**: a profundidad 12 su árbol
  tiene la mitad de nodos (2,67 M frente a 5,65 M) y otra forma, y restar
  tiempos por nodo entre árboles distintos llega a dar costes negativos. Ver
  §4 de `docs/BancoPruebas.md`.
- **Juega partidas de verdad sin romperse**: 256 parejas de la red de
  material contra 0.28 dieron −236,4 Elo [−271,7, −204,9], que es lo que
  tenía que dar una red que solo cuenta material, y en 512 partidas ni una
  jugada ilegal ni una desconexión (`028-nnue-fase1-material`).

Y con la red entrenada:

- **Predice mejor que la HCE lo que va a decir la búsqueda.** Sobre 20.000
  posiciones de **partidas apartadas** (el mismo reparto de validación que usó el
  entrenador, `tools/nnue/holdout.py`), la red recorta el **22,9 %** de la pérdida
  de la HCE en el espacio de la sigmoide: 0,008612 contra 0,011168, con un error
  medio de 96,4 cp frente a 108,1. La HCE parte con desventaja —las etiquetas son
  búsquedas de 25.000 nodos, no evaluaciones estáticas— y eso es justo lo que se
  quería medir: cuánta de esa distancia recorta la red.
- **Es más rápida que la HCE, no más lenta.** A profundidad 12 contra 0.28:
  **+53 % de nodos/segundo** con la máquina libre y **+36 % con ocho procesos
  simultáneos**, que es la carga del SPRT. La red sufre más que la HCE cuando
  compiten por la caché (404 KB de pesos), y aun así la ventaja aguanta. El banco
  avisa, con razón, de que los nodos difieren en las 12 posiciones y de que por
  tanto esto no aprueba nada por sí solo.
- **Y esa velocidad es profundidad**: +0,60 plies de media en partidas de verdad,
  contra los +0,52 que predice el factor de ramificación 1,71 para ese +36 %.
- **Sonda de fuerza** (`029-atalaya-sonda`, 128 parejas a `movetime` 100 ms):
  **+200 Elo** [+157, +250], 168-53-35, cero partidas anómalas. Es una sonda, no
  un veredicto: la decisión es `029-atalaya-256-A` y la cifra,
  `029-atalaya-256-A-estimacion`.

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

#### Tabla de transposición en quiescencia (0.27.0)

Hasta 0.26 quiescencia ni leía ni escribía la TT, siendo la mayoría de los
nodos de cualquier búsqueda. Ahora hace ambas cosas, con tres decisiones que
conviene no deshacer sin entender por qué están:

- **Se lee después del control de tablas por regla, nunca antes.** Que una
  posición sea tablas depende del camino recorrido para llegar a ella
  —repeticiones, contador de cincuenta jugadas—, y una entrada de la tabla
  no lleva camino. Sondear primero dejaría que una entrada legítima de otra
  transposición convirtiera unas tablas muertas en partida ganada. Lo fija
  el test `a_stored_score_never_overrides_a_draw_by_rule_in_quiescence`.
- **No hace falta comparar profundidades.** Un nodo de quiescencia es
  profundidad 0 por definición y toda entrada guardada se buscó al menos
  tan hondo, así que cualquiera cuyo contador de cincuenta jugadas sea
  compatible sirve.
- **El corte no se restringe a nodos que no son PV**, al revés que en
  `negamax`. Allí esa restricción existe porque devolver pronto deja al
  padre sin línea que enseñar; quiescencia solo limpia la variante y nunca
  escribe en ella, así que no hay línea que truncar.

Se **escribe siempre con profundidad 0**, y eso es lo que impide que un
resultado de quiescencia suplante a una búsqueda real: `negamax` solo se fía
de entradas que alcancen su propia profundidad, y nunca pregunta a
profundidad 0 porque esos nodos se los pasa directamente a quiescencia.

Las puntuaciones de mate se desplazan por `ply` al entrar y al salir, igual
que en `negamax`. Es la parte más fácil de romper, porque un fallo ahí solo
se manifiesta en la *segunda* búsqueda de una posición, nunca en la primera;
lo cubre `a_horizon_mate_still_reads_back_correctly_from_a_warm_table`.

**Medición** (`banco sprt`, experimentos `027-tt-quiescencia` y
`027-tt-quiescencia-largo`): **aceptada**, veredicto `acepta_h1`. Es la
primera mejora del proyecto con decisión formal del banco.

Sobre la magnitud hay que tener cuidado y por eso queda escrita aquí con su
reserva. La tanda larga paró al cruzar la frontera y reporta +20,4 Elo, pero
**esa cifra está sesgada al alza por construcción**: una prueba secuencial
que se detiene en el instante en que cruza se detiene, por definición, en un
momento favorable. La estimación sin ese sesgo es la de la tanda que agotó
su tope de libro sin pararse en ninguna frontera: **+11,4 Elo** sobre 4.000
partidas, IC95 [+2,5, +20,3]. Agregando las dos, 6.626 partidas, sale +15,0
[+8,1, +21,8]. **La magnitud honesta es "entre +11 y +15 Elo"**; lo que el
`acepta_h1` certifica es que supera +5, y eso sí es firme.

Un dato que contradijo la hipótesis de partida: la profundidad media
alcanzada **no sube** (10,751 plies el candidato frente a 10,705 la base,
sobre las 194.886 jugadas de la tanda).

Esas dos cifras están corregidas en 0.28. Aquí ponía 10,763 contra 10,797,
o sea el candidato **por debajo** de la base, y el signo estaba invertido
por un error de atribución al leer `parejas.jsonl`: las jugadas de índice
par no son de las blancas, son **del bando que movía en la posición del
libro**, y 10.920 de las 20.000 posiciones de `vigia-20000.epd` tienen
negras a mover. Atribuir por paridad mezcla los dos motores y empuja las
dos medias hacia el promedio común, que es justo el aspecto que tenían.
La conclusión de 0.27 no cambia —la profundidad no sube de forma
apreciable, y sigue siendo el dato que contradijo la hipótesis de
partida—, pero el número sí. La ganancia no es llegar más hondo sino acertar más a
la misma profundidad, porque un nodo de quiescencia que acierta en la tabla
recupera una puntuación guardada por una búsqueda más profunda en vez de
resolver el intercambio con su propia vista corta.

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

## 8. Banco de pruebas (`src/bin/banco/`)

Cómo se valida una mejora antes de darla por buena. La referencia completa
—metodología, formatos, procedimiento e higiene estadística— está en
**`docs/BancoPruebas.md`**; aquí queda el resumen y el estado.

### 8.1 Por qué se rehízo

El harness de 0.25 (`src/bin/selfplay.rs`) jugaba 8 aperturas × 2 colores =
16 partidas, con un error típico de ±150–200 Elo. Las cifras "+394 Elo" de
0.24.0 y "−89 Elo" de 0.23.0 son, estadísticamente, el mismo dato. Los tres
informes externos de 0.25.0 coincidieron en que el punto más débil del
proyecto no era el motor sino la medición.

`banco` (0.26) sustituye ese harness. Sigue sin dependencias externas y
sigue arbitrando con las reglas del propio motor, pero mide de verdad.

### 8.2 Los cuatro comandos

```bash
banco sprt      --config <fichero.json>   # fuerza de juego. Horas.
banco velocidad --motor <exe> --contra <exe>  # solo velocidad. Segundos.
banco epd       --fichero <suite.epd> --motor <exe>  # táctica. Minutos.
banco humo      --motor <exe>             # verifica el propio banco. Un minuto.
```

Más `banco informe --run <dir>` (rehace resumen y PGN desde los datos
crudos) y `banco libro` (construye libros de aperturas).

### 8.3 Cómo decide `sprt`

- Unidad estadística: la **pareja** (una apertura jugada dos veces con los
  colores cambiados), clasificada en cinco cajones — estadística
  pentanomial. Cancela buena parte del ruido de la apertura y decide con
  bastantes menos partidas que contar victorias sueltas.
- **SPRT** con el LLR pentanomial de Fishtest (máxima verosimilitud
  generalizada, resuelta por bisección sobre la ecuación secular). Fronteras
  `log(β/(1−α))` y `log((1−β)/α)`. Hipótesis habituales: `elo0=0, elo1=5`.
- Paralelismo por workers, con el LLR evaluado **solo sobre el prefijo
  contiguo** de parejas: si no, la decisión dependería de qué worker terminó
  antes. Verificado: 1 worker y 4 workers dan partidas byte a byte
  idénticas.
- Persistencia reanudable (`parejas.jsonl` como única fuente de verdad,
  vaciada a disco tras cada pareja) y firma del experimento que impide
  mezclar tandas de binarios o parámetros distintos.
- Salida PGN en SAN con puntuación y profundidad por jugada, para poder
  clasificar las derrotas en vez de solo contarlas.

### 8.4 La prueba de humo A/A, y por qué es fuerte

`banco humo` enfrenta un binario contra sí mismo. Como la búsqueda es
determinista a nodos fijos con un hilo y `Variety` apagado, y `ucinewgame`
vacía la TT, las dos partidas de cada pareja son la misma partida con los
papeles cambiados. Por tanto **todas** las parejas deben caer en el cajón
central, con 0,00 Elo exacto.

Resultado actual: pentanomial `[0, 0, 8, 0, 0]` sobre 8 parejas, 0 partidas
anómalas. Cualquier desviación significaría que el banco introduce asimetría
entre los dos bandos, y hasta arreglarlo ninguna medición suya sería fiable.

### 8.5 Una trampa encontrada al construirlo

El control por nodos solo es justo si **los dos** binarios respetan
`go nodes`. **La release 0.24 no lo hace**: pedidos 25.000 nodos, anuncia la
jugada tras unos 4.100 (17 %). Enfrentada así a un binario que sí los
respeta, pierde el 92 % de las partidas y el banco reporta **+422 Elo** —
cifra que no mide fuerza, mide que un bando pensó cuatro veces menos.

El banco sondea ahora ambos motores antes de empezar y aborta si alguno se
queda por debajo del 50 % de los nodos pedidos. Contra binarios antiguos hay
que usar `movetime_ms`.

Esto obliga a releer con cuidado cualquier comparación histórica del
proyecto que se hiciera por nodos contra una release anterior a 0.25.

### 8.6 Medición de 0.25 contra 0.24

Dato histórico del harness antiguo, conservado tal cual se publicó (32
partidas, `movetime` 300 y 800 ms): **+77 Elo**, con la advertencia explícita
de que ni siquiera el agregado era una medición sólida.

Primera medición del banco (`id` `025dev-vs-024`, 128 parejas = 256
partidas, `movetime` 100 ms, libro `vigia-256.epd` semilla 20260818,
adjudicación activada, 4 workers):

| | |
|---|---|
| Candidato (0.25-dev) | 92 ganadas, 59 tablas, 105 perdidas |
| Pentanomial | `[20, 23, 49, 22, 14]` |
| Puntuación | 47,46 % |
| **Elo** | **−17,7** (IC 95 %: −53,7 … +18,0) |
| LOS | 16,6 % |
| LLR | −0,3026 sobre fronteras ±2,944 → **`continuar`** |

Lectura honesta, que es distinta de la cómoda:

1. **La prueba no decidió.** El intervalo cruza el cero: con 128 parejas a
   este control no se puede afirmar ni que 0.25 sea mejor ni que sea peor.
2. **La cifra de +77 Elo no se reproduce.** No queda refutada —el intervalo
   la excluye, pero el control de tiempo no es el mismo—, pero desde luego
   no queda respaldada. Aquella medición eran 32 partidas y su propia
   documentación ya avisaba de que no era una medición sólida.
3. **El control importa, y aquí juega en contra.** `banco velocidad` mide
   que 0.25-dev es un ~16 % **más lento** en nodos/segundo que 0.24 (más
   términos de evaluación). A 100 ms por jugada esa lentitud pesa mucho; el
   dato antiguo era a 300 y 800 ms, y ya entonces se observó que la
   diferencia mejoraba cuanto más largo era el control.

Lo pendiente: repetir a 300 ms y a 800 ms, ya con el libro ancho de 2.000
posiciones (`banco/libros/vigia-2000.epd`), que era el otro punto de esta
lista y ya está resuelto. Hasta entonces, **el proyecto no tiene una cifra
de Elo demostrada para 0.25 frente a 0.24**, y eso es una mejora sobre tener
una cifra falsa.

Otras tandas registradas:

| `id` | qué | resultado |
|---|---|---|
| `humo-A-contra-A` | 0.25-dev contra sí mismo, 20.000 nodos | pentanomial `[0,0,8,0,0]`, 0,00 Elo — el banco es simétrico |
| `025dev-vs-024` por nodos | 0.25-dev vs 0.24, 25.000 nodos | **inválido**, ver §8.5 |


### 8.7 Cuánto Elo compra la velocidad

La pregunta llevaba abierta desde 0.26 y `docs/Descartados.md` prohíbe
expresamente contestarla con la regla de folleto de "duplicar velocidad =
+65 Elo". 0.28 permitió medirla, porque un cambio de **solo** velocidad es
el único experimento limpio posible: no toca ninguna decisión de la
búsqueda, así que todo lo que se mida es atribuible al reloj.

Dos tandas, 0.28 contra 0.27 congeladas, `movetime` 100 ms, `Hash` 32, un
hilo, libro de 20.000 y 8 workers:

| tanda | parejas | qué contesta | resultado |
|---|---:|---|---|
| `028-velocidad-en-elo` | 46 | decisión | `acepta_h1` (H1 = +60 frente a H0 = +20), +109,2 Elo |
| `028-velocidad-en-elo-estimacion` | 6.072 | magnitud | **+72,4 Elo, IC 95 % [+67,3, +77,6]** |

**La primera decidió y la segunda midió, y hay 37 Elo de diferencia entre
sus cifras.** Es la demostración empírica del sesgo que §7 de
`docs/BancoPruebas.md` describía en abstracto: la tanda que cruzó paró en
la pareja 46, en el instante favorable en que cruzó. La cifra buena es la
de la segunda, que agotó su tope sin cruzar nada.

El tipo de cambio que sale, **en este punto de operación y no en general**:

| magnitud | valor |
|---|---|
| nodos/segundo | +32,1 % |
| profundidad media | 11,125 contra 10,632 = **+0,493 plies** |
| fuerza | **+72,4 Elo** |
| Elo por doblar la velocidad | **180** |
| Elo por ply | 147 |
| Elo por cada 1 % de nodos/segundo | **2,26** |

Los +0,493 plies superan los `log2(1,321) = 0,40` que predice la cuenta
lisa, y tiene explicación: el presupuesto blando decide si **empieza** una
iteración entera más, así que la ganancia no es continua sino a saltos, y
un motor un tercio más rápido cruza ese umbral en bastantes más posiciones
de las que la proporción sugiere.

#### El segundo punto: a 300 ms el tipo de cambio no se mueve

La reserva más fuerte de todo lo anterior era que estaba medido a 100 ms, y
que la curva de Elo contra tiempo se aplana, así que a control de torneo el
mismo +32 % valdría bastante menos. **Esa predicción es mía y los datos la
contradicen.** Se midió con `028-velocidad-en-elo-300ms`, idéntica a la
anterior salvo el reloj: mismos binarios congelados, mismo libro, misma
adjudicación, mismos 8 workers, un solo grado de libertad.

| | 100 ms | 300 ms |
|---|---:|---:|
| Elo de 0.28 sobre 0.27 | +72,4 [+67,3, +77,6] | +69,1 [+60,6, +77,7] |
| profundidad media, base | 10,632 | 12,649 |
| profundidad media, candidato | 11,125 | 13,202 |
| plies comprados por el +32,1 % | +0,493 | +0,553 |
| **Elo por doblar la velocidad** | **180** | **172** |

Los intervalos se solapan casi por completo. La diferencia es de 3,3 Elo
con un error típico de 5,1: **0,65 sigmas, o sea nada**. Entre 100 y 300 ms
el tipo de cambio es plano.

Y hay una comprobación del modelo que salió de regalo. Triplicar el tiempo
sube la profundidad en 2,02 plies (base) y 2,08 (candidato), lo que fija el
**factor de ramificación efectivo del motor en ≈ 1,71**. Con ese EBF, un
+32,1 % de nodos debería comprar `ln(1,321)/ln(1,71) = 0,519` plies. Lo
medido son 0,493 y 0,553. El modelo predice el dato que no se usó para
construirlo, que es la única clase de acuerdo que significa algo.

Por qué sale plano, en la medida en que se puede descomponer: el Elo por
ply baja con la profundidad (de ~147 a ~125) mientras los plies que compra
el mismo porcentaje se mantienen en ~0,52, y los dos efectos casi se
cancelan. Conviene decir que **el producto está mejor medido que sus dos
factores**: "Elo por ply" se obtiene dividiendo por una profundidad que
tiene su propio ruido, mientras que los Elo por doblar velocidad salen
directamente de la tanda.

**Las reservas que quedan en pie.**

1. **Sigue siendo una extrapolación corta.** De 100 a 300 ms hay 1,6
   duplicaciones de tiempo y la profundidad solo pasa de ~10,6 a ~12,6
   plies. Que no se aplane ahí no demuestra que no se aplane a 60 s por
   jugada, que es control de torneo de verdad; demuestra que se aplana
   **más despacio de lo que yo suponía**. Un tercer punto a 800 ms sigue
   siendo el trabajo pendiente más rentable del banco.
2. **Es a esta fuerza.** Un motor más fuerte saca menos de cada ply nuevo.
3. **El 70 % de las partidas terminan por abandono adjudicado** a 100 ms, y
   el 63,8 % a 300 ms (`resign_cp` 900). La adjudicación recorta finales,
   así que esto describe la fuerza en apertura y medio juego más que en
   final.

**Consecuencia práctica, y no es pequeña**: a 2,26 Elo por punto porcentual
de nodos/segundo, un 10 % de velocidad vale ~+22 Elo — más que cualquiera
de las mejoras de evaluación o de poda que quedan en
`docs/MejorasPendientes.md`, y validable en minutos con `banco velocidad`
en vez de en horas con `sprt`. Y al revés: una evaluación que cueste la
mitad de los nodos/segundo parte con ~175 Elo en contra antes de evaluar
mejor ni una posición.

Eso condiciona el tamaño de una futura red NNUE más que cualquier otra
consideración, y el segundo punto de la curva lo endurece: la deuda no era
un artefacto del control rápido en el que se midió. A 300 ms es la misma.

### 8.8 El harness antiguo

`src/bin/selfplay.rs` sigue compilando, con sus 8 tests, y sirve como *smoke
test* rápido. Está superado por `banco` para cualquier medición y no debe
usarse para aprobar un cambio.

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
  conservado en `tools/calibration/`. La ruta a `vigia.exe` se deduce de la
  ubicación del propio script, así que no depende del equipo; los motores
  oráculo, que sí viven fuera del repositorio, se buscan bajo `C:\AjeEng` y
  la variable de entorno `AJEENG` cambia esa raíz. Requiere `python-chess` y
  tener los cuatro oráculos instalados.
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
- **0.26.0 — magic bitboards, y nada más.** Primera versión validada con el
  banco de pruebas en vez de con una corazonada: *magic bitboards* (§3),
  aprobada por `banco velocidad` con nodos idénticos en las 12 posiciones y
  ~+15 % de nodos/segundo. Que lleve **un solo cambio** es deliberado y es
  la regla de oro del proyecto en acción: si la versión mezclase esto con
  una mejora de evaluación y el resultado saliera raro, no habría forma de
  saber cuál de las dos fue. Al no tocar ninguna decisión de la búsqueda no
  necesitaba `sprt`, y por tanto no consumió ni una partida.
- **0.27.0 — tabla de transposición en quiescencia, y nada más.** La
  **primera mejora de fuerza del proyecto con veredicto formal**:
  `acepta_h1` sobre 2.626 partidas, tras 4.000 previas que no llegaron a
  decidir por agotarse el libro. Magnitud honesta entre +11 y +15 Elo (§6,
  con el detalle de por qué no es el +20,4 que reporta la tanda que paró al
  cruzar). Un solo cambio otra vez, por la misma razón.

  Lo que costó no fue el código —unas cuarenta líneas— sino aprender a
  medirlo: hicieron falta tres tandas, dos libros y dos hallazgos de método
  que están en §7 de `docs/BancoPruebas.md`.
- **0.28.0 — legalidad por clavadas, y nada más.** Sustituye el make/unmake
  por jugada del filtro de legalidad por aritmética de rayos (§3).
  Aprobada por `banco velocidad` con nodos idénticos y ~+32 % de
  nodos/segundo; cero partidas gastadas, como 0.26.

  Medida después como fuerza, ya con la versión cerrada: **+72,4 Elo**, IC
  95 % [+67,3, +77,6] (§8.7). Es la mayor ganancia medida del proyecto, y
  de paso contestó una pregunta que llevaba abierta desde 0.26 — cuánto
  Elo compra la velocidad — con consecuencias para todo lo que viene.

  Se eligió entre diez candidatos, analizando cada uno contra el código en
  vez de contra la literatura, y el criterio que decidió fue **Elo por hora
  de máquina**. Las cuatro mejoras de fuerza que competían (LMR sensible a
  la historia, poda por SEE de tranquilas, historia de capturas, ProbCut)
  comparten un problema que la lista de pendientes no decía: su efecto
  esperado ronda +3/+5 Elo, y un efecto de +4 Elo **no cruza `elo1=5`
  nunca**. Cada una habría costado cinco o seis horas de máquina para
  terminar, con toda probabilidad, en *sin decisión*.

  De paso salieron tres correcciones a datos que la documentación daba por
  buenos: la premisa de Syzygy (§`MejorasPendientes`), la cifra de nps de
  referencia de la máquina, y un defecto real del banco — `banco humo` no
  se podía correr con su presupuesto de nodos por defecto, y tampoco con
  0.27. Está arreglado y contado en §6 de `docs/BancoPruebas.md`.

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
| Opus P8 | Rayos clásicos donde cabían magic bitboards | §3 |

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
cargo test --release              # 254 del motor + 119 del banco + 8 del harness antiguo
cargo test --release -- --ignored # + perft profundos (lentos a propósito)
cargo clippy --release --all-targets   # debe quedar en 0 avisos
```

Para comprobar que el propio banco de pruebas mide bien (ver §8.4):

```bash
cargo build --release
./target/release/banco.exe humo --motor target/release/vigia.exe
```

Para comparar fuerza contra una versión anterior — leer antes
`docs/BancoPruebas.md`, y avisar de cuántas CPUs se van a ocupar:

```bash
./target/release/banco.exe sprt --config banco/configs/mi-experimento.json
```

Para inspeccionar el eval estático de una posición sin buscar, desde el
propio motor UCI:

```
position fen <FEN>
eval
```
