# Notas de trabajo (Vigía en BurroCode)

- Release vigente **0.35.0** (Atalaya-384) LIBERADA por El Juez: comparar liberar APROBADO vs 0.34.0 (run 20261025-105119-comparar-94ba3c, sha256 516f201bf93a; 1010 parejas). Solidez APTO (run 20261025-103256-solidez-51baae). Elo de la tanda sesgado al alza (+24,6): la cifra honesta es +19,3 (IC 8,6..30,1) del banco propio.
- Anterior: 0.34.0 liberada (run 20261022-204707-comparar-839470).
- El Juez exige `perft N` por UCI (formato divide). Para el Juez: carpetas autocontenidas `juez035/`, `juez034base/` (un solo exe cada una).
- Descartado: alinear a 64 bytes (+0,44 % nps); caché de acumuladores (techo ~1 %).
- Hecho: rebuild_half_after por juego de instrucciones (+1,81 % nps), N=384 (peaje -1,93 % nps).
- Siguiente: cubos de rey con factorizador (docs/MejorasPendientes.md).
- Compilar/probar: `cargo test --release`, `cargo clippy --release --all-targets`, `cargo build --release`.
- Máx. 2 CPUs.
