//! Perfilador de muestreo por fases: herramienta de desarrollo, apagada por
//! defecto.
//!
//! Sin la característica `perfil` (la compilación normal) `enter` devuelve una
//! guarda sin tamaño y desaparece al compilar: el motor no paga nada. Con ella,
//! cada región marcada anota en una variable atómica en qué fase está el hilo
//! de búsqueda, y un hilo muestreador la lee unas decenas de miles de veces por
//! segundo. El histograma resultante es el reparto del tiempo entre fases.
//!
//! Por qué así y no con un perfilador del sistema: en Windows no hay uno a mano
//! que dé símbolos de un binario con LTO, donde casi todo está en línea dentro
//! de `negamax`. Las fases son las regiones que interesan para decidir qué
//! optimizar (generación, ordenación, evaluación, tabla de transposición...), y
//! el muestreo perturba poco: cada guarda es una lectura y una escritura
//! relajadas.
//!
//! Se usa con `cargo build --release --features perfil --bin perfil
//! --target-dir target-perfil` y `target-perfil/release/perfil.exe`.

/// Cuerpo de `negamax` (poda, reducciones, contabilidad).
pub const OTHER: u8 = 0;
/// Cuerpo de la quiescencia.
pub const QS: u8 = 1;
/// Sondeo y escritura de la tabla de transposición (cerrojo incluido).
pub const TT: u8 = 2;
/// Generación pseudolegal y pruebas de legalidad.
pub const GEN: u8 = 3;
/// Puntuación y ordenación de jugadas.
pub const ORDER: u8 = 4;
/// Intercambios estáticos (SEE).
pub const SEE: u8 = 5;
/// Evaluación (salida de la red).
pub const EVAL: u8 = 6;
/// Actualización incremental de los acumuladores de la red.
pub const PUSH: u8 = 7;
/// `make_move` y `unmake_move`.
pub const MAKE: u8 = 8;
/// `gives_check` e `is_in_check`.
pub const CHECK: u8 = 9;
/// Historias y asesinas (consulta y actualización).
pub const HIST: u8 = 10;
/// Hash de peones y corrección de la evaluación.
pub const CORR: u8 = 11;
/// Tablas por regla (repetición, cincuenta jugadas, material insuficiente).
pub const DRAW: u8 = 12;

/// Preparación de la legalidad del nodo (`check_info`: jaques y clavadas).
pub const CHECKINFO: u8 = 13;
/// ¿Hay alguna jugada legal? (quiescencia).
pub const ANYLEGAL: u8 = 14;
/// Lista legal completa (nodos de la búsqueda principal y evasiones).
pub const GENFULL: u8 = 15;
/// Capturas y promociones legales (quiescencia).
pub const GENNOISY: u8 = 16;

/// Prueba de legalidad jugada a jugada (`vet`).
pub const VET: u8 = 17;

/// Ordenación propiamente dicha (la puntuación va aparte, en `ORDER`).
pub const SORT: u8 = 18;

pub const PHASES: usize = 19;
pub const NAMES: [&str; PHASES] = [
    "negamax (resto)",
    "quiescencia (resto)",
    "tabla transposicion",
    "generacion+legalidad",
    "ordenacion",
    "SEE",
    "evaluacion (red)",
    "acumuladores (push)",
    "make/unmake",
    "jaques",
    "historias",
    "hash peones+correccion",
    "reglas de tablas",
    "check_info (jaques+clavadas)",
    "any_legal (qs)",
    "lista legal completa",
    "ruidosas legales (qs)",
    "prueba de legalidad (vet)",
    "ordenacion: sort",
];

#[cfg(feature = "perfil")]
mod on {
    use super::PHASES;
    use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
    use std::sync::Arc;
    use std::thread::JoinHandle;
    use std::time::{Duration, Instant};

    static PHASE: AtomicU8 = AtomicU8::new(super::OTHER);

    /// Restaura al salir la fase que había al entrar.
    #[must_use]
    pub struct Guard {
        previous: u8,
    }

    #[inline(always)]
    pub fn enter(phase: u8) -> Guard {
        let previous = PHASE.load(Ordering::Relaxed);
        PHASE.store(phase, Ordering::Relaxed);
        Guard { previous }
    }

    impl Drop for Guard {
        #[inline(always)]
        fn drop(&mut self) {
            PHASE.store(self.previous, Ordering::Relaxed);
        }
    }

    /// Hilo que lee la fase cada `period` y cuenta. Espera activa, no
    /// `sleep`: en Windows el sueño granular es de milisegundos.
    pub struct Sampler {
        stop: Arc<AtomicBool>,
        handle: JoinHandle<[u64; PHASES]>,
    }

    pub fn start_sampler(period: Duration) -> Sampler {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            let mut counts = [0u64; PHASES];
            let mut next = Instant::now() + period;
            while !flag.load(Ordering::Relaxed) {
                if Instant::now() >= next {
                    let phase = PHASE.load(Ordering::Relaxed) as usize;
                    counts[phase.min(PHASES - 1)] += 1;
                    next += period;
                } else {
                    std::hint::spin_loop();
                }
            }
            counts
        });
        Sampler { stop, handle }
    }

    impl Sampler {
        pub fn finish(self) -> [u64; PHASES] {
            self.stop.store(true, Ordering::Relaxed);
            self.handle.join().unwrap_or([0; PHASES])
        }
    }
}

#[cfg(feature = "perfil")]
pub use on::{enter, start_sampler, Guard, Sampler};

#[cfg(not(feature = "perfil"))]
mod off {
    pub struct Guard;

    #[inline(always)]
    pub fn enter(_phase: u8) -> Guard {
        Guard
    }
}

#[cfg(not(feature = "perfil"))]
pub use off::{enter, Guard};
