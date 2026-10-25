# Notas de trabajo (Vigía en BurroCode)

- Release vigente 0.34.0 LIBERADA por El Juez: perft APROBADO, solidez APTO, comparar liberar APROBADO vs 0.33 (run 20261022-204707-comparar-839470, sha256 dfd90e37d24d). Siguiente: carril de velocidad (docs/Relevo.md) con 0.34.0 de base.
- Binario juzgado: `target/release/vigia.exe` (HEAD `0e6850e`, 0.34.0 + comando `perft N` / `go perft N`).
- El Juez exige `perft N` por UCI (formato divide); se añadió en `0e6850e`.
- Perft completa: APROBADO (run 20261022-194122-perft-9f7ef2).
- Descartado: alinear a 64 bytes (+0,44 % ± 0,49 % nps), commit `f07a63b`.
- Pendiente: carril de velocidad: (b) rebuild_half_after por juego de instrucciones, (c) caché de acumuladores; luego N=384.
- Compilar/probar: `cargo test --release`, `cargo clippy --release --all-targets`, `cargo build --release`.
- Máx. 2 CPUs.
