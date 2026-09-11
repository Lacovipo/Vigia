# Vigía — instrucciones para Claude Code

Motor de ajedrez UCI en Rust (edición 2021), **sin dependencias externas**
(`[dependencies]` vacío, y así debe seguir). Escrito íntegramente por Claude;
el usuario asesora y decide prioridades, las decisiones técnicas las toma el
modelo.

## La regla que manda sobre todas

**Ninguna mejora de fuerza se da por buena sin pasar por el banco de
pruebas.** Existe desde 0.26 y es el binario `banco`. Antes de tocar
búsqueda o evaluación, lee **`docs/BancoPruebas.md`**.

Una mejora cada vez. Nunca dos a la vez: si el resultado sale mal, no se
sabe cuál fue.

## Órdenes de trabajo habituales

```bash
cargo test --release                   # 374 tests (247 motor, 119 banco, 8 harness viejo)
cargo test --release -- --ignored      # + perft profundos, lentos a propósito
cargo clippy --release --all-targets   # tiene que quedar en 0 avisos
cargo build --release
```

Validar un cambio (detalle completo en `docs/BancoPruebas.md`):

```bash
./target/release/banco.exe humo --motor target/release/vigia.exe   # ¿mide bien el banco?
./target/release/banco.exe velocidad --motor <nuevo> --contra <viejo>  # solo velocidad
./target/release/banco.exe sprt --config banco/configs/<experimento>.json  # fuerza
```

## Antes de lanzar una tanda de partidas

**Hay que decirle al usuario cuántas CPUs se van a ocupar y esperar a que
libere sitio.** Es petición explícita suya: la máquina es la de trabajo y
suele tener otras cosas corriendo. `partidas.workers` = partidas
simultáneas = CPUs.

La máquina es un **AMD Ryzen 9 9950X: 16 núcleos físicos, 32 hilos
lógicos** (comprobado, no heredado; aquí ponía 12 y era falso). Para medir
fuerza cuenta el número de físicos: dos partidas compartiendo un núcleo por
SMT van cada una a su ritmo y ensucian el control por tiempo. O sea, tope
sensato 16 workers, y menos si el usuario está usando el equipo.

Una tanda de 128 parejas a `movetime` 100 ms con 4 workers tarda unos 12
minutos. A 25.000 nodos, unos 4.

## Documentación (leerla, no reinventarla)

| fichero | qué contiene |
|---|---|
| `docs/BancoPruebas.md` | **cómo se valida una mejora**. Punto de entrada obligado |
| `docs/Documentacion_tecnica.md` | estado actual del motor, módulo a módulo |
| `docs/MejorasPendientes.md` | qué hacer a continuación, priorizado, con cómo validar cada cosa |
| `docs/Descartados.md` | propuestas rechazadas y por qué (no volver a proponerlas) |
| `docs/PlanNNUE.md` | el plan de la red NNUE, por fases, cada una con su criterio de aceptación |
| `docs/_Info_humano.md` | recursos externos que el usuario pone a disposición. **Solo local**: no se publica porque lleva rutas de su equipo |

Al terminar una mejora hay que **actualizar la documentación**: el estado en
`Documentacion_tecnica.md` y el punto correspondiente de
`MejorasPendientes.md`, con el `id` del experimento y su decisión.

## El número que ordena las prioridades

**2,26 Elo por cada 1 % de nodos/segundo.** 180 Elo por doblar la
velocidad, no los 65 del folclore. Medido en 0.28 con 12.144 partidas
(§8.7 de `docs/Documentacion_tecnica.md`), a `movetime` 100 ms y con un
cambio de **solo** velocidad, que es el único experimento limpio posible.

Consecuencia: la velocidad es la mejor mejora por hora de máquina, y además
se valida con `banco velocidad` en minutos exigiendo nodos idénticos, en
vez de en cinco horas de `sprt`. Un 10 % de nps vale ~+22 Elo, más que
cualquier mejora de evaluación pendiente. Y al revés: una evaluación que
cueste la mitad de los nps parte con 180 Elo en contra.

**Reserva al citarlo**: es a esta fuerza, y medido en dos puntos. A 300 ms
el mismo contraste da 172 Elo por doblar frente a 180 a 100 ms —
indistinguibles—, así que en ese tramo la curva **no** se aplana, contra lo
que se predijo. Sigue siendo extrapolación corta: falta el punto de 800 ms.
El factor de ramificación efectivo del motor es ≈ 1,71, y con él un +32 %
de nodos/segundo compra ~0,52 plies.

## Trampas conocidas

- **Con presupuestos de nodos pequeños ningún Vigía agota `go nodes`**, ni
  siquiera el actual: el límite blando cierra una iteración y no empieza la
  siguiente si no cabe. A 5.000 nodos, 0.25 y 0.26 anuncian jugada al 42 %
  (y en el mismo nodo exacto, 2.123, lo que de paso confirma que los magic
  bitboards no tocan la búsqueda). No es un fallo.

  Aquí ponía que a partir de ~25.000 el efecto desaparece, y que por eso
  `banco humo` usaba 25.000 por defecto. **Las dos cosas eran falsas** y
  dejaban la prueba de humo sin poder ejecutarse: el punto de parada no
  crece con el presupuesto sino a saltos, así que 0.27 se planta en 8.310
  nodos tanto con 20.000 (42 %) como con 25.000 (33 %), por debajo del 50 %
  que exige el guardián del banco. Corregido en 0.28: el defecto es 50.000,
  y a partir de 30.000 ya pasa.
- **Las releases anteriores a 0.25 sí lo incumplen de verdad** (0.24 se planta
  en el 17 % de lo pedido, y no por el límite blando). Comparar por nodos contra ellas da cifras
  absurdas (+422 Elo que no significan nada). El banco lo detecta y aborta;
  contra binarios antiguos hay que usar `movetime_ms`.
- **La cifra de "+77 Elo" de 0.25 sobre 0.24 no está demostrada.** Repetida
  en condiciones serias dio −17,7 Elo con intervalo cruzando el cero, o sea
  *sin decisión*. Ver `docs/Documentacion_tecnica.md` §8.6. No citarla como
  hecho.
- **`acepta_h0` con `elo0=0, elo1=5` no significa "empeora"**, significa "no
  llega a +5 Elo".
- **Una tanda que cruza frontera da una cifra sesgada al alza.** Medido en
  0.28: la misma comparación dio +109,2 Elo parando al cruzar en la pareja
  46 y **+72,4** con tope fijo y 6.072 parejas. Si la cifra es el objetivo,
  hace falta una segunda tanda que no pueda pararse sola; cómo se hace, en
  §7 de `docs/BancoPruebas.md`.
- **Las jugadas de índice par de `parejas.jsonl` NO son de las blancas.** La
  primera la hace quien mueve en la posición del libro, y 10.920 de las
  20.000 de `vigia-20000.epd` tienen negras a mover. Atribuir por paridad
  mezcla los dos motores y da el resultado tranquilizador de "los dos salen
  igual". Ya ha mordido dos veces.
- **La red NNUE va apagada por defecto** (opción UCI `UseNNUE`), y la empotrada
  hoy solo cuenta material: no es para jugar. `banco velocidad` la enciende con
  `--uci UseNNUE=true`. Y el coste de la evaluación **no se saca restando nps
  entre red y HCE**, porque los árboles son distintos (llega a salir negativo);
  se saca duplicando la llamada con nodos idénticos.
- **El ensamblador de la biblioteca sola engaña.** `cargo rustc --release --lib
  -- --emit asm` saca `nnue.rs` completamente escalar, porque con `lto = true`
  la vectorización ocurre al enlazar. Para comprobar las reglas R1–R3 de la red
  hay que mirar el del binario (`--bin vigia`, fichero `deps/vigia.s`).
- El harness antiguo `src/bin/selfplay.rs` sigue compilando pero **no sirve
  para aprobar nada**: 16 partidas, ±150–200 Elo de error.

## Convenciones del repositorio

- Documentación y comentarios de diseño **en español**; nombres de tests e
  identificadores del banco, en el idioma que ya use cada fichero (el motor
  está en inglés, el banco en español).
- Los comentarios explican **por qué**, no qué hace la línea siguiente.
- Todo cambio va con sus tests. `cargo clippy` en cero avisos no es
  negociable.
- Rama única `main`, remoto `origin` en GitHub. El usuario prefiere que
  las operaciones de git las haga yo.
- `banco/resultados/` está fuera del repositorio: pesa y se regenera.
