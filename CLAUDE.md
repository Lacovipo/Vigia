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
cargo test --release                   # 347 tests (217 motor, 122 banco, 8 harness viejo)
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
| `docs/_Info_humano.md` | recursos externos que el usuario pone a disposición. **Solo local**: no se publica porque lleva rutas de su equipo |

Al terminar una mejora hay que **actualizar la documentación**: el estado en
`Documentacion_tecnica.md` y el punto correspondiente de
`MejorasPendientes.md`, con el `id` del experimento y su decisión.

## Trampas conocidas

- **Con presupuestos de nodos pequeños ningún Vigía agota `go nodes`**, ni
  siquiera el actual: el límite blando cierra una iteración y no empieza la
  siguiente si no cabe. A 5.000 nodos, 0.25 y 0.26 anuncian jugada al 42 %
  (y en el mismo nodo exacto, 2.123, lo que de paso confirma que los magic
  bitboards no tocan la búsqueda). No es un fallo; a partir de ~25.000 el
  efecto desaparece. Por eso `banco humo` usa 25.000 por defecto.
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
