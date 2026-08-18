# banco/

Datos del banco de pruebas. La documentación está en
[`../docs/BancoPruebas.md`](../docs/BancoPruebas.md); el código, en
`../src/bin/banco/`.

- `configs/` — configuraciones de experimento, versionadas. Empieza por
  `plantilla.json`, que lleva cada campo comentado.
- `libros/` — libros de aperturas, versionados. `vigia-256.epd` es el que se
  usa por defecto; su cabecera registra origen, filtros y semilla, así que
  es auditable sin consultar nada más.
- `resultados/` — salidas de las tandas. **Fuera del repositorio**: pesan y
  se regeneran desde el manifiesto, que sí queda descrito en la
  documentación de cada experimento.

```bash
cargo build --release
./target/release/banco.exe ayuda
./target/release/banco.exe humo --motor target/release/vigia.exe
```
