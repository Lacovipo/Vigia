//! Libro de aperturas: carga, deduplicación, barajado y generación.
//!
//! # Por qué el libro importa tanto como la estadística
//!
//! Con ocho aperturas, dos motores casi idénticos repiten las mismas
//! partidas una y otra vez: se acumulan resultados, pero no información.
//! La prueba parece avanzar y en realidad está midiendo ocho posiciones.
//! Un libro ancho y equilibrado es lo que convierte cada pareja en un dato
//! nuevo.
//!
//! # Identidad y deduplicación
//!
//! Dos aperturas son la misma unidad estadística si llevan a la misma
//! posición, aunque vengan por caminos distintos. La identidad son los
//! **cuatro primeros campos FEN** (tablero, turno, enroque, al paso); los
//! relojes no cuentan. La deduplicación se hace **antes** de barajar y
//! conserva la primera aparición en el orden original del fichero, de forma
//! que el resultado no depende de la semilla.
//!
//! Si el libro tiene menos posiciones únicas que parejas pedidas, el banco
//! lo dice y se planta: repetir aperturas reintroduce exactamente la
//! correlación que las parejas venían a eliminar.

use std::collections::HashSet;
use std::path::Path;

use vigia::board::Board;
use vigia::eval;
use vigia::movegen;

use crate::sha256;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Apertura {
    /// FEN completa de seis campos, tal y como se le pasa al motor.
    pub fen: String,
}

impl Apertura {
    /// Los cuatro primeros campos: la identidad de la posición.
    pub fn identidad(&self) -> String {
        self.fen.split_whitespace().take(4).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Debug)]
pub struct Libro {
    pub aperturas: Vec<Apertura>,
    pub ruta: String,
    pub sha256: String,
    /// Líneas con posición en el fichero de origen.
    pub leidas: usize,
    /// Cuántas quedaron tras deduplicar por identidad FEN4.
    pub unicas: usize,
}

/// Lee un fichero de posiciones (una FEN o EPD por línea) y deduplica.
///
/// Se aceptan tanto FEN de seis campos como EPD de cuatro (el formato que
/// producen la mayoría de las herramientas): a un EPD se le añaden relojes
/// `0 1`. Las líneas vacías y las que empiezan por `#` se ignoran, lo que
/// permite que un libro lleve su propia procedencia escrita en la cabecera.
pub fn cargar(ruta: &Path) -> Result<Libro, String> {
    let bytes = std::fs::read(ruta)
        .map_err(|e| format!("no se pudo leer el libro '{}': {e}", ruta.display()))?;
    let sha = sha256::hash_bytes(&bytes);
    let texto = String::from_utf8(bytes)
        .map_err(|_| format!("el libro '{}' no es UTF-8", ruta.display()))?;

    let mut aperturas: Vec<Apertura> = Vec::new();
    // HashSet y no Vec: aquí hubo un `Vec::contains`, que recorre todo lo
    // acumulado por cada línea leída y hace la carga cuadrática. Con las
    // ~28.000 posiciones del libro original eso eran un par de segundos y no
    // se notó; con un fichero de 3,8 millones son billones de comparaciones
    // de cadena y la orden no termina nunca. El orden de `aperturas` no
    // depende de esta estructura —se conserva la primera aparición tal cual—,
    // así que el libro generado es idéntico y sigue siendo reproducible.
    let mut identidades: HashSet<String> = HashSet::new();
    let mut leidas = 0usize;

    for (n, linea) in texto.lines().enumerate() {
        let linea = linea.trim();
        if linea.is_empty() || linea.starts_with('#') {
            continue;
        }
        let fen = normalizar_fen(linea)
            .ok_or_else(|| format!("{}:{}: no parece una FEN: '{linea}'", ruta.display(), n + 1))?;
        Board::from_fen(&fen).map_err(|e| {
            format!("{}:{}: FEN inválida ({e}): '{fen}'", ruta.display(), n + 1)
        })?;
        leidas += 1;
        let apertura = Apertura { fen };
        // `insert` devuelve false si ya estaba: una sola consulta en lugar de
        // consultar y volver a insertar.
        if !identidades.insert(apertura.identidad()) {
            continue;
        }
        aperturas.push(apertura);
    }

    if aperturas.is_empty() {
        return Err(format!("el libro '{}' no contiene ninguna posición", ruta.display()));
    }
    let unicas = aperturas.len();
    Ok(Libro { aperturas, ruta: ruta.display().to_string(), sha256: sha, leidas, unicas })
}

/// Deja una línea de EPD/FEN en forma de FEN de seis campos, o `None` si no
/// tiene pinta de posición. Las operaciones EPD (`bm`, `id`, ...) se
/// descartan: aquí solo interesa la posición.
fn normalizar_fen(linea: &str) -> Option<String> {
    let sin_operaciones = linea.split(';').next().unwrap_or(linea).trim();
    let campos: Vec<&str> = sin_operaciones.split_whitespace().collect();
    if campos.len() < 4 || !campos[0].contains('/') {
        return None;
    }
    let base = campos[..4].join(" ");
    let halfmove = campos.get(4).filter(|t| t.parse::<u16>().is_ok()).copied().unwrap_or("0");
    let fullmove = campos.get(5).filter(|t| t.parse::<u16>().is_ok()).copied().unwrap_or("1");
    Some(format!("{base} {halfmove} {fullmove}"))
}

/// Generador congruencial splitmix64: barajar y muestrear tienen que ser
/// reproducibles a partir de la semilla que queda escrita en el manifiesto,
/// y para eso hace falta un generador propio y estable, no el del sistema.
pub struct Rng(u64);

impl Rng {
    pub fn new(semilla: u64) -> Rng {
        Rng(semilla)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Entero uniforme en `[0, n)`, por rechazo: el clásico `% n` sesga los
    /// primeros valores, y con un libro de unos cientos de posiciones ese
    /// sesgo es medible.
    pub fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0);
        let limite = u64::MAX - (u64::MAX % n);
        loop {
            let x = self.next_u64();
            if x < limite {
                return x % n;
            }
        }
    }
}

/// Barajado de Fisher-Yates. Se aplica **después** de deduplicar, así que
/// la semilla actúa siempre sobre la misma lista.
pub fn barajar<T>(items: &mut [T], semilla: u64) {
    let mut rng = Rng::new(semilla);
    for i in (1..items.len()).rev() {
        let j = rng.below(i as u64 + 1) as usize;
        items.swap(i, j);
    }
}

/// Criterios para quedarse con una posición al generar un libro.
#[derive(Clone, Copy, Debug)]
pub struct Filtros {
    /// Máxima ventaja estática tolerada, en centipeones. Una apertura muy
    /// desequilibrada mide sobre todo quién convierte ventajas, no quién
    /// juega mejor al ajedrez.
    pub max_cp: i32,
    pub min_jugada: u16,
    pub max_jugada: u16,
    /// Mínimo de piezas sobre el tablero: evita colar finales del volcado.
    pub min_piezas: u32,
}

impl Default for Filtros {
    fn default() -> Filtros {
        Filtros { max_cp: 90, min_jugada: 4, max_jugada: 12, min_piezas: 26 }
    }
}

pub struct Rechazos {
    pub fen_invalida: usize,
    pub fuera_de_jugada: usize,
    pub pocas_piezas: usize,
    pub en_jaque: usize,
    pub terminada: usize,
    pub desequilibrada: usize,
}

/// Selecciona posiciones candidatas de un fichero de origen.
///
/// El equilibrio se mide con la evaluación estática del propio motor. Es
/// una elección consciente y tiene un sesgo conocido: el libro queda
/// equilibrado *según Vigía*. Como candidato y base juegan las dos mismas
/// posiciones con los dos colores, ese sesgo se cancela en la comparación;
/// lo que no puede afirmarse es que el libro sea neutral para un motor
/// ajeno.
pub fn seleccionar(
    origen: &Libro,
    filtros: &Filtros,
) -> (Vec<Apertura>, Rechazos) {
    let mut aceptadas = Vec::new();
    let mut r = Rechazos {
        fen_invalida: 0,
        fuera_de_jugada: 0,
        pocas_piezas: 0,
        en_jaque: 0,
        terminada: 0,
        desequilibrada: 0,
    };

    for apertura in &origen.aperturas {
        let Ok(board) = Board::from_fen(&apertura.fen) else {
            r.fen_invalida += 1;
            continue;
        };
        if board.fullmove_number < filtros.min_jugada || board.fullmove_number > filtros.max_jugada {
            r.fuera_de_jugada += 1;
            continue;
        }
        if board.occupied().count() < filtros.min_piezas {
            r.pocas_piezas += 1;
            continue;
        }
        // Empezar en jaque fuerza las primeras jugadas y estrecha el
        // abanico: la posición aporta menos información de la que parece.
        if movegen::is_in_check(&board, board.side_to_move) {
            r.en_jaque += 1;
            continue;
        }
        if movegen::generate_legal_moves(&board).is_empty() {
            r.terminada += 1;
            continue;
        }
        if eval::evaluate(&board).abs() > filtros.max_cp {
            r.desequilibrada += 1;
            continue;
        }
        aceptadas.push(apertura.clone());
    }
    (aceptadas, r)
}

/// Serializa un libro con su procedencia en la cabecera, para que el
/// fichero sea auditable por sí solo sin consultar ningún manifiesto.
pub fn serializar(aperturas: &[Apertura], cabecera: &[String]) -> String {
    let mut out = String::new();
    for linea in cabecera {
        out.push_str("# ");
        out.push_str(linea);
        out.push('\n');
    }
    for apertura in aperturas {
        out.push_str(&apertura.fen);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_epd_line_without_clocks_gets_default_counters() {
        assert_eq!(
            normalizar_fen("4k3/8/8/8/8/8/8/4K3 w - -").unwrap(),
            "4k3/8/8/8/8/8/8/4K3 w - - 0 1"
        );
    }

    #[test]
    fn epd_operations_are_dropped() {
        assert_eq!(
            normalizar_fen("4k3/8/8/8/8/8/8/4K3 w - - ; bm Kd2; id \"x\"").unwrap(),
            "4k3/8/8/8/8/8/8/4K3 w - - 0 1"
        );
    }

    #[test]
    fn a_six_field_fen_keeps_its_clocks() {
        assert_eq!(
            normalizar_fen("4k3/8/8/8/8/8/8/4K3 b - - 7 33").unwrap(),
            "4k3/8/8/8/8/8/8/4K3 b - - 7 33"
        );
    }

    #[test]
    fn a_line_that_is_not_a_position_is_rejected() {
        assert_eq!(normalizar_fen("hola que tal amigo"), None);
        assert_eq!(normalizar_fen("4k3 w - -"), None);
    }

    #[test]
    fn identity_ignores_the_clocks() {
        let a = Apertura { fen: "4k3/8/8/8/8/8/8/4K3 w - - 0 1".to_string() };
        let b = Apertura { fen: "4k3/8/8/8/8/8/8/4K3 w - - 44 90".to_string() };
        assert_eq!(a.identidad(), b.identidad());
    }

    #[test]
    fn the_shuffle_is_reproducible_from_its_seed() {
        let base: Vec<u32> = (0..64).collect();
        let mut a = base.clone();
        let mut b = base.clone();
        let mut c = base.clone();
        barajar(&mut a, 12345);
        barajar(&mut b, 12345);
        barajar(&mut c, 12346);
        assert_eq!(a, b, "misma semilla, mismo orden");
        assert_ne!(a, c, "semillas distintas deberían diferir");
        let mut ordenado = a.clone();
        ordenado.sort_unstable();
        assert_eq!(ordenado, base, "barajar no puede perder ni duplicar elementos");
    }

    #[test]
    fn the_random_generator_stays_in_range() {
        let mut rng = Rng::new(7);
        for n in [1u64, 2, 3, 17, 458] {
            for _ in 0..200 {
                assert!(rng.below(n) < n);
            }
        }
    }

    #[test]
    fn shuffling_a_tiny_slice_does_not_panic() {
        let mut vacio: Vec<u32> = vec![];
        barajar(&mut vacio, 1);
        let mut uno = vec![9u32];
        barajar(&mut uno, 1);
        assert_eq!(uno, vec![9]);
    }

    #[test]
    fn the_header_of_a_serialized_book_is_commented_out() {
        let aperturas = vec![Apertura { fen: "4k3/8/8/8/8/8/8/4K3 w - - 0 1".to_string() }];
        let texto = serializar(&aperturas, &["origen: x".to_string()]);
        assert!(texto.starts_with("# origen: x\n"));
        assert!(texto.contains("4k3/8/8/8/8/8/8/4K3 w - - 0 1"));
    }

    #[test]
    fn loading_deduplicates_by_position_and_keeps_the_first_occurrence() {
        let dir = std::env::temp_dir().join("banco_libro_test");
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("dupes.epd");
        std::fs::write(
            &ruta,
            "# cabecera\n\
             4k3/8/8/8/8/8/8/4K3 w - - 0 1\n\
             \n\
             4k3/8/8/8/8/8/8/4K3 w - - 9 40\n\
             4k3/8/8/8/8/8/8/3K4 w - - 0 1\n",
        )
        .unwrap();
        let libro = cargar(&ruta).unwrap();
        assert_eq!(libro.leidas, 3);
        assert_eq!(libro.unicas, 2);
        // Se conserva la primera, con sus relojes originales.
        assert_eq!(libro.aperturas[0].fen, "4k3/8/8/8/8/8/8/4K3 w - - 0 1");
        std::fs::remove_file(&ruta).unwrap();
    }

    /// FEN con blancas a mover a partir de las piezas colocadas. El índice de
    /// casilla es `fila * 8 + columna`, con la fila 0 siendo la primera.
    fn fen_con(piezas: &[(usize, char)]) -> String {
        let mut tablero = [' '; 64];
        for &(casilla, pieza) in piezas {
            tablero[casilla] = pieza;
        }
        let mut filas: Vec<String> = Vec::with_capacity(8);
        for fila in (0..8).rev() {
            let mut texto = String::new();
            let mut vacias = 0;
            for columna in 0..8 {
                match tablero[fila * 8 + columna] {
                    ' ' => vacias += 1,
                    pieza => {
                        if vacias > 0 {
                            texto.push_str(&vacias.to_string());
                            vacias = 0;
                        }
                        texto.push(pieza);
                    }
                }
            }
            if vacias > 0 {
                texto.push_str(&vacias.to_string());
            }
            filas.push(texto);
        }
        format!("{} w - - 0 1", filas.join("/"))
    }

    /// Este test existe por el coste, no por el resultado. La deduplicación se
    /// hacía con `Vec::contains`, lineal, lo que volvía la carga cuadrática:
    /// con las ~28.000 posiciones del libro original eran un par de segundos y
    /// nadie lo vio, pero con un origen de 3,8 millones la orden se quedaba
    /// horas sin terminar. Con 40.000 posiciones esto tarda décimas; si alguien
    /// vuelve a poner una búsqueda lineal son ~800 millones de comparaciones de
    /// cadena y el test pasa a tardar segundos largos, que es la señal.
    #[test]
    fn loading_a_large_source_does_not_degrade_quadratically() {
        // Reyes en h1 y a8: ni adyacentes ni en jaque. Los peones blancos van
        // por las filas 2 a 6 —fuera de la 7—, así que jamás dan jaque al rey
        // negro ni quedan a un paso de coronar, y toda combinación es legal.
        const REYES: [(usize, char); 2] = [(7, 'K'), (56, 'k')];
        const OBJETIVO: usize = 40_000;
        let casillas: Vec<usize> = (8..48).collect();
        let mut fens: Vec<String> = Vec::with_capacity(OBJETIVO);
        'generado: for a in 0..casillas.len() {
            for b in (a + 1)..casillas.len() {
                for c in (b + 1)..casillas.len() {
                    for d in (c + 1)..casillas.len() {
                        let mut piezas = REYES.to_vec();
                        for &i in &[a, b, c, d] {
                            piezas.push((casillas[i], 'P'));
                        }
                        fens.push(fen_con(&piezas));
                        if fens.len() == OBJETIVO {
                            break 'generado;
                        }
                    }
                }
            }
        }
        assert_eq!(fens.len(), OBJETIVO);

        // Cada posición dos veces: de paso comprueba que deduplicar sigue
        // funcionando a esta escala, y no solo con las tres líneas del test
        // de al lado.
        let mut texto = String::with_capacity(OBJETIVO * 140);
        for fen in fens.iter().chain(fens.iter()) {
            texto.push_str(fen);
            texto.push('\n');
        }

        let dir = std::env::temp_dir().join("banco_libro_test");
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("grande.epd");
        std::fs::write(&ruta, &texto).unwrap();

        let libro = cargar(&ruta).unwrap();
        assert_eq!(libro.leidas, OBJETIVO * 2);
        assert_eq!(libro.unicas, OBJETIVO);
        assert_eq!(libro.aperturas[0].fen, fens[0], "se conserva el orden del fichero");
        assert_eq!(libro.aperturas[OBJETIVO - 1].fen, fens[OBJETIVO - 1]);
        std::fs::remove_file(&ruta).unwrap();
    }

    #[test]
    fn an_illegal_fen_in_the_book_is_reported_with_its_line_number() {
        let dir = std::env::temp_dir().join("banco_libro_test");
        std::fs::create_dir_all(&dir).unwrap();
        let ruta = dir.join("mala.epd");
        std::fs::write(&ruta, "4k3/8/8/8/8/8/8/4K3 w - - 0 1\n8/8/8/8/8/8/8/8 w - - 0 1\n").unwrap();
        let err = cargar(&ruta).unwrap_err();
        assert!(err.contains(":2:"), "{err}");
        std::fs::remove_file(&ruta).unwrap();
    }

    #[test]
    fn the_filter_rejects_the_positions_it_says_it_rejects() {
        let origen = Libro {
            aperturas: vec![
                // Final de reyes pelados: pocas piezas y fuera de jugada.
                Apertura { fen: "4k3/8/8/8/8/8/8/4K3 w - - 0 40".to_string() },
                // Posición de apertura razonable.
                Apertura {
                    fen: "r1bqkbnr/pppp1ppp/2n5/4p3/2B1P3/5N2/PPPP1PPP/RNBQK2R b KQkq - 4 4"
                        .to_string(),
                },
            ],
            ruta: "memoria".to_string(),
            sha256: String::new(),
            leidas: 2,
            unicas: 2,
        };
        let (aceptadas, rechazos) = seleccionar(&origen, &Filtros::default());
        assert_eq!(aceptadas.len(), 1);
        assert!(aceptadas[0].fen.starts_with("r1bqkbnr"));
        assert_eq!(rechazos.fuera_de_jugada, 1);
    }
}
