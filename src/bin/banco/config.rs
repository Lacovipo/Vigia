//! Lectura y normalización de la configuración de un experimento.
//!
//! Dos decisiones que parecen quisquillosas y no lo son:
//!
//! 1. **Las claves desconocidas son un error.** Un `max_parejs` mal escrito
//!    que se ignora en silencio deja la tanda corriendo con el valor por
//!    defecto durante horas y el fichero de configuración mintiendo sobre
//!    lo que se ejecutó. Es la clase de fallo que no se detecta nunca.
//! 2. **Lo que se firma es la configuración normalizada**, reconstruida a
//!    partir de los valores ya validados, no el texto original. Así,
//!    reordenar claves o cambiar el sangrado no rompe una reanudación, pero
//!    cambiar un umbral sí.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::arbitro::Adjudicacion;
use crate::json::Json;
use crate::motor::Limite;
use crate::stats::Sprt;

pub const ESQUEMA: u64 = 1;

/// Opciones UCI que el banco fija por defecto si la configuración no dice
/// otra cosa. Quedan escritas en el manifiesto, de modo que el fichero
/// refleja siempre lo que se le mandó al motor, no lo que el usuario
/// escribió.
const OPCIONES_POR_DEFECTO: &[(&str, &str)] = &[("Hash", "32"), ("Threads", "1")];

#[derive(Clone, Debug)]
pub struct MotorCfg {
    pub nombre: String,
    pub ruta: PathBuf,
    pub cwd: Option<PathBuf>,
    pub opciones: Vec<(String, String)>,
}

impl MotorCfg {
    pub fn hilos(&self) -> u64 {
        self.opciones
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Threads"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(1)
    }
}

#[derive(Clone, Debug)]
pub struct LibroCfg {
    pub fichero: PathBuf,
    pub semilla: u64,
    pub barajar: bool,
}

#[derive(Clone, Debug)]
pub struct MatchCfg {
    pub max_parejas: usize,
    pub workers: usize,
    pub max_plies: u32,
    pub margen_ms: u64,
    pub saludo_ms: u64,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub id: String,
    pub candidato: MotorCfg,
    pub base: MotorCfg,
    pub libro: LibroCfg,
    pub limite: Limite,
    pub partidas: MatchCfg,
    pub adjudicacion: Adjudicacion,
    pub sprt: Sprt,
    pub salida: PathBuf,
    /// Reconstrucción canónica, la que se firma y se guarda.
    pub normalizado: Json,
}

impl Config {
    pub fn margen(&self) -> Duration {
        Duration::from_millis(self.partidas.margen_ms)
    }

    pub fn saludo(&self) -> Duration {
        Duration::from_millis(self.partidas.saludo_ms)
    }

    /// Texto legible del control de búsqueda, para el PGN y los informes.
    pub fn control(&self) -> String {
        match self.limite {
            Limite::Nodos(n) => format!("{n} nodos"),
            Limite::Movetime(ms) => format!("{ms} ms/jugada"),
            Limite::Profundidad(d) => format!("profundidad {d}"),
            Limite::Reloj { wtime_ms, winc_ms, .. } => {
                format!("{:.1}+{:.2}", wtime_ms as f64 / 1000.0, winc_ms as f64 / 1000.0)
            }
        }
    }

    /// El control por nodos y por profundidad no depende del reloj de la
    /// máquina: la misma configuración da exactamente las mismas partidas.
    /// Con `movetime` o control de tiempo real, no.
    pub fn es_reproducible(&self) -> bool {
        matches!(self.limite, Limite::Nodos(_) | Limite::Profundidad(_))
    }
}

/// Lector de objetos JSON que exige que todas las claves se consuman.
struct Lector<'a> {
    entradas: &'a [(String, Json)],
    ruta: String,
    usadas: Vec<String>,
}

impl<'a> Lector<'a> {
    fn nuevo(valor: &'a Json, ruta: &str) -> Result<Lector<'a>, String> {
        let entradas = valor
            .as_obj()
            .ok_or_else(|| format!("'{ruta}' debería ser un objeto, es un {}", valor.type_name()))?;
        Ok(Lector { entradas, ruta: ruta.to_string(), usadas: Vec::new() })
    }

    fn campo(&mut self, clave: &str) -> Option<&'a Json> {
        self.usadas.push(clave.to_string());
        self.entradas.iter().find(|(k, _)| k == clave).map(|(_, v)| v)
    }

    fn ruta_de(&self, clave: &str) -> String {
        if self.ruta.is_empty() {
            clave.to_string()
        } else {
            format!("{}.{clave}", self.ruta)
        }
    }

    fn requerido(&mut self, clave: &str) -> Result<&'a Json, String> {
        let ruta = self.ruta_de(clave);
        self.campo(clave).ok_or_else(|| format!("falta el campo obligatorio '{ruta}'"))
    }

    fn cadena(&mut self, clave: &str) -> Result<String, String> {
        let ruta = self.ruta_de(clave);
        self.requerido(clave)?
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| format!("'{ruta}' debería ser una cadena"))
    }

    fn entero(&mut self, clave: &str) -> Result<u64, String> {
        let ruta = self.ruta_de(clave);
        self.requerido(clave)?
            .as_u64()
            .ok_or_else(|| format!("'{ruta}' debería ser un entero no negativo"))
    }

    fn entero_opt(&mut self, clave: &str, defecto: u64) -> Result<u64, String> {
        let ruta = self.ruta_de(clave);
        match self.campo(clave) {
            None | Some(Json::Null) => Ok(defecto),
            Some(v) => v.as_u64().ok_or_else(|| format!("'{ruta}' debería ser un entero no negativo")),
        }
    }

    fn entero_con_signo_opt(&mut self, clave: &str) -> Result<Option<i64>, String> {
        let ruta = self.ruta_de(clave);
        match self.campo(clave) {
            None | Some(Json::Null) => Ok(None),
            Some(v) => v
                .as_i64()
                .map(Some)
                .ok_or_else(|| format!("'{ruta}' debería ser un entero")),
        }
    }

    fn decimal(&mut self, clave: &str) -> Result<f64, String> {
        let ruta = self.ruta_de(clave);
        self.requerido(clave)?
            .as_f64()
            .ok_or_else(|| format!("'{ruta}' debería ser un número"))
    }

    fn booleano_opt(&mut self, clave: &str, defecto: bool) -> Result<bool, String> {
        let ruta = self.ruta_de(clave);
        match self.campo(clave) {
            None | Some(Json::Null) => Ok(defecto),
            Some(v) => v.as_bool().ok_or_else(|| format!("'{ruta}' debería ser true o false")),
        }
    }

    fn fin(self) -> Result<(), String> {
        // JSON no tiene comentarios y una configuración de experimento los
        // pide a gritos ("por qué esta semilla", "de dónde sale este
        // umbral"). Cualquier clave que empiece por '_' se ignora y sirve
        // de nota al margen.
        let sobrantes: Vec<&str> = self
            .entradas
            .iter()
            .map(|(k, _)| k.as_str())
            .filter(|k| !k.starts_with('_'))
            .filter(|k| !self.usadas.iter().any(|u| u == k))
            .collect();
        if sobrantes.is_empty() {
            return Ok(());
        }
        let donde = if self.ruta.is_empty() { "la raíz".to_string() } else { format!("'{}'", self.ruta) };
        Err(format!(
            "claves desconocidas en {donde}: {}. Las claves no reconocidas se rechazan a propósito: \
             una errata que se ignore en silencio deja la tanda corriendo con otros parámetros.",
            sobrantes.join(", ")
        ))
    }
}

pub fn desde_texto(texto: &str) -> Result<Config, String> {
    let raiz = crate::json::parse(texto)?;
    let mut lector = Lector::nuevo(&raiz, "")?;

    let esquema = lector.entero("esquema")?;
    if esquema != ESQUEMA {
        return Err(format!("esquema {esquema} desconocido; este banco usa el {ESQUEMA}"));
    }
    let id = lector.cadena("id")?;
    if id.trim().is_empty() {
        return Err("'id' no puede estar vacío: es el nombre con el que se cita el experimento".into());
    }

    let candidato = motor_cfg(lector.requerido("candidato")?, "candidato")?;
    let base = motor_cfg(lector.requerido("base")?, "base")?;
    let libro = libro_cfg(lector.requerido("libro")?)?;
    let limite = limite_cfg(lector.requerido("busqueda")?)?;
    let partidas = match_cfg(lector.requerido("partidas")?)?;
    let adjudicacion = match lector.campo("adjudicacion") {
        None | Some(Json::Null) => Adjudicacion::desactivada(),
        Some(v) => adjudicacion_cfg(v)?,
    };
    let sprt = sprt_cfg(lector.requerido("sprt")?)?;
    let salida = PathBuf::from(lector.cadena("salida")?);
    lector.fin()?;

    if partidas.workers == 0 {
        return Err("'partidas.workers' debe ser al menos 1".into());
    }
    if partidas.max_parejas == 0 {
        return Err("'partidas.max_parejas' debe ser al menos 1".into());
    }
    let hilos = candidato.hilos().max(base.hilos());
    if hilos > 1 && partidas.workers > 1 {
        return Err(format!(
            "'Threads' vale {hilos} y hay {} workers: eso pide {} CPUs y la medición se convierte \
             en una prueba de contención del planificador. Con varios workers, Threads debe ser 1.",
            partidas.workers,
            hilos * partidas.workers as u64
        ));
    }
    if sprt.elo0 >= sprt.elo1 {
        return Err(format!(
            "'sprt.elo0' ({}) debe ser menor que 'sprt.elo1' ({})",
            sprt.elo0, sprt.elo1
        ));
    }
    if !(0.0..0.5).contains(&sprt.alpha) || !(0.0..0.5).contains(&sprt.beta) {
        return Err("'sprt.alpha' y 'sprt.beta' deben estar en (0, 0.5)".into());
    }

    let normalizado = normalizar(
        &id, &candidato, &base, &libro, &limite, &partidas, &adjudicacion, &sprt,
    );

    Ok(Config {
        id,
        candidato,
        base,
        libro,
        limite,
        partidas,
        adjudicacion,
        sprt,
        salida,
        normalizado,
    })
}

pub fn desde_fichero(ruta: &Path) -> Result<Config, String> {
    let texto = std::fs::read_to_string(ruta)
        .map_err(|e| format!("no se pudo leer la configuración '{}': {e}", ruta.display()))?;
    desde_texto(&texto).map_err(|e| format!("{}: {e}", ruta.display()))
}

fn motor_cfg(valor: &Json, cual: &str) -> Result<MotorCfg, String> {
    let mut lector = Lector::nuevo(valor, cual)?;
    let nombre = lector.cadena("nombre")?;
    let ruta = PathBuf::from(lector.cadena("ruta")?);
    let cwd = match lector.campo("cwd") {
        None | Some(Json::Null) => None,
        Some(v) => Some(PathBuf::from(
            v.as_str().ok_or_else(|| format!("'{cual}.cwd' debería ser una cadena"))?,
        )),
    };
    let mut opciones: Vec<(String, String)> = Vec::new();
    if let Some(v) = lector.campo("opciones") {
        let entradas = v
            .as_obj()
            .ok_or_else(|| format!("'{cual}.opciones' debería ser un objeto nombre → valor"))?;
        for (k, val) in entradas {
            let texto = match val {
                Json::Str(s) => s.clone(),
                Json::Bool(b) => b.to_string(),
                Json::Num(_) => val.to_compact(),
                _ => {
                    return Err(format!(
                        "'{cual}.opciones.{k}' debería ser cadena, número o booleano"
                    ))
                }
            };
            opciones.push((k.clone(), texto));
        }
    }
    lector.fin()?;

    for (clave, valor) in OPCIONES_POR_DEFECTO {
        if !opciones.iter().any(|(k, _)| k.eq_ignore_ascii_case(clave)) {
            opciones.push((clave.to_string(), valor.to_string()));
        }
    }
    opciones.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(MotorCfg { nombre, ruta, cwd, opciones })
}

fn libro_cfg(valor: &Json) -> Result<LibroCfg, String> {
    let mut lector = Lector::nuevo(valor, "libro")?;
    let fichero = PathBuf::from(lector.cadena("fichero")?);
    let semilla = lector.entero("semilla")?;
    let barajar = lector.booleano_opt("barajar", true)?;
    lector.fin()?;
    Ok(LibroCfg { fichero, semilla, barajar })
}

fn limite_cfg(valor: &Json) -> Result<Limite, String> {
    let mut lector = Lector::nuevo(valor, "busqueda")?;
    let nodos = lector.entero_con_signo_opt("nodos")?;
    let movetime = lector.entero_con_signo_opt("movetime_ms")?;
    let profundidad = lector.entero_con_signo_opt("profundidad")?;
    let reloj = lector.campo("reloj").cloned();
    lector.fin()?;

    let declarados = [nodos.is_some(), movetime.is_some(), profundidad.is_some(), reloj.is_some()]
        .iter()
        .filter(|x| **x)
        .count();
    if declarados != 1 {
        return Err(
            "'busqueda' debe declarar exactamente uno de: nodos, movetime_ms, profundidad, reloj"
                .into(),
        );
    }
    if let Some(n) = nodos {
        if n <= 0 {
            return Err("'busqueda.nodos' debe ser positivo".into());
        }
        return Ok(Limite::Nodos(n as u64));
    }
    if let Some(ms) = movetime {
        if ms <= 0 {
            return Err("'busqueda.movetime_ms' debe ser positivo".into());
        }
        return Ok(Limite::Movetime(ms as u64));
    }
    if let Some(d) = profundidad {
        if d <= 0 {
            return Err("'busqueda.profundidad' debe ser positiva".into());
        }
        return Ok(Limite::Profundidad(d as u32));
    }
    let reloj = reloj.expect("comprobado arriba");
    let mut lector = Lector::nuevo(&reloj, "busqueda.reloj")?;
    let base_ms = lector.entero("base_ms")?;
    let incremento_ms = lector.entero_opt("incremento_ms", 0)?;
    lector.fin()?;
    if base_ms == 0 {
        return Err("'busqueda.reloj.base_ms' debe ser positivo".into());
    }
    Ok(Limite::Reloj {
        wtime_ms: base_ms,
        btime_ms: base_ms,
        winc_ms: incremento_ms,
        binc_ms: incremento_ms,
    })
}

fn match_cfg(valor: &Json) -> Result<MatchCfg, String> {
    let mut lector = Lector::nuevo(valor, "partidas")?;
    let max_parejas = lector.entero("max_parejas")? as usize;
    let workers = lector.entero_opt("workers", 1)? as usize;
    let max_plies = lector.entero_opt("max_plies", 300)? as u32;
    let margen_ms = lector.entero_opt("margen_ms", 10_000)?;
    let saludo_ms = lector.entero_opt("saludo_ms", 10_000)?;
    lector.fin()?;
    Ok(MatchCfg { max_parejas, workers, max_plies, margen_ms, saludo_ms })
}

fn adjudicacion_cfg(valor: &Json) -> Result<Adjudicacion, String> {
    let mut lector = Lector::nuevo(valor, "adjudicacion")?;
    let resign_cp = lector.entero_con_signo_opt("resign_cp")?;
    let resign_plies = lector.entero_opt("resign_plies", 0)? as u32;
    let tablas_cp = lector.entero_con_signo_opt("tablas_cp")?;
    let tablas_plies = lector.entero_opt("tablas_plies", 0)? as u32;
    let tablas_desde_jugada = lector.entero_opt("tablas_desde_jugada", 0)? as u16;
    lector.fin()?;

    if resign_cp.is_some() != (resign_plies > 0) {
        return Err("'adjudicacion': resign_cp y resign_plies van juntos o no van".into());
    }
    if tablas_cp.is_some() != (tablas_plies > 0) {
        return Err("'adjudicacion': tablas_cp y tablas_plies van juntos o no van".into());
    }
    Ok(Adjudicacion {
        resign_cp: resign_cp.map(|v| v as i32),
        resign_plies,
        tablas_cp: tablas_cp.map(|v| v as i32),
        tablas_plies,
        tablas_desde_jugada,
    })
}

fn sprt_cfg(valor: &Json) -> Result<Sprt, String> {
    let mut lector = Lector::nuevo(valor, "sprt")?;
    let elo0 = lector.decimal("elo0")?;
    let elo1 = lector.decimal("elo1")?;
    let alpha = lector.decimal("alpha")?;
    let beta = lector.decimal("beta")?;
    lector.fin()?;
    Ok(Sprt { elo0, elo1, alpha, beta })
}

#[allow(clippy::too_many_arguments)]
fn normalizar(
    id: &str,
    candidato: &MotorCfg,
    base: &MotorCfg,
    libro: &LibroCfg,
    limite: &Limite,
    partidas: &MatchCfg,
    adj: &Adjudicacion,
    sprt: &Sprt,
) -> Json {
    let motor_json = |m: &MotorCfg| {
        let mut j = Json::obj();
        j.set("nombre", m.nombre.as_str().into());
        j.set("ruta", m.ruta.display().to_string().into());
        j.set(
            "cwd",
            m.cwd.as_ref().map(|p| Json::Str(p.display().to_string())).unwrap_or(Json::Null),
        );
        let mut ops = Json::obj();
        for (k, v) in &m.opciones {
            ops.set(k, v.as_str().into());
        }
        j.set("opciones", ops);
        j
    };

    let mut busqueda = Json::obj();
    match *limite {
        Limite::Nodos(n) => {
            busqueda.set("tipo", "nodos".into());
            busqueda.set("nodos", n.into());
        }
        Limite::Movetime(ms) => {
            busqueda.set("tipo", "movetime".into());
            busqueda.set("movetime_ms", ms.into());
        }
        Limite::Profundidad(d) => {
            busqueda.set("tipo", "profundidad".into());
            busqueda.set("profundidad", u64::from(d).into());
        }
        Limite::Reloj { wtime_ms, winc_ms, .. } => {
            busqueda.set("tipo", "reloj".into());
            busqueda.set("base_ms", wtime_ms.into());
            busqueda.set("incremento_ms", winc_ms.into());
        }
    }

    let mut libro_json = Json::obj();
    libro_json.set("fichero", libro.fichero.display().to_string().into());
    libro_json.set("semilla", libro.semilla.into());
    libro_json.set("barajar", libro.barajar.into());

    let mut partidas_json = Json::obj();
    partidas_json.set("max_parejas", partidas.max_parejas.into());
    partidas_json.set("workers", partidas.workers.into());
    partidas_json.set("max_plies", u64::from(partidas.max_plies).into());
    partidas_json.set("margen_ms", partidas.margen_ms.into());
    partidas_json.set("saludo_ms", partidas.saludo_ms.into());

    let mut adj_json = Json::obj();
    adj_json.set("resign_cp", adj.resign_cp.map(|v| Json::Num(v as f64)).unwrap_or(Json::Null));
    adj_json.set("resign_plies", u64::from(adj.resign_plies).into());
    adj_json.set("tablas_cp", adj.tablas_cp.map(|v| Json::Num(v as f64)).unwrap_or(Json::Null));
    adj_json.set("tablas_plies", u64::from(adj.tablas_plies).into());
    adj_json.set("tablas_desde_jugada", u64::from(adj.tablas_desde_jugada).into());

    let mut sprt_json = Json::obj();
    sprt_json.set("elo0", sprt.elo0.into());
    sprt_json.set("elo1", sprt.elo1.into());
    sprt_json.set("alpha", sprt.alpha.into());
    sprt_json.set("beta", sprt.beta.into());

    let mut raiz = Json::obj();
    raiz.set("esquema", ESQUEMA.into());
    raiz.set("id", id.into());
    raiz.set("candidato", motor_json(candidato));
    raiz.set("base", motor_json(base));
    raiz.set("libro", libro_json);
    raiz.set("busqueda", busqueda);
    raiz.set("partidas", partidas_json);
    raiz.set("adjudicacion", adj_json);
    raiz.set("sprt", sprt_json);
    raiz
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMA: &str = r#"{
        "esquema": 1,
        "id": "prueba",
        "candidato": {"nombre": "cand", "ruta": "a.exe"},
        "base": {"nombre": "base", "ruta": "b.exe"},
        "libro": {"fichero": "l.epd", "semilla": 1},
        "busqueda": {"nodos": 3000},
        "partidas": {"max_parejas": 8},
        "sprt": {"elo0": 0.0, "elo1": 5.0, "alpha": 0.05, "beta": 0.05},
        "salida": "out"
    }"#;

    #[test]
    fn a_minimal_config_gets_sane_defaults() {
        let cfg = desde_texto(MINIMA).unwrap();
        assert_eq!(cfg.id, "prueba");
        assert_eq!(cfg.limite, Limite::Nodos(3000));
        assert_eq!(cfg.partidas.workers, 1);
        assert_eq!(cfg.partidas.max_plies, 300);
        assert!(cfg.libro.barajar);
        assert!(cfg.es_reproducible());
        // La adjudicación no se inventa: si no se pide, no actúa.
        assert!(cfg.adjudicacion.resign_cp.is_none());
    }

    #[test]
    fn default_uci_options_are_injected_and_visible_in_the_manifest() {
        let cfg = desde_texto(MINIMA).unwrap();
        assert_eq!(cfg.candidato.hilos(), 1);
        let ops = cfg.normalizado.get("candidato").unwrap().get("opciones").unwrap();
        assert_eq!(ops.get("Threads").unwrap().as_str(), Some("1"));
        assert_eq!(ops.get("Hash").unwrap().as_str(), Some("32"));
    }

    #[test]
    fn keys_starting_with_underscore_are_comments() {
        let texto = MINIMA.replace(
            "\"esquema\": 1,",
            "\"esquema\": 1, \"_nota\": \"la semilla sale del sorteo del 18/08\",",
        );
        assert!(desde_texto(&texto).is_ok());
    }

    #[test]
    fn an_unknown_key_is_rejected_with_its_name() {
        let texto = MINIMA.replace("\"max_parejas\": 8", "\"max_parejas\": 8, \"max_parejs\": 9");
        let err = desde_texto(&texto).unwrap_err();
        assert!(err.contains("max_parejs"), "{err}");
    }

    #[test]
    fn exactly_one_search_limit_is_required() {
        let dos = MINIMA.replace("\"nodos\": 3000", "\"nodos\": 3000, \"movetime_ms\": 100");
        assert!(desde_texto(&dos).unwrap_err().contains("exactamente uno"));
        let ninguno = MINIMA.replace("\"busqueda\": {\"nodos\": 3000}", "\"busqueda\": {}");
        assert!(desde_texto(&ninguno).unwrap_err().contains("exactamente uno"));
    }

    #[test]
    fn a_real_time_control_is_not_reproducible_and_says_so() {
        let texto = MINIMA.replace(
            "\"busqueda\": {\"nodos\": 3000}",
            "\"busqueda\": {\"reloj\": {\"base_ms\": 10000, \"incremento_ms\": 100}}",
        );
        let cfg = desde_texto(&texto).unwrap();
        assert!(!cfg.es_reproducible());
        assert_eq!(cfg.control(), "10.0+0.10");
    }

    #[test]
    fn threads_above_one_with_several_workers_is_refused() {
        let texto = MINIMA
            .replace(
                "\"candidato\": {\"nombre\": \"cand\", \"ruta\": \"a.exe\"}",
                "\"candidato\": {\"nombre\": \"cand\", \"ruta\": \"a.exe\", \"opciones\": {\"Threads\": 4}}",
            )
            .replace("\"max_parejas\": 8", "\"max_parejas\": 8, \"workers\": 4");
        let err = desde_texto(&texto).unwrap_err();
        assert!(err.contains("CPUs"), "{err}");
    }

    #[test]
    fn threads_above_one_is_allowed_with_a_single_worker() {
        let texto = MINIMA.replace(
            "\"candidato\": {\"nombre\": \"cand\", \"ruta\": \"a.exe\"}",
            "\"candidato\": {\"nombre\": \"cand\", \"ruta\": \"a.exe\", \"opciones\": {\"Threads\": 4}}",
        );
        assert_eq!(desde_texto(&texto).unwrap().candidato.hilos(), 4);
    }

    #[test]
    fn the_hypotheses_must_be_ordered() {
        let texto = MINIMA.replace("\"elo0\": 0.0, \"elo1\": 5.0", "\"elo0\": 5.0, \"elo1\": 0.0");
        assert!(desde_texto(&texto).unwrap_err().contains("menor que"));
    }

    #[test]
    fn half_an_adjudication_rule_is_rejected() {
        let texto = MINIMA.replace(
            "\"salida\": \"out\"",
            "\"adjudicacion\": {\"resign_cp\": 900}, \"salida\": \"out\"",
        );
        assert!(desde_texto(&texto).unwrap_err().contains("van juntos"));
    }

    #[test]
    fn reformatting_the_config_does_not_change_the_normalized_form() {
        let cfg = desde_texto(MINIMA).unwrap();
        let reordenada = r#"{
            "salida": "out",
            "sprt": {"beta": 0.05, "alpha": 0.05, "elo1": 5.0, "elo0": 0.0},
            "partidas": {"max_parejas": 8},
            "busqueda": {"nodos": 3000},
            "libro": {"semilla": 1, "fichero": "l.epd"},
            "base": {"ruta": "b.exe", "nombre": "base"},
            "candidato": {"ruta": "a.exe", "nombre": "cand"},
            "id": "prueba",
            "esquema": 1
        }"#;
        let otra = desde_texto(reordenada).unwrap();
        assert_eq!(cfg.normalizado.to_compact(), otra.normalizado.to_compact());
    }

    #[test]
    fn changing_a_threshold_does_change_the_normalized_form() {
        let cfg = desde_texto(MINIMA).unwrap();
        let otra = desde_texto(&MINIMA.replace("\"nodos\": 3000", "\"nodos\": 3001")).unwrap();
        assert_ne!(cfg.normalizado.to_compact(), otra.normalizado.to_compact());
    }

    #[test]
    fn a_wrong_schema_is_refused_before_anything_else() {
        let texto = MINIMA.replace("\"esquema\": 1", "\"esquema\": 7");
        assert!(desde_texto(&texto).unwrap_err().contains("esquema 7"));
    }

    #[test]
    fn uci_option_values_survive_as_text_whatever_their_json_type() {
        let texto = MINIMA.replace(
            "\"base\": {\"nombre\": \"base\", \"ruta\": \"b.exe\"}",
            "\"base\": {\"nombre\": \"base\", \"ruta\": \"b.exe\", \"opciones\": {\"Ponder\": false, \"Hash\": 64}}",
        );
        let cfg = desde_texto(&texto).unwrap();
        let ops: Vec<(String, String)> = cfg.base.opciones;
        assert!(ops.contains(&("Ponder".to_string(), "false".to_string())));
        assert!(ops.contains(&("Hash".to_string(), "64".to_string())));
    }
}
