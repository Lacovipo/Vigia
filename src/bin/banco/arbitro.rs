//! El árbitro: juega una partida entre dos procesos UCI y decide el
//! resultado con las reglas reales del motor.
//!
//! Ningún resultado depende de lo que los motores digan sobre la partida.
//! Se les pide una jugada; el resto —mate, ahogado, cincuenta jugadas,
//! repetición triple, material insuficiente, tiempo— lo determina este
//! módulo usando `vigia::movegen` y `vigia::eval`, que son exactamente las
//! reglas que usa el motor por dentro.
//!
//! # Reclamación de tablas
//!
//! Las tablas por repetición triple y por regla de cincuenta jugadas se
//! aplican **automáticamente**, sin preguntar al motor. UCI no tiene
//! comando para reclamarlas, así que cualquier alternativa exige inventarse
//! un convenio. En un banco A/B los dos bandos son el mismo motor o
//! parientes cercanos, de modo que un convenio elaborado no compra
//! precisión y sí añade una fuente de discrepancia entre ejecuciones.
//! Aplicarlas siempre es determinista y no favorece a ninguno de los dos.
//!
//! # Orden de comprobación
//!
//! Tras aplicar una jugada se comprueban primero las reglas duras y solo
//! después la adjudicación heurística. Así un mate nunca queda oculto tras
//! un abandono adjudicado.

use std::time::Duration;

use vigia::board::Board;
use vigia::eval;
use vigia::movegen;
use vigia::types::{Color, Move};

use crate::motor::{Limite, Motor, Score};
use crate::san;

/// FEN de la posición inicial, para saber si un PGN necesita etiquetas
/// `SetUp`/`FEN`.
pub const FEN_INICIAL: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resultado {
    GananBlancas,
    GananNegras,
    Tablas,
}

impl Resultado {
    pub fn pgn(self) -> &'static str {
        match self {
            Resultado::GananBlancas => "1-0",
            Resultado::GananNegras => "0-1",
            Resultado::Tablas => "1/2-1/2",
        }
    }

    /// Puntos que se lleva `color`.
    pub fn puntos(self, color: Color) -> f64 {
        match (self, color) {
            (Resultado::Tablas, _) => 0.5,
            (Resultado::GananBlancas, Color::White) | (Resultado::GananNegras, Color::Black) => 1.0,
            _ => 0.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Final {
    Mate,
    Ahogado,
    CincuentaJugadas,
    RepeticionTriple,
    MaterialInsuficiente,
    /// Adjudicada por ventaja sostenida y reconocida por ambos motores.
    AbandonoAdjudicado,
    /// Adjudicadas por igualdad sostenida y reconocida por ambos motores.
    TablasAdjudicadas,
    /// Se alcanzó el tope de plies sin conclusión: se anota como tablas.
    TopeDePlies,
    /// El motor no contestó dentro del plazo.
    Incomparecencia,
    /// El motor devolvió algo que no es una jugada legal.
    JugadaIlegal,
    /// Solo en control de tiempo real.
    TiempoAgotado,
}

impl Final {
    pub fn as_str(self) -> &'static str {
        match self {
            Final::Mate => "mate",
            Final::Ahogado => "ahogado",
            Final::CincuentaJugadas => "cincuenta_jugadas",
            Final::RepeticionTriple => "repeticion_triple",
            Final::MaterialInsuficiente => "material_insuficiente",
            Final::AbandonoAdjudicado => "abandono_adjudicado",
            Final::TablasAdjudicadas => "tablas_adjudicadas",
            Final::TopeDePlies => "tope_de_plies",
            Final::Incomparecencia => "incomparecencia",
            Final::JugadaIlegal => "jugada_ilegal",
            Final::TiempoAgotado => "tiempo_agotado",
        }
    }

    /// Un final anómalo no es un resultado de ajedrez: indica que algo va
    /// mal en el binario o en la máquina, y el orquestador lo cuenta aparte
    /// para poder abortar la tanda en vez de publicar Elo basado en cuelgues.
    pub fn es_anomalo(self) -> bool {
        matches!(self, Final::Incomparecencia | Final::JugadaIlegal)
    }
}

/// Umbrales de adjudicación. Ahorran mucho tiempo de CPU en finales ya
/// decididos, a cambio de asumir que los motores saben reconocerlos.
///
/// La adjudicación por abandono exige que **los dos** motores coincidan: el
/// que pierde debe verse por debajo de `-resign_cp` y el que gana por
/// encima de `+resign_cp`. Con un solo lado bastaría el pesimismo de un
/// motor mal calibrado para regalar partidas que aún se pueden salvar.
#[derive(Clone, Copy, Debug)]
pub struct Adjudicacion {
    pub resign_cp: Option<i32>,
    pub resign_plies: u32,
    pub tablas_cp: Option<i32>,
    pub tablas_plies: u32,
    pub tablas_desde_jugada: u16,
}

impl Adjudicacion {
    pub fn desactivada() -> Adjudicacion {
        Adjudicacion {
            resign_cp: None,
            resign_plies: 0,
            tablas_cp: None,
            tablas_plies: 0,
            tablas_desde_jugada: 0,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ConfigPartida {
    pub limite: Limite,
    pub max_plies: u32,
    /// Margen sobre el presupuesto teórico antes de dar por colgado al
    /// motor (y, en control de tiempo, antes de declarar tiempo agotado).
    pub margen: Duration,
    pub adjudicacion: Adjudicacion,
}

#[derive(Clone, Debug)]
pub struct JugadaRegistrada {
    pub uci: String,
    pub san: String,
    pub score: Option<Score>,
    pub depth: Option<u32>,
    pub ms: u64,
}

#[derive(Clone, Debug)]
pub struct Partida {
    pub resultado: Resultado,
    pub final_partida: Final,
    pub fen_inicial: String,
    pub jugadas: Vec<JugadaRegistrada>,
}

/// Ventana deslizante de puntuaciones por bando.
///
/// Un `None` (el motor devolvió `bestmove` sin anunciar puntuación
/// utilizable) **rompe** la racha en vez de saltarse el hueco. Unir
/// puntuaciones no consecutivas equivaldría a adjudicar sobre una ventana
/// que nunca se cumplió: la partida se corta antes de tiempo y el resultado
/// deja de corresponder a las reglas declaradas.
#[derive(Default, Clone, Copy)]
struct Rachas {
    bajo: [u32; 2],
    alto: [u32; 2],
    plano: [u32; 2],
}

impl Rachas {
    fn registrar(&mut self, color: Color, score: Option<Score>, adj: &Adjudicacion) {
        let i = color as usize;
        let Some(score) = score else {
            self.bajo[i] = 0;
            self.alto[i] = 0;
            self.plano[i] = 0;
            return;
        };
        let cp = score.as_cp();
        match adj.resign_cp {
            Some(umbral) => {
                self.bajo[i] = if cp <= -umbral { self.bajo[i] + 1 } else { 0 };
                self.alto[i] = if cp >= umbral { self.alto[i] + 1 } else { 0 };
            }
            None => {
                self.bajo[i] = 0;
                self.alto[i] = 0;
            }
        }
        self.plano[i] = match adj.tablas_cp {
            Some(umbral) if cp.abs() <= umbral => self.plano[i] + 1,
            _ => 0,
        };
    }
}

fn estado_terminal(board: &Board, hashes: &[u64]) -> Option<(Resultado, Final)> {
    if movegen::generate_legal_moves(board).is_empty() {
        return Some(if movegen::is_in_check(board, board.side_to_move) {
            let ganador = match board.side_to_move {
                Color::White => Resultado::GananNegras,
                Color::Black => Resultado::GananBlancas,
            };
            (ganador, Final::Mate)
        } else {
            (Resultado::Tablas, Final::Ahogado)
        });
    }
    if board.halfmove_clock >= 100 {
        return Some((Resultado::Tablas, Final::CincuentaJugadas));
    }
    if eval::is_insufficient_material(board) {
        return Some((Resultado::Tablas, Final::MaterialInsuficiente));
    }
    if hashes.iter().filter(|&&h| h == board.hash).count() >= 3 {
        return Some((Resultado::Tablas, Final::RepeticionTriple));
    }
    None
}

fn adjudicar(board: &Board, rachas: &Rachas, adj: &Adjudicacion) -> Option<(Resultado, Final)> {
    if let Some(_umbral) = adj.resign_cp {
        let n = adj.resign_plies;
        if n > 0 {
            for perdedor in [Color::White, Color::Black] {
                let ganador = perdedor.opposite();
                if rachas.bajo[perdedor as usize] >= n && rachas.alto[ganador as usize] >= n {
                    let resultado = match ganador {
                        Color::White => Resultado::GananBlancas,
                        Color::Black => Resultado::GananNegras,
                    };
                    return Some((resultado, Final::AbandonoAdjudicado));
                }
            }
        }
    }
    if adj.tablas_cp.is_some()
        && adj.tablas_plies > 0
        && board.fullmove_number >= adj.tablas_desde_jugada
        && rachas.plano[Color::White as usize] >= adj.tablas_plies
        && rachas.plano[Color::Black as usize] >= adj.tablas_plies
    {
        return Some((Resultado::Tablas, Final::TablasAdjudicadas));
    }
    None
}

fn perdedor(color: Color) -> Resultado {
    match color {
        Color::White => Resultado::GananNegras,
        Color::Black => Resultado::GananBlancas,
    }
}

/// Juega una partida completa desde `fen_inicial`.
///
/// `blancas` y `negras` deben venir ya preparados con `nueva_partida()`.
/// Un `Err` es un fallo del banco (tubería rota, FEN inválida), no un
/// resultado de ajedrez: el orquestador lo trata como pareja no completada.
pub fn jugar_partida(
    blancas: &mut Motor,
    negras: &mut Motor,
    fen_inicial: &str,
    cfg: &ConfigPartida,
) -> Result<Partida, String> {
    let mut board = Board::from_fen(fen_inicial)
        .map_err(|e| format!("apertura con FEN inválida '{fen_inicial}': {e}"))?;
    let mut hashes: Vec<u64> = vec![board.hash];
    let mut jugadas: Vec<JugadaRegistrada> = Vec::new();
    let mut uci_jugadas: Vec<String> = Vec::new();
    let mut rachas = Rachas::default();

    let (mut wtime, mut btime, winc, binc) = match cfg.limite {
        Limite::Reloj { wtime_ms, btime_ms, winc_ms, binc_ms } => {
            (wtime_ms, btime_ms, winc_ms, binc_ms)
        }
        _ => (0, 0, 0, 0),
    };

    let terminar = |resultado, final_partida, jugadas: Vec<JugadaRegistrada>| {
        Ok(Partida {
            resultado,
            final_partida,
            fen_inicial: fen_inicial.to_string(),
            jugadas,
        })
    };

    // Una apertura puede venir ya terminada (mate, ahogado, material
    // insuficiente). Comprobarlo aquí evita pedirle una jugada a un motor
    // que no la tiene.
    if let Some((r, f)) = estado_terminal(&board, &hashes) {
        return terminar(r, f, jugadas);
    }

    for _ in 0..cfg.max_plies {
        let turno = board.side_to_move;
        let (limite, plazo) = match cfg.limite {
            Limite::Reloj { .. } => {
                let restante = if turno == Color::White { wtime } else { btime };
                (
                    Limite::Reloj { wtime_ms: wtime, btime_ms: btime, winc_ms: winc, binc_ms: binc },
                    Duration::from_millis(restante) + cfg.margen,
                )
            }
            Limite::Movetime(ms) => (cfg.limite, Duration::from_millis(ms) + cfg.margen),
            // Sin cota temporal declarada, el único plazo es el margen: es
            // una red contra cuelgues, no un control de tiempo.
            Limite::Nodos(_) | Limite::Profundidad(_) => (cfg.limite, cfg.margen),
        };

        let motor = if turno == Color::White { &mut *blancas } else { &mut *negras };
        let respuesta = motor.pedir_jugada(Some(fen_inicial), &uci_jugadas, limite, plazo)?;

        let Some(respuesta) = respuesta else {
            return terminar(perdedor(turno), Final::Incomparecencia, jugadas);
        };

        if let Limite::Reloj { .. } = cfg.limite {
            let gastado = respuesta.elapsed.as_millis() as u64;
            let restante = if turno == Color::White { wtime } else { btime };
            if gastado > restante + cfg.margen.as_millis() as u64 {
                return terminar(perdedor(turno), Final::TiempoAgotado, jugadas);
            }
            let nuevo = restante.saturating_sub(gastado) + winc;
            if turno == Color::White {
                wtime = nuevo;
            } else {
                btime = nuevo;
            }
        }

        let legales = movegen::generate_legal_moves(&board);
        let Some(mv) = legales.into_iter().find(|m: &Move| m.to_string() == respuesta.uci) else {
            return terminar(perdedor(turno), Final::JugadaIlegal, jugadas);
        };

        jugadas.push(JugadaRegistrada {
            uci: respuesta.uci.clone(),
            san: san::to_san(&board, mv),
            score: respuesta.score,
            depth: respuesta.depth,
            ms: respuesta.elapsed.as_millis() as u64,
        });
        uci_jugadas.push(respuesta.uci);
        board.make_move(mv);
        hashes.push(board.hash);
        rachas.registrar(turno, respuesta.score, &cfg.adjudicacion);

        // Reglas duras primero; la adjudicación no puede tapar un mate.
        if let Some((r, f)) = estado_terminal(&board, &hashes) {
            return terminar(r, f, jugadas);
        }
        if let Some((r, f)) = adjudicar(&board, &rachas, &cfg.adjudicacion) {
            return terminar(r, f, jugadas);
        }
    }

    terminar(Resultado::Tablas, Final::TopeDePlies, jugadas)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adj_por_defecto() -> Adjudicacion {
        Adjudicacion {
            resign_cp: Some(800),
            resign_plies: 3,
            tablas_cp: Some(10),
            tablas_plies: 4,
            tablas_desde_jugada: 40,
        }
    }

    #[test]
    fn checkmate_is_detected_from_the_position_alone() {
        let board = Board::from_fen("r1bqkbnr/pppp1Qpp/2n5/4p3/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 1").unwrap();
        assert_eq!(
            estado_terminal(&board, &[board.hash]),
            Some((Resultado::GananBlancas, Final::Mate))
        );
    }

    #[test]
    fn stalemate_is_a_draw_not_a_win() {
        let board = Board::from_fen("7k/5Q2/6K1/8/8/8/8/8 b - - 0 1").unwrap();
        assert_eq!(estado_terminal(&board, &[board.hash]), Some((Resultado::Tablas, Final::Ahogado)));
    }

    #[test]
    fn the_fifty_move_rule_fires_at_a_hundred_halfmoves() {
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 100 80").unwrap();
        assert_eq!(
            estado_terminal(&board, &[board.hash]),
            Some((Resultado::Tablas, Final::CincuentaJugadas))
        );
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 99 80").unwrap();
        assert_eq!(estado_terminal(&board, &[board.hash]), None);
    }

    #[test]
    fn checkmate_wins_over_the_fifty_move_rule() {
        // Mate del pasillo con el contador de cincuenta jugadas ya agotado:
        // manda el mate, porque las reglas duras se comprueban en ese orden.
        let board = Board::from_fen("R5k1/5ppp/8/8/8/8/8/6K1 b - - 100 90").unwrap();
        assert_eq!(estado_terminal(&board, &[board.hash]), Some((Resultado::GananBlancas, Final::Mate)));
    }

    #[test]
    fn a_third_repetition_is_a_draw() {
        let board = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 10 30").unwrap();
        let hashes = vec![board.hash, 1, board.hash, 2, board.hash];
        assert_eq!(estado_terminal(&board, &hashes), Some((Resultado::Tablas, Final::RepeticionTriple)));
        let dos_veces = vec![board.hash, 1, board.hash];
        assert_eq!(estado_terminal(&board, &dos_veces), None);
    }

    #[test]
    fn bare_kings_end_the_game_as_a_draw() {
        let board = Board::from_fen("4k3/8/8/8/8/8/8/4K3 w - - 0 1").unwrap();
        assert_eq!(
            estado_terminal(&board, &[board.hash]),
            Some((Resultado::Tablas, Final::MaterialInsuficiente))
        );
    }

    #[test]
    fn a_resign_needs_both_engines_to_agree() {
        let adj = adj_por_defecto();
        let board = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 50").unwrap();
        let mut rachas = Rachas::default();
        // Solo las blancas se ven ganando: no basta.
        for _ in 0..4 {
            rachas.registrar(Color::White, Some(Score::Cp(900)), &adj);
            rachas.registrar(Color::Black, Some(Score::Cp(-100)), &adj);
        }
        assert_eq!(adjudicar(&board, &rachas, &adj), None);
        // Ahora las negras también se ven perdidas.
        for _ in 0..3 {
            rachas.registrar(Color::White, Some(Score::Cp(900)), &adj);
            rachas.registrar(Color::Black, Some(Score::Cp(-900)), &adj);
        }
        assert_eq!(
            adjudicar(&board, &rachas, &adj),
            Some((Resultado::GananBlancas, Final::AbandonoAdjudicado))
        );
    }

    #[test]
    fn a_missing_score_breaks_the_resign_window() {
        let adj = adj_por_defecto();
        let board = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 50").unwrap();
        let mut rachas = Rachas::default();
        for _ in 0..2 {
            rachas.registrar(Color::White, Some(Score::Cp(900)), &adj);
            rachas.registrar(Color::Black, Some(Score::Cp(-900)), &adj);
        }
        // Un hueco en medio: la ventana vuelve a empezar de cero.
        rachas.registrar(Color::White, None, &adj);
        rachas.registrar(Color::Black, Some(Score::Cp(-900)), &adj);
        for _ in 0..2 {
            rachas.registrar(Color::White, Some(Score::Cp(900)), &adj);
            rachas.registrar(Color::Black, Some(Score::Cp(-900)), &adj);
        }
        assert_eq!(adjudicar(&board, &rachas, &adj), None, "la ventana no debía completarse");
        rachas.registrar(Color::White, Some(Score::Cp(900)), &adj);
        rachas.registrar(Color::Black, Some(Score::Cp(-900)), &adj);
        assert!(adjudicar(&board, &rachas, &adj).is_some(), "tres consecutivas ya bastan");
    }

    #[test]
    fn a_mate_score_counts_as_a_decisive_advantage() {
        let adj = adj_por_defecto();
        let board = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 50").unwrap();
        let mut rachas = Rachas::default();
        for _ in 0..3 {
            rachas.registrar(Color::White, Some(Score::Mate(4)), &adj);
            rachas.registrar(Color::Black, Some(Score::Mate(-4)), &adj);
        }
        assert_eq!(
            adjudicar(&board, &rachas, &adj),
            Some((Resultado::GananBlancas, Final::AbandonoAdjudicado))
        );
    }

    #[test]
    fn draws_are_not_adjudicated_before_the_configured_move_number() {
        let adj = adj_por_defecto();
        let mut rachas = Rachas::default();
        for _ in 0..5 {
            rachas.registrar(Color::White, Some(Score::Cp(3)), &adj);
            rachas.registrar(Color::Black, Some(Score::Cp(-3)), &adj);
        }
        let pronto = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 20").unwrap();
        assert_eq!(adjudicar(&pronto, &rachas, &adj), None);
        let tarde = Board::from_fen("4k3/8/8/8/8/8/4P3/4K3 w - - 0 45").unwrap();
        assert_eq!(
            adjudicar(&tarde, &rachas, &adj),
            Some((Resultado::Tablas, Final::TablasAdjudicadas))
        );
    }

    #[test]
    fn disabled_adjudication_never_fires() {
        let adj = Adjudicacion::desactivada();
        let board = Board::from_fen("4k3/8/8/8/8/8/8/3QK3 w - - 0 90").unwrap();
        let mut rachas = Rachas::default();
        for _ in 0..50 {
            rachas.registrar(Color::White, Some(Score::Cp(2000)), &adj);
            rachas.registrar(Color::Black, Some(Score::Cp(-2000)), &adj);
        }
        assert_eq!(adjudicar(&board, &rachas, &adj), None);
    }

    #[test]
    fn points_add_up_to_one_between_the_two_colors() {
        for r in [Resultado::GananBlancas, Resultado::GananNegras, Resultado::Tablas] {
            assert_eq!(r.puntos(Color::White) + r.puntos(Color::Black), 1.0);
        }
    }

    #[test]
    fn anomalous_terminations_are_flagged_apart_from_chess_results() {
        assert!(Final::Incomparecencia.es_anomalo());
        assert!(Final::JugadaIlegal.es_anomalo());
        assert!(!Final::Mate.es_anomalo());
        assert!(!Final::TopeDePlies.es_anomalo());
        assert!(!Final::TiempoAgotado.es_anomalo());
    }
}
