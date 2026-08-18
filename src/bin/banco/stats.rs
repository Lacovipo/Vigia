//! Estadística del banco: SPRT pentanomial, Elo con intervalo y LOS.
//!
//! # Por qué parejas y no partidas
//!
//! La unidad estadística es la **pareja**: la misma apertura jugada dos
//! veces con los colores intercambiados. Sumar partidas sueltas trata como
//! independientes dos resultados que comparten posición inicial, y eso
//! infla la varianza. Con parejas, buena parte del ruido de "esta apertura
//! era mala para las negras" se cancela dentro de la propia pareja, y la
//! prueba necesita bastantes menos partidas para decidir lo mismo.
//!
//! El resultado de una pareja, visto desde el candidato, cae en uno de
//! cinco cajones (de ahí "pentanomial"), con puntuación normalizada
//! 0, ¼, ½, ¾, 1:
//!
//! | cajón | partidas | puntos |
//! |---|---|---|
//! | 0 | pierde las dos | 0 |
//! | 1 | pierde una, tablas la otra | ½ |
//! | 2 | tablas las dos, o gana una y pierde la otra | 1 |
//! | 3 | gana una, tablas la otra | 1½ |
//! | 4 | gana las dos | 2 |
//!
//! # Cómo se decide
//!
//! Prueba secuencial de razón de verosimilitudes (SPRT) sobre H0: "la
//! diferencia es `elo0`" frente a H1: "la diferencia es `elo1`". El LLR se
//! calcula con el modelo de máxima verosimilitud generalizado que usa
//! Fishtest: para cada hipótesis se busca la distribución sobre los cinco
//! cajones más cercana (en divergencia de Kullback-Leibler) a la observada
//! **con la media que impone esa hipótesis**, y se comparan. Esto exige
//! resolver una ecuación secular de una variable, que aquí se hace por
//! bisección; es la razón de que no haga falta SciPy ni ninguna otra
//! dependencia.
//!
//! Las fronteras son `log(beta/(1-alpha))` y `log((1-beta)/alpha)`.

/// Puntuación normalizada de cada cajón pentanomial.
pub const PAIR_SCORES: [f64; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];

/// Sustituto de un cajón vacío al calcular el LLR. Sin él, un recuento con
/// ceros hace que la verosimilitud bajo una hipótesis sea exactamente cero
/// y el LLR salte a infinito con una muestra ridícula. Es el mismo valor
/// que usa Fishtest.
const EMPTY_BIN_EPSILON: f64 = 1e-3;

const Z_95: f64 = 1.959_963_984_540_054;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// Cruzó la frontera superior: hay evidencia a favor de `elo1`.
    AceptaH1,
    /// Cruzó la frontera inferior: hay evidencia a favor de `elo0`.
    AceptaH0,
    /// No se ha cruzado ninguna frontera todavía.
    Continuar,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::AceptaH1 => "acepta_h1",
            Decision::AceptaH0 => "acepta_h0",
            Decision::Continuar => "continuar",
        }
    }

    pub fn is_final(self) -> bool {
        !matches!(self, Decision::Continuar)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sprt {
    pub elo0: f64,
    pub elo1: f64,
    pub alpha: f64,
    pub beta: f64,
}

impl Sprt {
    /// (frontera inferior, frontera superior) del LLR.
    pub fn bounds(&self) -> (f64, f64) {
        (
            (self.beta / (1.0 - self.alpha)).ln(),
            ((1.0 - self.beta) / self.alpha).ln(),
        )
    }

    pub fn decide(&self, llr: f64) -> Decision {
        let (lower, upper) = self.bounds();
        if llr >= upper {
            Decision::AceptaH1
        } else if llr <= lower {
            Decision::AceptaH0
        } else {
            Decision::Continuar
        }
    }
}

/// Recuento de parejas por cajón, desde el punto de vista del candidato.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pentanomial {
    pub bins: [u64; 5],
}

impl Pentanomial {
    /// Clasifica una pareja a partir de los puntos que el candidato sacó en
    /// cada una de las dos partidas (0, ½ o 1).
    pub fn bin_of(game_a: f64, game_b: f64) -> usize {
        // (0, .5, 1) + (0, .5, 1) da siempre un múltiplo de ½ entre 0 y 2.
        let total = game_a + game_b;
        (total * 2.0).round() as usize
    }

    pub fn add_pair(&mut self, game_a: f64, game_b: f64) {
        self.bins[Self::bin_of(game_a, game_b)] += 1;
    }

    pub fn pairs(&self) -> u64 {
        self.bins.iter().sum()
    }

    /// Distribución empírica cruda, sin regularizar.
    fn pdf(&self) -> Option<[f64; 5]> {
        let n = self.pairs();
        if n == 0 {
            return None;
        }
        let n = n as f64;
        let mut p = [0.0; 5];
        for (slot, &bin) in p.iter_mut().zip(self.bins.iter()) {
            *slot = bin as f64 / n;
        }
        Some(p)
    }

    /// Distribución con los cajones vacíos sustituidos por un épsilon.
    fn regularized_pdf(&self) -> Option<[f64; 5]> {
        if self.pairs() == 0 {
            return None;
        }
        let mut counts = [0.0f64; 5];
        let mut total = 0.0;
        for (slot, &bin) in counts.iter_mut().zip(self.bins.iter()) {
            *slot = if bin == 0 { EMPTY_BIN_EPSILON } else { bin as f64 };
            total += *slot;
        }
        let mut p = [0.0; 5];
        for (slot, &c) in p.iter_mut().zip(counts.iter()) {
            *slot = c / total;
        }
        Some(p)
    }

    /// Puntuación media por pareja (equivale a la puntuación media por
    /// partida) y su desviación típica.
    pub fn mean_and_stddev(&self) -> Option<(f64, f64)> {
        let p = self.pdf()?;
        let mean: f64 = (0..5).map(|i| p[i] * PAIR_SCORES[i]).sum();
        let var: f64 = (0..5)
            .map(|i| p[i] * (PAIR_SCORES[i] - mean).powi(2))
            .sum();
        Some((mean, var.sqrt()))
    }

    /// Error típico de la puntuación media.
    pub fn standard_error(&self) -> Option<f64> {
        let (_, sd) = self.mean_and_stddev()?;
        let n = self.pairs();
        if n == 0 {
            return None;
        }
        Some(sd / (n as f64).sqrt())
    }

    /// Elo estimado y sus límites al 95 %. `None` si la muestra está vacía
    /// o si el resultado es tan extremo que el Elo no es finito.
    pub fn elo_estimate(&self) -> Option<EloEstimate> {
        let (mean, _) = self.mean_and_stddev()?;
        let se = self.standard_error()?;
        Some(EloEstimate {
            score: mean,
            standard_error: se,
            elo: elo_from_score(mean),
            elo_low: elo_from_score(mean - Z_95 * se),
            elo_high: elo_from_score(mean + Z_95 * se),
            los: los_from(mean, se),
        })
    }

    /// LLR acumulado de toda la muestra.
    pub fn llr(&self, sprt: &Sprt) -> f64 {
        let Some(p) = self.regularized_pdf() else {
            return 0.0;
        };
        let n = self.pairs() as f64;
        let s0 = score_from_elo(sprt.elo0);
        let s1 = score_from_elo(sprt.elo1);
        let q0 = mle_with_mean(&p, &PAIR_SCORES, s0);
        let q1 = mle_with_mean(&p, &PAIR_SCORES, s1);
        let per_pair: f64 = (0..5).map(|i| p[i] * (q1[i] / q0[i]).ln()).sum();
        n * per_pair
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EloEstimate {
    pub score: f64,
    pub standard_error: f64,
    pub elo: Option<f64>,
    pub elo_low: Option<f64>,
    pub elo_high: Option<f64>,
    /// Probabilidad de que el candidato sea realmente mejor que la base.
    pub los: f64,
}

/// Puntuación esperada (por partida) para una diferencia de Elo, modelo
/// logístico estándar.
pub fn score_from_elo(elo: f64) -> f64 {
    1.0 / (1.0 + 10f64.powf(-elo / 400.0))
}

/// Inversa de `score_from_elo`. `None` en 0 % y 100 %, donde la fórmula
/// diverge y la muestra no dice nada útil de todos modos.
pub fn elo_from_score(score: f64) -> Option<f64> {
    if score <= 0.0 || score >= 1.0 {
        return None;
    }
    Some(-400.0 * (1.0 / score - 1.0).log10())
}

fn los_from(mean: f64, se: f64) -> f64 {
    if se <= 0.0 {
        return if mean > 0.5 {
            1.0
        } else if mean < 0.5 {
            0.0
        } else {
            0.5
        };
    }
    normal_cdf((mean - 0.5) / se)
}

/// Función de distribución normal estándar, vía la aproximación 7.1.26 de
/// Abramowitz y Stegun para `erf` (error < 1.5e-7). Sobra para un LOS que
/// se reporta con un decimal.
pub fn normal_cdf(z: f64) -> f64 {
    0.5 * (1.0 + erf(z / std::f64::consts::SQRT_2))
}

fn erf(x: f64) -> f64 {
    let sign = if x < 0.0 { -1.0 } else { 1.0 };
    let x = x.abs();
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t - 0.284_496_736) * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    sign * y
}

/// Distribución de máxima verosimilitud sobre el mismo soporte que `p`,
/// con media exactamente `s`.
///
/// La solución tiene la forma `q_i = p_i / (1 + θ·(x_i − s))`, donde θ es la
/// raíz de la ecuación secular `Σ p_i·(x_i − s)/(1 + θ·(x_i − s)) = 0`. Esa
/// forma garantiza por construcción que `Σ q_i = 1` y que `Σ q_i·x_i = s`
/// (ver los tests), así que basta con localizar θ.
fn mle_with_mean(p: &[f64; 5], x: &[f64; 5], s: f64) -> [f64; 5] {
    let a: Vec<f64> = (0..5).map(|i| x[i] - s).collect();
    let theta = solve_secular(p, &a);
    let mut q = [0.0; 5];
    for i in 0..5 {
        q[i] = p[i] / (1.0 + theta * a[i]);
    }
    q
}

/// Raíz de `f(θ) = Σ p_i·a_i/(1 + θ·a_i)`.
///
/// `f` es estrictamente decreciente dentro del intervalo donde todos los
/// `1 + θ·a_i` son positivos, y diverge a +∞ y −∞ en sus extremos, así que
/// la bisección converge siempre y sin derivadas. El intervalo lo fijan los
/// propios `a_i`: cada uno prohíbe un semieje.
fn solve_secular(p: &[f64; 5], a: &[f64]) -> f64 {
    let mut lo = f64::NEG_INFINITY;
    let mut hi = f64::INFINITY;
    for i in 0..a.len() {
        if p[i] <= 0.0 || a[i] == 0.0 {
            continue;
        }
        let limit = -1.0 / a[i];
        if a[i] > 0.0 {
            lo = lo.max(limit);
        } else {
            hi = hi.min(limit);
        }
    }
    // Con la distribución regularizada siempre hay masa por debajo y por
    // encima de la media buscada, así que el intervalo es finito. Si aun
    // así no lo fuera, θ = 0 (la propia distribución observada) es la
    // respuesta degenerada correcta.
    if !lo.is_finite() || !hi.is_finite() || lo >= hi {
        return 0.0;
    }
    // Los extremos son asíntotas: se entra un poco hacia dentro.
    let span = hi - lo;
    let mut lo = lo + span * 1e-12;
    let mut hi = hi - span * 1e-12;

    let f = |theta: f64| -> f64 {
        (0..a.len())
            .map(|i| p[i] * a[i] / (1.0 + theta * a[i]))
            .sum()
    };

    for _ in 0..200 {
        let mid = 0.5 * (lo + hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if f(mid) > 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn an_even_score_is_zero_elo_and_back() {
        assert!(approx(score_from_elo(0.0), 0.5, 1e-12));
        assert!(approx(elo_from_score(0.5).unwrap(), 0.0, 1e-9));
        for elo in [-200.0, -5.0, 0.0, 3.0, 150.0] {
            let round = elo_from_score(score_from_elo(elo)).unwrap();
            assert!(approx(round, elo, 1e-9), "ida y vuelta falló para {elo}");
        }
    }

    #[test]
    fn elo_is_unestimable_at_the_extremes() {
        assert_eq!(elo_from_score(0.0), None);
        assert_eq!(elo_from_score(1.0), None);
    }

    #[test]
    fn pair_bins_map_the_five_possible_pair_scores() {
        assert_eq!(Pentanomial::bin_of(0.0, 0.0), 0);
        assert_eq!(Pentanomial::bin_of(0.0, 0.5), 1);
        assert_eq!(Pentanomial::bin_of(0.5, 0.5), 2);
        assert_eq!(Pentanomial::bin_of(1.0, 0.0), 2);
        assert_eq!(Pentanomial::bin_of(1.0, 0.5), 3);
        assert_eq!(Pentanomial::bin_of(1.0, 1.0), 4);
    }

    #[test]
    fn the_mle_reproduces_the_analytic_two_point_solution() {
        // Soporte {0,1} con masa (½,½) y media impuesta 0,6: la solución
        // exacta es (0,4 · 0,6), calculable a mano. Es la comprobación que
        // ancla el solver a algo que no depende de esta implementación.
        let p = [0.5, 0.0, 0.0, 0.0, 0.5];
        let x = [0.0, 0.25, 0.5, 0.75, 1.0];
        let q = mle_with_mean(&p, &x, 0.6);
        assert!(approx(q[0], 0.4, 1e-9), "q0 = {}", q[0]);
        assert!(approx(q[4], 0.6, 1e-9), "q4 = {}", q[4]);
    }

    #[test]
    fn the_mle_is_a_distribution_with_exactly_the_requested_mean() {
        let p = [0.05, 0.15, 0.45, 0.25, 0.10];
        for s in [0.35, 0.5, 0.501, 0.62] {
            let q = mle_with_mean(&p, &PAIR_SCORES, s);
            let total: f64 = q.iter().sum();
            let mean: f64 = (0..5).map(|i| q[i] * PAIR_SCORES[i]).sum();
            assert!(approx(total, 1.0, 1e-9), "no suma 1 para s={s}: {total}");
            assert!(approx(mean, s, 1e-9), "media {mean} != {s}");
            assert!(q.iter().all(|&v| v > 0.0), "masa negativa para s={s}");
        }
    }

    #[test]
    fn imposing_the_observed_mean_returns_the_observed_distribution() {
        let p = [0.05, 0.15, 0.45, 0.25, 0.10];
        let observed: f64 = (0..5).map(|i| p[i] * PAIR_SCORES[i]).sum();
        let q = mle_with_mean(&p, &PAIR_SCORES, observed);
        for i in 0..5 {
            assert!(approx(q[i], p[i], 1e-9), "cajón {i}: {} != {}", q[i], p[i]);
        }
    }

    #[test]
    fn identical_hypotheses_give_zero_llr() {
        let pen = Pentanomial { bins: [10, 20, 100, 25, 12] };
        let sprt = Sprt { elo0: 3.0, elo1: 3.0, alpha: 0.05, beta: 0.05 };
        assert!(approx(pen.llr(&sprt), 0.0, 1e-9));
    }

    #[test]
    fn llr_grows_as_the_candidate_wins_more_pairs() {
        let sprt = Sprt { elo0: 0.0, elo1: 5.0, alpha: 0.05, beta: 0.05 };
        let flat = Pentanomial { bins: [10, 20, 100, 20, 10] };
        let good = Pentanomial { bins: [10, 20, 100, 25, 15] };
        let better = Pentanomial { bins: [5, 15, 100, 30, 20] };
        assert!(flat.llr(&sprt) < good.llr(&sprt));
        assert!(good.llr(&sprt) < better.llr(&sprt));
    }

    #[test]
    fn a_clearly_stronger_candidate_crosses_the_upper_bound() {
        let sprt = Sprt { elo0: 0.0, elo1: 5.0, alpha: 0.05, beta: 0.05 };
        // ~57 % de puntuación sobre 400 parejas: muy por encima de +5 Elo.
        let pen = Pentanomial { bins: [10, 40, 150, 140, 60] };
        let llr = pen.llr(&sprt);
        assert_eq!(sprt.decide(llr), Decision::AceptaH1, "LLR = {llr}");
    }

    #[test]
    fn a_clearly_weaker_candidate_crosses_the_lower_bound() {
        let sprt = Sprt { elo0: 0.0, elo1: 5.0, alpha: 0.05, beta: 0.05 };
        let pen = Pentanomial { bins: [60, 140, 150, 40, 10] };
        let llr = pen.llr(&sprt);
        assert_eq!(sprt.decide(llr), Decision::AceptaH0, "LLR = {llr}");
    }

    #[test]
    fn a_small_sample_does_not_decide_anything() {
        let sprt = Sprt { elo0: 0.0, elo1: 5.0, alpha: 0.05, beta: 0.05 };
        let pen = Pentanomial { bins: [0, 1, 4, 2, 1] };
        assert_eq!(sprt.decide(pen.llr(&sprt)), Decision::Continuar);
    }

    #[test]
    fn an_all_wins_sweep_does_not_produce_an_infinite_llr() {
        // Sin regularización, un cajón vacío mandaría el LLR a infinito con
        // ocho parejas. Debe seguir siendo un número finito.
        let sprt = Sprt { elo0: 0.0, elo1: 5.0, alpha: 0.05, beta: 0.05 };
        let pen = Pentanomial { bins: [0, 0, 0, 0, 8] };
        let llr = pen.llr(&sprt);
        assert!(llr.is_finite(), "LLR = {llr}");
    }

    #[test]
    fn symmetric_bounds_are_the_textbook_values() {
        let sprt = Sprt { elo0: 0.0, elo1: 5.0, alpha: 0.05, beta: 0.05 };
        let (lo, hi) = sprt.bounds();
        assert!(approx(hi, (0.95f64 / 0.05).ln(), 1e-12));
        assert!(approx(lo, (0.05f64 / 0.95).ln(), 1e-12));
        assert!(approx(hi, 2.944_438_979_166_44, 1e-9));
        assert!(approx(lo, -2.944_438_979_166_44, 1e-9));
    }

    #[test]
    fn the_confidence_interval_brackets_the_point_estimate() {
        let pen = Pentanomial { bins: [10, 40, 150, 140, 60] };
        let est = pen.elo_estimate().unwrap();
        let (elo, lo, hi) = (est.elo.unwrap(), est.elo_low.unwrap(), est.elo_high.unwrap());
        assert!(lo < elo && elo < hi, "{lo} < {elo} < {hi}");
        assert!(est.los > 0.99, "LOS = {}", est.los);
    }

    #[test]
    fn an_exactly_even_result_has_a_coin_flip_los() {
        let pen = Pentanomial { bins: [20, 40, 100, 40, 20] };
        let est = pen.elo_estimate().unwrap();
        assert!(approx(est.los, 0.5, 1e-6), "LOS = {}", est.los);
        assert!(approx(est.elo.unwrap(), 0.0, 1e-6));
    }

    #[test]
    fn variance_reduction_shows_up_as_a_tighter_interval_than_unpaired_play() {
        // Mismo número de partidas y misma puntuación media, pero con los
        // resultados correlacionados dentro de la pareja (cajón 2, "gana
        // una y pierde la otra") el error típico es menor que si las
        // parejas se repartieran entre 0 y 4. Es exactamente el motivo por
        // el que el banco cuenta parejas.
        let correlated = Pentanomial { bins: [0, 0, 200, 0, 0] };
        let scattered = Pentanomial { bins: [50, 0, 100, 0, 50] };
        assert!(approx(
            correlated.mean_and_stddev().unwrap().0,
            scattered.mean_and_stddev().unwrap().0,
            1e-12
        ));
        assert!(correlated.standard_error().unwrap() < scattered.standard_error().unwrap());
    }

    #[test]
    fn adding_pairs_lands_them_in_the_right_bins() {
        let mut pen = Pentanomial::default();
        pen.add_pair(1.0, 0.5);
        pen.add_pair(0.0, 0.0);
        pen.add_pair(0.5, 0.5);
        assert_eq!(pen.bins, [1, 0, 1, 1, 0]);
        assert_eq!(pen.pairs(), 3);
    }

    #[test]
    fn the_normal_cdf_matches_known_quantiles() {
        assert!(approx(normal_cdf(0.0), 0.5, 1e-9));
        assert!(approx(normal_cdf(1.959_963_98), 0.975, 1e-6));
        assert!(approx(normal_cdf(-1.959_963_98), 0.025, 1e-6));
    }
}
