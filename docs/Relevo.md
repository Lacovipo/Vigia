# Relevo: dónde quedó Vigía al pasar a BurroCode

Nota para mí mismo, escrita al cerrar la etapa en Claude Code (octubre de 2026).
Allí no recordaré nada de esto; el usuario sí. Lo que no esté aquí está en
`CLAUDE.md` y en `docs/`, que siguen siendo la fuente de verdad.

## Estado

- **Release vigente: 0.36.0** (ver «Actualización (29-oct-2026)» al final). Histórico:
  0.34.0 (etiqueta `0.34`, commit `b72b05e`): red Atalaya-256
  con espejo horizontal, +8,5 Elo [+0,8, +16,2] sobre 0.33 a 100 ms por jugada.
- Validada por El Juez (oct. 2026): comparar liberar APROBADO vs 0.33, run 20261022-204707-comparar-839470 (5551 parejas, SPRT acepta H1). (a) alinear a 64 B descartado (+0,44 % ± 0,49 % nps).
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

## Actualización (25-oct-2026)

- (b) hecho y commiteado (`0f864a2`, +1,81 % nps ± 0,29 %); sin liberar: se junta con la próxima red.
- (c) caché de acumuladores: **descartada por estimación**. El rebuild corre en ~3 % de los push; techo ~1 % nps. No compensa la complejidad.
- **Peaje de N = 384 medido** (red 256 rellena con ceros a 384: mismas salidas, nodos idénticos): **−1,93 % nps ± 0,32 %** (`banco/velocidad/n384-peaje.log`), no el ~3 % estimado. Siguiente: entrenar Atalaya-384 (parametrizar HIDDEN en `train.py`/`netfmt.py`/`quantize.py`/`nnue.rs`).

## Actualización (27-oct-2026)

- **0.35.0 (Atalaya-384) LIBERADA por El Juez**: comparar liberar APROBADO vs 0.34.0, run 20261025-105119-comparar-94ba3c (1010 parejas, sha256 516f201bf93a); solidez APTO 20261025-103256-solidez-51baae. El +24,6 de la tanda esta sesgado al alza; cifra del banco propio: +19,3 (IC 8,6..30,1).
- Release vigente: 0.35.0. Siguiente: cubos de rey con factorizador.

## Actualización (28-oct-2026)

- Cubos de rey (2) + factorizador, N=384: **descartado por validación** (0.012521 final, 0.012342 mejor en ép. 6, vs 0.012250 de la 384 sin cubos; cp 0.008935 vs 0.008664). Sobreajusta con el corpus actual. Rama `descartado/cubos-rey-kb2`. Release vigente sigue siendo 0.35.0.

## Actualización (29-oct-2026)

- **0.36.0 (LMR por `quiets_tried` + PV/improving + historia) LIBERADA por El Juez**: comparar liberar APROBADO vs 0.35.0, run 20261028-052705-comparar-e36e02 (4354 parejas, sha256 3aa98d187b18, SPRT acepta H1); solidez APTO 20261028-050319-solidez-8f9f1f. El +7,2 ± 5,5 de la tanda esta sesgado al alza; cifra del banco propio: +9,8 (IC −0,0..+19,7, 1500 parejas, `036-lmr-historia-estimacion`). Copia en `Release/Vigia 0.36.exe`.
- Aviso: `Release/vigia-036-lmr.exe` (el del banco propio) tiene otro sha256 que el liberado (misma fuente, otra compilación): el liberado es `Release/Vigia 0.36.exe`.
- Corrección: el NMP **ya era dinámico** (`R = 2 + prof/3 + min((eval−beta)/200, 3)`); lo que le falta es verificación a gran profundidad, guarda de TT y umbral que dependa de la profundidad. Siguiente: idea 2 (NMP) con 0.36.0 como base; después correction history multi-clave, ProbCut, capture history.
- Máx. 2 CPUs también para el banco y el Juez: no solapar tandas.

## Actualización (29-oct-2026, tarde): candidata 0.37.0 (velocidad pura)

- **0.37.0 = mismo árbol, +49,75 % de nodos/s ± 0,49 %** frente a 0.36.0 (`banco/velocidad/acumulado-j.log`, 12 pasadas, nodos idénticos en las 12 posiciones). Pasos y atribución en `docs/Documentacion_tecnica.md` §8.9; cada paso con su `.log` en `banco/velocidad/`. Con 2 hilos por UCI, +46 % y +41 % (una ejecución).
- Hecho: `MoveList` en pila, TT sin cerrojo + prefetch, ocupación por color cacheada, quiescencia por etapas (`Vetting`), legalidad perezosa, claves de orden `u64`, perfilador por fases (`--features perfil`). Bitset `tried_quiets` ampliado a `MAX_MOVES` (la lista es pseudolegal), con test sobre la posición de 218 jugadas.
- Descartado: atajo de SEE en quiescencia (−0,11 % ± 0,70 %, `see-shortcut-i.log`), quitado antes de commitear.
- Verificado: 422 tests (282 motor), clippy en 0 con y sin `perfil`, perft profundos (`--ignored`) OK.
- **Siguiente: El Juez** (perft completa, solidez liberar, comparar liberar 0.37.0 vs `juez036/`). Si APROBADO: copiar a `Release/Vigia 0.37.exe`, etiqueta `0.37`, anotar aquí y en `MEMORY.md`. Luego, carril de velocidad: acumulador perezoso y selección en vez de ordenación (`docs/MejorasPendientes.md`).

### Preinscripción del Juez para 0.37.0 (escrita y commiteada antes de lanzar)

- Candidato: `juez037/vigia.exe` (sha256 a77c208db78f…, «Vigia 0.37.0», commit d4c7a20 + documentación). Base: `juez036/vigia.exe` (3aa98d187b18…, «Vigia 0.36.0»).
- Cambio sometido a prueba: velocidad pura, mismo árbol (+49,75 % nodos/s ± 0,49 %; `tools/diferencial.py` idéntico en 2.528 posiciones, puntuación/nodos/variante por profundidad; ver §8.9 del documento técnico).
- Si gana el candidato (APROBADO): liberar 0.37.0 → `Release/Vigia 0.37.exe`, etiqueta `0.37`, anotar en MEMORY.md, Relevo.md y la documentación; seguir con el carril de velocidad (acumulador perezoso, selección en vez de ordenación) con 0.37.0 de base.
- Si gana la base (RECHAZADO): no liberar. Sería inesperado con +50 % de velocidad a árbol idéntico: investigar antes de nada (carga del Juez, relojes, TT bajo carga) y no tocar la búsqueda.
- Si no hay decisión: no liberar y no relanzar a ciegas; mirar profundidad media y relojes del veredicto para entender por qué un +50 % de velocidad no decide; una sola ampliación.
