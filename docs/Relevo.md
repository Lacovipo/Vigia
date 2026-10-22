# Relevo: dónde quedó Vigía al pasar a BurroCode

Nota para mí mismo, escrita al cerrar la etapa en Claude Code (octubre de 2026).
Allí no recordaré nada de esto; el usuario sí. Lo que no esté aquí está en
`CLAUDE.md` y en `docs/`, que siguen siendo la fuente de verdad.

## Estado

- **Release vigente: 0.34.0** (etiqueta `0.34`, commit `b72b05e`): red Atalaya-256
  con espejo horizontal, +8,5 Elo [+0,8, +16,2] sobre 0.33 a 100 ms por jugada.
- 406 tests, clippy en cero. Binarios congelados en `Release/` (fuera de git).

## Cómo trabaja el usuario conmigo

- Me delega las decisiones técnicas («Vigía es tu motor»); él fija prioridades.
- Textos cortos. «Liberar» = publicar una versión que merece probarse sola.
- **Antes de lanzar partidas, decirle cuántas CPUs y esperar a que libere sitio.**
  La máquina nunca está parada: la mantiene al 50–60 % a propósito («un juego con
  reglas ruidosas»); 4 workers suele ser lo razonable.
- Fuentes de motores top en `D:\desa\_Top_Ajedrez`: ideas sí, copiar código no.
  `C:\NNUE\PGN`: no tocar, copiar lo que haga falta. `docs/_Info_humano.md` es
  solo local.
- Yo hago commit, etiqueta y push en `main`.

## Lo siguiente, por orden (detalle en `docs/MejorasPendientes.md`, fila 7.5)

1. **Carril de velocidad**: el espejo cuesta de un 4 a un 6 % de nps (9–13 Elo).
   (a) alinear a 64 bytes tabla de rasgos y acumuladores (hipótesis: también
   explicaría que una copia idéntica de un binario mida un 1 % distinto);
   (b) `rebuild_half_after` bajo cada juego de instrucciones; (c) caché de
   acumuladores. Una cosa cada vez, con `banco velocidad --pasadas 12`, nodos
   idénticos, salida guardada y más de una compilación.
2. N = 384 (peaje estimado ~3 %, sin medir), después cubos de rey con factorizador.

## Lecciones que me costaron caras (todas en `CLAUDE.md`)

- Ninguna cifra sin su medida guardada al lado: el «−1,75 %» del espejo era falso.
- El tope de una tanda se calcula con `tools/potencia_sprt.py`, no a ojo.
- La preinscripción, rama «sin decisión» incluida, se commitea antes de lanzar.
- Una banda estrecha no es ausencia de sesgo: calibrar contra algo conocido.
- Revisión adversarial antes de publicar: siempre encontró algo.
