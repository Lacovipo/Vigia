//! `banco` — banco de pruebas de Vigía.
//!
//! Responde a la pregunta que el proyecto no sabía contestar: **¿esta
//! mejora, mejora?** Un match de dieciséis partidas no distingue +20 Elo
//! del ruido, así que hasta ahora cada cambio de búsqueda o evaluación era
//! una apuesta razonada. Esto lo convierte en una medición.
//!
//! Cuatro herramientas, cada una para un tipo de cambio:
//!
//! | comando | para qué | coste |
//! |---|---|---|
//! | `sprt` | fuerza de juego (búsqueda, evaluación, tiempo) | horas |
//! | `velocidad` | cambios de solo velocidad, sin tocar decisiones | segundos |
//! | `epd` | no-regresión táctica, diagnóstico | minutos |
//! | `humo` | que el propio banco funcione | un minuto |
//!
//! `informe` y `libro` son de apoyo: rehacer el resumen de una tanda y
//! construir el libro de aperturas.
//!
//! Todo sin dependencias externas: el binario se compila con el resto del
//! proyecto y arbitra con las reglas del propio motor.

mod arbitro;
mod config;
mod epd;
mod json;
mod libro;
mod motor;
mod pgn;
mod run;
mod san;
mod sha256;
mod stats;
mod velocidad;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::motor::Limite;
use crate::stats::Decision;

const AYUDA: &str = "\
banco — banco de pruebas de Vigía

USO
  banco sprt      --config <fichero.json>
  banco informe   --run <directorio>
  banco humo      --motor <exe> [--libro <epd>] [--parejas N] [--nodos N] [--salida <dir>]
  banco velocidad --motor <exe> [--contra <exe>] [--profundidad N] [--hash MB] [--hilos N]
  banco epd       --fichero <suite.epd> --motor <exe> [--movetime N | --nodos N] [--hash MB]
  banco libro     --fuente <origen.epd> --salida <libro.epd>
                  [--n N] [--semilla N] [--max-cp N] [--min-jugada N] [--max-jugada N]
                  [--min-piezas N]

COMANDOS
  sprt       Enfrenta candidato y base con parada secuencial (SPRT pentanomial).
             Es la única forma de dar por buena una mejora de fuerza.
  informe    Recalcula resumen y PGN de una tanda ya ejecutada, releyendo
             parejas.jsonl y verificando que es un prefijo coherente.
  humo       Prueba de humo A/A: enfrenta un binario contra sí mismo. Con
             control por nodos el resultado tiene que ser empate exacto en
             todas las parejas; si no, el banco tiene un fallo.
  velocidad  Nodos y nodos/segundo a profundidad fija. Para cambios que no
             deben alterar ninguna decisión de la búsqueda.
  epd        Ejecuta una suite EPD con 'bm'/'am'. Diagnóstico, no veredicto.
  libro      Filtra un fichero de posiciones y produce un libro de aperturas
             equilibrado, deduplicado y reproducible a partir de su semilla.

CÓDIGOS DE SALIDA de 'sprt'
   0  acepta_h1 (el cambio se acepta)
  10  acepta_h0 (el cambio no llega)
  20  continuar (sin decisión: hacen falta más parejas)
   1  error

Antes de una tanda larga hay que decidir cuántas CPUs se ocupan:
'partidas.workers' es el número de partidas simultáneas y cada una usa
una CPU. El comando lo anuncia al arrancar.
";

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || argv[0] == "ayuda" || argv[0] == "--help" || argv[0] == "-h" {
        print!("{AYUDA}");
        return ExitCode::SUCCESS;
    }

    let comando = argv[0].clone();
    let args = match Args::parsear(&argv[1..]) {
        Ok(a) => a,
        Err(e) => return fallar(&e),
    };

    let resultado = match comando.as_str() {
        "sprt" => cmd_sprt(&args),
        "informe" => cmd_informe(&args),
        "humo" => cmd_humo(&args),
        "velocidad" => cmd_velocidad(&args),
        "epd" => cmd_epd(&args),
        "libro" => cmd_libro(&args),
        otro => Err(format!("comando desconocido '{otro}'. Prueba 'banco ayuda'.")),
    };

    match resultado {
        Ok(codigo) => codigo,
        Err(e) => fallar(&e),
    }
}

fn fallar(mensaje: &str) -> ExitCode {
    eprintln!("Error: {mensaje}");
    ExitCode::FAILURE
}

/// Lector de `--clave valor` que exige que todo lo pasado se use, por el
/// mismo motivo que la configuración: una opción mal escrita que se ignore
/// en silencio hace que la tanda mida otra cosa de la que se cree.
struct Args {
    valores: BTreeMap<String, String>,
    usados: std::cell::RefCell<Vec<String>>,
}

impl Args {
    fn parsear(argv: &[String]) -> Result<Args, String> {
        let mut valores = BTreeMap::new();
        let mut i = 0;
        while i < argv.len() {
            let clave = argv[i]
                .strip_prefix("--")
                .ok_or_else(|| format!("se esperaba una opción '--algo', hay '{}'", argv[i]))?;
            let valor = argv
                .get(i + 1)
                .filter(|v| !v.starts_with("--"))
                .ok_or_else(|| format!("la opción '--{clave}' necesita un valor"))?;
            if valores.insert(clave.to_string(), valor.clone()).is_some() {
                return Err(format!("la opción '--{clave}' aparece dos veces"));
            }
            i += 2;
        }
        Ok(Args { valores, usados: std::cell::RefCell::new(Vec::new()) })
    }

    fn opcional(&self, clave: &str) -> Option<&str> {
        self.usados.borrow_mut().push(clave.to_string());
        self.valores.get(clave).map(String::as_str)
    }

    fn requerido(&self, clave: &str) -> Result<&str, String> {
        self.opcional(clave).ok_or_else(|| format!("falta la opción obligatoria '--{clave}'"))
    }

    fn numero(&self, clave: &str, defecto: u64) -> Result<u64, String> {
        match self.opcional(clave) {
            None => Ok(defecto),
            Some(v) => v.parse().map_err(|_| format!("'--{clave} {v}' no es un número")),
        }
    }

    fn fin(&self) -> Result<(), String> {
        let usados = self.usados.borrow();
        let sobrantes: Vec<&str> = self
            .valores
            .keys()
            .map(String::as_str)
            .filter(|k| !usados.iter().any(|u| u == k))
            .collect();
        if sobrantes.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "opciones no reconocidas por este comando: --{}",
                sobrantes.join(", --")
            ))
        }
    }
}

fn opciones_uci(args: &Args) -> Result<Vec<(String, String)>, String> {
    Ok(vec![
        ("Hash".to_string(), args.numero("hash", 32)?.to_string()),
        ("Threads".to_string(), args.numero("hilos", 1)?.to_string()),
    ])
}

// --------------------------------------------------------------------- sprt

fn cmd_sprt(args: &Args) -> Result<ExitCode, String> {
    let ruta = PathBuf::from(args.requerido("config")?);
    args.fin()?;
    let cfg = config::desde_fichero(&ruta)?;
    let prep = run::preparar(cfg)?;
    let salida = run::ejecutar(&prep)?;
    Ok(match salida.decision {
        Decision::AceptaH1 => ExitCode::SUCCESS,
        Decision::AceptaH0 => ExitCode::from(10),
        Decision::Continuar => ExitCode::from(20),
    })
}

fn cmd_informe(args: &Args) -> Result<ExitCode, String> {
    let dir = PathBuf::from(args.requerido("run")?);
    args.fin()?;
    run::informe(&dir)?;
    Ok(ExitCode::SUCCESS)
}

// --------------------------------------------------------------------- humo

/// Prueba de humo A/A.
///
/// Con control por nodos, un hilo y `Variety` apagado, la búsqueda de Vigía
/// es determinista. Enfrentando un binario contra sí mismo, las dos
/// partidas de cada pareja son **la misma partida con los papeles
/// cambiados**: quien abre con blancas en la primera hace exactamente lo
/// mismo que quien abre con blancas en la segunda, porque ambos son el
/// mismo programa arrancando de cero (`ucinewgame` vacía la tabla). El
/// candidato gana una y pierde la otra, o entabla las dos.
///
/// Por tanto **todas** las parejas deben caer en el cajón central. Cualquier
/// otra cosa significa que algo del banco no es simétrico: un arranque que
/// contamina, un margen que corta una búsqueda, estado que sobrevive entre
/// partidas. Es la comprobación de extremo a extremo del sistema entero.
fn cmd_humo(args: &Args) -> Result<ExitCode, String> {
    let motor = args.requerido("motor")?.to_string();
    let libro_ruta = args.opcional("libro").unwrap_or("banco/libros/humo.epd").to_string();
    let parejas = args.numero("parejas", 4)?;
    let nodos = args.numero("nodos", 5_000)?;
    let salida = args
        .opcional("salida")
        .unwrap_or("banco/resultados/humo")
        .to_string();
    args.fin()?;

    let texto = format!(
        r#"{{
  "esquema": 1,
  "id": "humo-A-contra-A",
  "candidato": {{"nombre": "A", "ruta": {motor:?}}},
  "base": {{"nombre": "A-copia", "ruta": {motor:?}}},
  "libro": {{"fichero": {libro_ruta:?}, "semilla": 1}},
  "busqueda": {{"nodos": {nodos}}},
  "partidas": {{"max_parejas": {parejas}, "workers": 1, "max_plies": 200}},
  "sprt": {{"elo0": -5.0, "elo1": 0.0, "alpha": 0.05, "beta": 0.05}},
  "salida": {salida:?}
}}"#
    );

    // Una prueba de humo tiene que poder repetirse sin ceremonia.
    let dir = PathBuf::from(&salida);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)
            .map_err(|e| format!("no se pudo limpiar '{}': {e}", dir.display()))?;
    }

    let cfg = config::desde_texto(&texto)?;
    let prep = run::preparar(cfg)?;
    let resultado = run::ejecutar(&prep)?;

    let bins = resultado.acumulado.pentanomial.bins;
    let centrales = bins[2];
    let total = resultado.acumulado.pentanomial.pairs();
    println!();
    println!("--- Verificación del banco ---");
    if total == 0 {
        return Err("no se completó ninguna pareja".into());
    }
    if centrales == total && resultado.acumulado.anomalas == 0 {
        println!(
            "OK: las {total} parejas cayeron en el cajón central, como exige el enfrentamiento \
             de un binario determinista contra sí mismo.\n\
             El árbitro, el libro, la simetría de colores, la persistencia y la estadística \
             funcionan de extremo a extremo."
        );
        Ok(ExitCode::SUCCESS)
    } else {
        Err(format!(
            "FALLO: pentanomial {bins:?} con {} partidas anómalas. Un binario contra sí mismo, con \
             control por nodos y un hilo, tiene que empatar todas las parejas. Que no ocurra \
             significa que el banco introduce asimetría entre los dos bandos (o que la búsqueda no \
             es determinista a nodos fijos) — y hasta arreglarlo, ninguna medición de este banco es \
             de fiar.",
            resultado.acumulado.anomalas
        ))
    }
}

// ---------------------------------------------------------------- velocidad

fn cmd_velocidad(args: &Args) -> Result<ExitCode, String> {
    let ruta_a = PathBuf::from(args.requerido("motor")?);
    let ruta_b = args.opcional("contra").map(PathBuf::from);
    let profundidad = args.numero("profundidad", 12)? as u32;
    let opciones = opciones_uci(args)?;
    args.fin()?;

    let margen = Duration::from_secs(300);
    let nombre_a = ruta_a.display().to_string();
    let medidas_a = velocidad::medir(&ruta_a, "A", &opciones, profundidad, margen)?;
    velocidad::imprimir_una(&nombre_a, &medidas_a, profundidad);

    if let Some(ruta_b) = ruta_b {
        let nombre_b = ruta_b.display().to_string();
        let medidas_b = velocidad::medir(&ruta_b, "B", &opciones, profundidad, margen)?;
        println!();
        velocidad::imprimir_una(&nombre_b, &medidas_b, profundidad);
        velocidad::imprimir_comparacion(&nombre_a, &medidas_a, &nombre_b, &medidas_b, profundidad);
    }
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------------- epd

fn cmd_epd(args: &Args) -> Result<ExitCode, String> {
    let fichero = PathBuf::from(args.requerido("fichero")?);
    let ruta_motor = PathBuf::from(args.requerido("motor")?);
    let movetime = args.opcional("movetime").map(str::to_string);
    let nodos = args.opcional("nodos").map(str::to_string);
    let opciones = opciones_uci(args)?;
    args.fin()?;

    let limite = match (movetime, nodos) {
        (Some(_), Some(_)) => return Err("--movetime y --nodos son excluyentes".into()),
        (Some(ms), None) => Limite::Movetime(
            ms.parse().map_err(|_| format!("'--movetime {ms}' no es un número"))?,
        ),
        (None, Some(n)) => {
            Limite::Nodos(n.parse().map_err(|_| format!("'--nodos {n}' no es un número"))?)
        }
        (None, None) => Limite::Movetime(1000),
    };

    let casos = epd::cargar(&fichero)?;
    println!("{} casos en {}", casos.len(), fichero.display());
    let informe = epd::ejecutar(
        &ruta_motor,
        &ruta_motor.display().to_string(),
        &opciones,
        &casos,
        limite,
        Duration::from_secs(60),
    )?;
    epd::imprimir(&ruta_motor.display().to_string(), &informe, 25);
    println!();
    println!(
        "Recordatorio: una suite EPD no mide fuerza. Sirve para detectar derrumbes y para tener \
         posiciones concretas que mirar, no para aprobar un cambio."
    );
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------------- libro

fn cmd_libro(args: &Args) -> Result<ExitCode, String> {
    let fuente = PathBuf::from(args.requerido("fuente")?);
    let salida = PathBuf::from(args.requerido("salida")?);
    let cuantas = args.numero("n", 256)? as usize;
    let semilla = args.numero("semilla", 20_260_818)?;
    let filtros = libro::Filtros {
        max_cp: args.numero("max-cp", 90)? as i32,
        min_jugada: args.numero("min-jugada", 4)? as u16,
        max_jugada: args.numero("max-jugada", 12)? as u16,
        min_piezas: args.numero("min-piezas", 26)? as u32,
    };
    args.fin()?;

    let origen = libro::cargar(&fuente)?;
    println!(
        "{}: {} líneas, {} posiciones únicas (sha256 {})",
        fuente.display(),
        origen.leidas,
        origen.unicas,
        &origen.sha256[..16]
    );

    let (mut aceptadas, rechazos) = libro::seleccionar(&origen, &filtros);
    println!(
        "Filtradas: {} aceptadas. Descartes — fuera de jugada {}, pocas piezas {}, en jaque {}, \
         terminadas {}, desequilibradas {}, FEN inválida {}.",
        aceptadas.len(),
        rechazos.fuera_de_jugada,
        rechazos.pocas_piezas,
        rechazos.en_jaque,
        rechazos.terminada,
        rechazos.desequilibrada,
        rechazos.fen_invalida
    );
    if aceptadas.len() < cuantas {
        return Err(format!(
            "solo {} posiciones pasan los filtros y se piden {cuantas}. Afloja '--max-cp' o \
             ensancha el rango de jugadas, o pide menos posiciones.",
            aceptadas.len()
        ));
    }

    libro::barajar(&mut aceptadas, semilla);
    aceptadas.truncate(cuantas);

    let cabecera = vec![
        format!("libro de aperturas para el banco de pruebas de Vigía — {}", run::marca_de_tiempo()),
        format!("generado por {}", run::BANCO_VERSION),
        format!("origen: {}", fuente.display()),
        format!("sha256 del origen: {}", origen.sha256),
        format!("líneas del origen: {} — únicas por FEN4: {}", origen.leidas, origen.unicas),
        format!(
            "filtros: |eval| <= {} cp, jugadas {}-{}, >= {} piezas, no en jaque, no terminada",
            filtros.max_cp, filtros.min_jugada, filtros.max_jugada, filtros.min_piezas
        ),
        format!("semilla de muestreo: {semilla} — posiciones: {cuantas}"),
        "el equilibrio se mide con la evaluación estática de Vigía; ver docs/BancoPruebas.md".to_string(),
    ];
    let texto = libro::serializar(&aceptadas, &cabecera);
    if let Some(padre) = salida.parent() {
        std::fs::create_dir_all(padre)
            .map_err(|e| format!("no se pudo crear '{}': {e}", padre.display()))?;
    }
    std::fs::write(&salida, texto)
        .map_err(|e| format!("no se pudo escribir '{}': {e}", salida.display()))?;
    println!("Escrito {} con {} posiciones.", salida.display(), aceptadas.len());
    Ok(ExitCode::SUCCESS)
}
