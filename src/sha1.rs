const BLOCK_LEN: usize = 64;
const INITIAL_STATE: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

pub fn hex_digest(data: &[u8]) -> String {
    digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

fn digest(data: &[u8]) -> [u8; 20] {
    let mut state = INITIAL_STATE;
    for block in pad(data).as_chunks::<BLOCK_LEN>().0 {
        compress(&mut state, block);
    }
    let mut out = [0u8; 20];
    for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        *chunk = word.to_be_bytes();
    }
    out
}

fn pad(data: &[u8]) -> Vec<u8> {
    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut message = data.to_vec();
    message.push(0x80);
    message.resize(message.len().next_multiple_of(BLOCK_LEN), 0);
    if BLOCK_LEN - (data.len() % BLOCK_LEN) <= 8 {
        message.resize(message.len() + BLOCK_LEN, 0);
    }
    let len_start = message.len() - 8;
    message[len_start..].copy_from_slice(&bit_len.to_be_bytes());
    message
}

fn compress(state: &mut [u32; 5], block: &[u8; BLOCK_LEN]) {
    let mut schedule = [0u32; 80];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for i in 16..80 {
        schedule[i] = (schedule[i - 3] ^ schedule[i - 8] ^ schedule[i - 14] ^ schedule[i - 16])
            .rotate_left(1);
    }

    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (i, word) in schedule.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A827999),
            20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
            _ => (b ^ c ^ d, 0xCA62C1D6),
        };
        let next = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*word);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = next;
    }
    for (total, value) in state.iter_mut().zip([a, b, c, d, e]) {
        *total = total.wrapping_add(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        assert_eq!(hex_digest(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(
            hex_digest(b"abc"),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            hex_digest(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
    }

    #[test]
    fn block_boundary_lengths() {
        assert_eq!(
            hex_digest(&[b'a'; 55]),
            "c1c8bbdc22796e28c0e15163d20899b65621d65a"
        );
        assert_eq!(
            hex_digest(&[b'a'; 56]),
            "c2db330f6083854c99d4b5bfb6e8f29f201be699"
        );
        assert_eq!(
            hex_digest(&[b'a'; 63]),
            "03f09f5b158a7a8cdad920bddc29b81c18a551f5"
        );
        assert_eq!(
            hex_digest(&[b'a'; 64]),
            "0098ba824b5c16427bd7a1122a5a442a25ec644d"
        );
    }

    #[test]
    fn million_a() {
        assert_eq!(
            hex_digest(&vec![b'a'; 1_000_000]),
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f"
        );
    }
}
