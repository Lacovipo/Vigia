# Notas de trabajo (Vigía en BurroCode)

- Release vigente **0.35.0** (Atalaya-384) LIBERADA por El Juez: comparar liberar APROBADO vs 0.34.0 (run 20261025-105119-comparar-94ba3c, sha256 516f201bf93a; 1010 parejas). Solidez APTO (run 20261025-103256-solidez-51baae). Elo de la tanda sesgado al alza (+24,6): la cifra honesta es +19,3 (IC 8,6..30,1) del banco propio.
- Anterior: 0.34.0 liberada (run 20261022-204707-comparar-839470).
- El Juez exige `perft N` por UCI (formato divide). Para el Juez: carpetas autocontenidas `juez035/`, `juez034base/` (un solo exe cada una).
- Descartado: alinear a 64 bytes (+0,44 % nps); caché de acumuladores (techo ~1 %).
- Hecho: rebuild_half_after por juego de instrucciones (+1,81 % nps), N=384 (peaje -1,93 % nps).
- Siguiente: cubos de rey con factorizador (docs/MejorasPendientes.md).
- Compilar/probar: `cargo test --release`, `cargo clippy --release --all-targets`, `cargo build --release`.
- Máx. 2 CPUs.
- IDEAS de _Top_Ajedrez (leer `_documentación_consolidada/temas/01_BUSQUEDA.md`, `03_...`): búsqueda de Vigía básica (NMP R=2 fijo, LMR simple por move_index, corrección solo por clave de peones, cont-history 1 ply, sin ProbCut/multicut). Orden propuesto: LMR por quiets_tried + historia; NMP R dinámico (eval-beta) con verificación; correction history multi-clave (peones, menores, no peones) + uso del residuo en LMR/RFP; ProbCut (prefiltro `banco velocidad`); capture history; singular con multicut/extensión doble; tiempo adaptativo. Una por vez.
- DESCARTADO: cubos de rey (2) con factorizador sobre N=384: validación final 0.012521 (mejor 0.012342, ép. 6) frente a 0.012250 sin cubos; sobreajuste (corpus ~72 M). Código en rama `descartado/cubos-rey-kb2`; main limpio. Reabrir solo con más corpus.
- 036 LMR por quiets_tried+historia: banco 1500 parejas vs 0.35.0 = +9,8 (IC -0,0..+19,7, LOS 97,5 %), sin decision. IC toca 0 (preinscripcion decia ajustar); decision del jefe: llevar directo al Juez como 0.36.0 (el SPRT 0..5 es el arbitro real). Si no aprueba, descartar o ajustar una vez.
