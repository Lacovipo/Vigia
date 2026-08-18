//! SHA-256 sin dependencias externas.
//!
//! El banco de pruebas sella en el manifiesto el hash de cada binario, del
//! libro de aperturas y de la configuración normalizada. Sin ese sello, una
//! reanudación no puede distinguir "seguir el mismo experimento" de
//! "mezclar resultados de dos binarios distintos", que es exactamente el
//! error que invalida una medición sin que nadie lo note.
//!
//! Implementación directa de FIPS 180-4. No pretende ser criptografía de
//! producción (no hay resistencia a canales laterales); es un identificador
//! de contenido.

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

pub struct Sha256 {
    state: [u32; 8],
    buffer: [u8; 64],
    buffered: usize,
    total_len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Sha256 { state: H0, buffer: [0u8; 64], buffered: 0, total_len: 0 }
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256::default()
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_len = self.total_len.wrapping_add(data.len() as u64);
        if self.buffered > 0 {
            let need = 64 - self.buffered;
            let take = need.min(data.len());
            self.buffer[self.buffered..self.buffered + take].copy_from_slice(&data[..take]);
            self.buffered += take;
            data = &data[take..];
            if self.buffered < 64 {
                // No llegó ni para completar el bloque pendiente. Hay que
                // salir aquí: el camino de abajo termina reasignando
                // `buffered` a partir del resto de `data`, y con `data` ya
                // vacío eso descartaría lo que se acaba de acumular.
                debug_assert!(data.is_empty());
                return;
            }
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
        }
        let mut chunks = data.chunks_exact(64);
        for chunk in &mut chunks {
            let mut block = [0u8; 64];
            block.copy_from_slice(chunk);
            self.compress(&block);
        }
        let rest = chunks.remainder();
        self.buffer[..rest.len()].copy_from_slice(rest);
        self.buffered = rest.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        // La longitud se congela antes de rellenar: `pad_byte` no la toca.
        let bit_len = self.total_len.wrapping_mul(8);
        self.pad_byte(0x80);
        while self.buffered != 56 {
            self.pad_byte(0x00);
        }
        for &b in &bit_len.to_be_bytes() {
            self.pad_byte(b);
        }
        debug_assert_eq!(self.buffered, 0);

        let mut out = [0u8; 32];
        for (i, word) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn pad_byte(&mut self, b: u8) {
        self.buffer[self.buffered] = b;
        self.buffered += 1;
        if self.buffered == 64 {
            let block = self.buffer;
            self.compress(&block);
            self.buffered = 0;
        }
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
}

pub fn hex(digest: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push(char::from_digit((b >> 4) as u32, 16).expect("medio byte siempre cabe en base 16"));
        s.push(char::from_digit((b & 0x0f) as u32, 16).expect("medio byte siempre cabe en base 16"));
    }
    s
}

pub fn hash_bytes(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finish())
}

/// Hash de un fichero leído por bloques: los binarios del motor pesan
/// megabytes y no hay motivo para cargarlos enteros en memoria.
pub fn hash_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("no se pudo abrir '{}': {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut buf)
            .map_err(|e| format!("error leyendo '{}': {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_matches_the_published_vector() {
        assert_eq!(
            hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn abc_matches_the_published_vector() {
        assert_eq!(
            hash_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_two_block_message_matches_the_published_vector() {
        assert_eq!(
            hash_bytes(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn a_million_a_matches_the_published_vector() {
        // Vector largo: ejercita el camino de bloques completos y el
        // contador de longitud, que es donde se esconden los errores de
        // relleno.
        let mut h = Sha256::new();
        let chunk = vec![b'a'; 1000];
        for _ in 0..1000 {
            h.update(&chunk);
        }
        assert_eq!(
            hex(&h.finish()),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn updating_in_ragged_pieces_matches_a_single_update() {
        // Trocear en tamaños que no son múltiplos de 64 debe dar lo mismo:
        // el buffering parcial es la parte fácil de romper.
        let data: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let one_shot = hash_bytes(&data);
        let mut h = Sha256::new();
        for piece in data.chunks(7) {
            h.update(piece);
        }
        assert_eq!(hex(&h.finish()), one_shot);
    }
}
