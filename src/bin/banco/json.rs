//! JSON mínimo (lectura y escritura) sin dependencias externas.
//!
//! El motor no tiene dependencias y el banco de pruebas tampoco debe
//! tenerlas: si validar una mejora exigiera instalar `serde`, Python o
//! `python-chess`, el banco dejaría de ser reproducible por sí solo y
//! pasaría a depender del estado de una máquina concreta.
//!
//! Alcance deliberado: el subconjunto de JSON que usan la configuración y
//! los ficheros de resultados. No hay soporte para exponentes exóticos ni
//! para pares sustitutos `\uD800`-`\uDBFF`; un fichero que los use se
//! rechaza con un error claro en vez de aceptarse a medias.
//!
//! Los objetos conservan el **orden de inserción** en lugar de ordenar las
//! claves. Eso hace que serializar sea determinista, que es lo que permite
//! firmar la configuración normalizada con SHA-256 y comparar esa firma al
//! reanudar.

use std::fmt::Write as _;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    pub fn obj() -> Json {
        Json::Obj(Vec::new())
    }

    /// Inserta o reemplaza una clave conservando su posición original si ya
    /// existía. Reemplazar in situ (en vez de borrar y añadir al final)
    /// mantiene estable la serialización, y con ella la firma.
    pub fn set(&mut self, key: &str, value: Json) -> &mut Json {
        if let Json::Obj(entries) = self {
            match entries.iter_mut().find(|(k, _)| k == key) {
                Some(slot) => slot.1 = value,
                None => entries.push((key.to_string(), value)),
            }
        }
        self
    }

    /// Elimina una clave si existe. La usa la firma del experimento para
    /// dejar fuera los ajustes puramente operativos (workers, márgenes)
    /// que no cambian qué se está midiendo.
    pub fn remove(&mut self, key: &str) {
        if let Json::Obj(entries) = self {
            entries.retain(|(k, _)| k != key);
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Json> {
        match self {
            Json::Obj(entries) => entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// Entero no negativo. Rechaza fracciones y negativos en vez de
    /// truncarlos: un `max_parejas: 12.5` es un error de configuración, no
    /// una petición de 12.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && *n >= 0.0 && *n <= 9.007_199_254_740_992e15 => {
                Some(*n as u64)
            }
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Json::Num(n) if n.fract() == 0.0 && n.abs() <= 9.007_199_254_740_992e15 => {
                Some(*n as i64)
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_obj(&self) -> Option<&[(String, Json)]> {
        match self {
            Json::Obj(entries) => Some(entries),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "booleano",
            Json::Num(_) => "número",
            Json::Str(_) => "cadena",
            Json::Arr(_) => "lista",
            Json::Obj(_) => "objeto",
        }
    }

    pub fn to_compact(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self, None, 0);
        out
    }

    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        write_value(&mut out, self, Some(2), 0);
        out
    }
}

impl From<&str> for Json {
    fn from(s: &str) -> Json {
        Json::Str(s.to_string())
    }
}

impl From<String> for Json {
    fn from(s: String) -> Json {
        Json::Str(s)
    }
}

impl From<bool> for Json {
    fn from(b: bool) -> Json {
        Json::Bool(b)
    }
}

impl From<u64> for Json {
    fn from(n: u64) -> Json {
        Json::Num(n as f64)
    }
}

impl From<usize> for Json {
    fn from(n: usize) -> Json {
        Json::Num(n as f64)
    }
}

impl From<i64> for Json {
    fn from(n: i64) -> Json {
        Json::Num(n as f64)
    }
}

impl From<i32> for Json {
    fn from(n: i32) -> Json {
        Json::Num(n as f64)
    }
}

impl From<f64> for Json {
    fn from(n: f64) -> Json {
        Json::Num(n)
    }
}

fn write_value(out: &mut String, value: &Json, indent: Option<usize>, level: usize) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(n) => write_number(out, *n),
        Json::Str(s) => write_string(out, s),
        Json::Arr(items) => write_seq(out, items.len(), indent, level, |out, i, indent, level| {
            write_value(out, &items[i], indent, level);
        }, '[', ']'),
        Json::Obj(entries) => {
            write_seq(out, entries.len(), indent, level, |out, i, indent, level| {
                write_string(out, &entries[i].0);
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, &entries[i].1, indent, level);
            }, '{', '}')
        }
    }
}

fn write_seq(
    out: &mut String,
    len: usize,
    indent: Option<usize>,
    level: usize,
    mut item: impl FnMut(&mut String, usize, Option<usize>, usize),
    open: char,
    close: char,
) {
    out.push(open);
    if len == 0 {
        out.push(close);
        return;
    }
    for i in 0..len {
        if i > 0 {
            out.push(',');
        }
        if let Some(step) = indent {
            out.push('\n');
            for _ in 0..(level + 1) * step {
                out.push(' ');
            }
        }
        item(out, i, indent, level + 1);
    }
    if let Some(step) = indent {
        out.push('\n');
        for _ in 0..level * step {
            out.push(' ');
        }
    }
    out.push(close);
}

/// Los enteros se escriben sin parte decimal para que un fichero de
/// resultados sea legible y comparable con `diff`. NaN/infinito no son JSON
/// válido; se escriben como `null` en vez de producir un fichero que luego
/// no se puede releer.
fn write_number(out: &mut String, n: f64) {
    if !n.is_finite() {
        out.push_str("null");
    } else if n.fract() == 0.0 && n.abs() < 1e15 {
        let _ = write!(out, "{}", n as i64);
    } else {
        let _ = write!(out, "{n}");
    }
}

fn write_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

pub fn parse(text: &str) -> Result<Json, String> {
    let mut p = Parser { bytes: text.as_bytes(), pos: 0 };
    p.skip_ws();
    let value = p.value()?;
    p.skip_ws();
    if p.pos != p.bytes.len() {
        return Err(p.err("sobra texto después del valor JSON"));
    }
    Ok(value)
}

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> String {
        // Byte, no carácter: basta para localizar el problema y no obliga a
        // reescanear el fichero entero para contar líneas.
        format!("JSON inválido en el byte {}: {msg}", self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, b: u8) -> Result<(), String> {
        if self.peek() == Some(b) {
            self.pos += 1;
            Ok(())
        } else {
            Err(self.err(&format!("se esperaba '{}'", b as char)))
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, String> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.err("literal desconocido"))
        }
    }

    fn value(&mut self) -> Result<Json, String> {
        match self.peek() {
            None => Err(self.err("fin de fichero inesperado")),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') => self.literal("true", Json::Bool(true)),
            Some(b'f') => self.literal("false", Json::Bool(false)),
            Some(b'n') => self.literal("null", Json::Null),
            Some(_) => self.number(),
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.expect(b'{')?;
        let mut entries: Vec<(String, Json)> = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Json::Obj(entries));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            if entries.iter().any(|(k, _)| *k == key) {
                // Una clave repetida es casi siempre una edición a medias
                // de la configuración; aceptar la última silenciosamente
                // haría que el fichero mienta sobre lo que se ejecutó.
                return Err(self.err(&format!("clave duplicada '{key}'")));
            }
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value()?;
            entries.push((key, value));
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Json::Obj(entries));
                }
                _ => return Err(self.err("se esperaba ',' o '}'")),
            }
        }
    }

    fn array(&mut self) -> Result<Json, String> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(self.err("se esperaba ',' o ']'")),
            }
        }
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            let b = self.peek().ok_or_else(|| self.err("cadena sin cerrar"))?;
            self.pos += 1;
            match b {
                b'"' => return Ok(s),
                b'\\' => {
                    let esc = self.peek().ok_or_else(|| self.err("escape sin cerrar"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{8}'),
                        b'f' => s.push('\u{c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hex = self
                                .bytes
                                .get(self.pos..self.pos + 4)
                                .ok_or_else(|| self.err("escape \\u incompleto"))?;
                            let text = std::str::from_utf8(hex)
                                .map_err(|_| self.err("escape \\u no es ASCII"))?;
                            let code = u32::from_str_radix(text, 16)
                                .map_err(|_| self.err("escape \\u no hexadecimal"))?;
                            self.pos += 4;
                            s.push(char::from_u32(code).ok_or_else(|| {
                                self.err("escape \\u fuera de rango (pares sustitutos no soportados)")
                            })?);
                        }
                        _ => return Err(self.err("escape desconocido")),
                    }
                }
                _ if b < 0x20 => return Err(self.err("carácter de control sin escapar")),
                _ => {
                    // Byte crudo: se re-decodifica el fragmento UTF-8 completo
                    // avanzando hasta el siguiente inicio de carácter.
                    let start = self.pos - 1;
                    while matches!(self.peek(), Some(x) if x & 0xc0 == 0x80) {
                        self.pos += 1;
                    }
                    let chunk = std::str::from_utf8(&self.bytes[start..self.pos])
                        .map_err(|_| self.err("cadena con UTF-8 inválido"))?;
                    s.push_str(chunk);
                }
            }
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.pos;
        if self.peek() == Some(b'-') {
            self.pos += 1;
        }
        while matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            self.pos += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| self.err("número con bytes no ASCII"))?;
        text.parse::<f64>()
            .map(Json::Num)
            .map_err(|_| format!("JSON inválido en el byte {start}: '{text}' no es un número"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_nested_document() {
        let doc = parse(r#"{"a": 1, "b": [true, null, "x"], "c": {"d": -2.5}}"#).unwrap();
        assert_eq!(doc.get("a").unwrap().as_u64(), Some(1));
        assert_eq!(doc.get("b").unwrap().as_array().unwrap().len(), 3);
        assert_eq!(doc.get("c").unwrap().get("d").unwrap().as_f64(), Some(-2.5));
    }

    #[test]
    fn round_trips_through_compact_serialization() {
        let text = r#"{"n":[1,2,3],"s":"a\"b","t":true,"z":null,"f":1.5}"#;
        let doc = parse(text).unwrap();
        assert_eq!(doc.to_compact(), text);
    }

    #[test]
    fn pretty_output_reparses_to_the_same_value() {
        let doc = parse(r#"{"a":{"b":[1,{"c":2}]},"d":[]}"#).unwrap();
        assert_eq!(parse(&doc.to_pretty()).unwrap(), doc);
    }

    #[test]
    fn object_key_order_is_insertion_order_not_alphabetical() {
        // De esto depende que la firma del experimento sea estable.
        let mut doc = Json::obj();
        doc.set("z", 1u64.into());
        doc.set("a", 2u64.into());
        assert_eq!(doc.to_compact(), r#"{"z":1,"a":2}"#);
    }

    #[test]
    fn setting_an_existing_key_keeps_its_position() {
        let mut doc = Json::obj();
        doc.set("z", 1u64.into());
        doc.set("a", 2u64.into());
        doc.set("z", 9u64.into());
        assert_eq!(doc.to_compact(), r#"{"z":9,"a":2}"#);
    }

    #[test]
    fn duplicated_keys_are_rejected() {
        assert!(parse(r#"{"a":1,"a":2}"#).is_err());
    }

    #[test]
    fn trailing_garbage_is_rejected() {
        assert!(parse(r#"{"a":1} sobra"#).is_err());
    }

    #[test]
    fn a_fractional_count_is_not_read_as_an_integer() {
        let doc = parse(r#"{"n":12.5}"#).unwrap();
        assert_eq!(doc.get("n").unwrap().as_u64(), None);
    }

    #[test]
    fn escapes_and_non_ascii_survive_a_round_trip() {
        let doc = parse(r#"{"s":"lín\teaá \\ \" fin"}"#).unwrap();
        let s = doc.get("s").unwrap().as_str().unwrap().to_string();
        assert_eq!(s, "lín\tea\u{e1} \\ \" fin");
        assert_eq!(parse(&doc.to_compact()).unwrap().get("s").unwrap().as_str(), Some(s.as_str()));
    }

    #[test]
    fn unterminated_string_is_rejected_instead_of_looping() {
        assert!(parse(r#"{"a":"sin cerrar}"#).is_err());
    }
}
