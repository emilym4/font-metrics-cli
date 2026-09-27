//! A minimal RFC 1951 DEFLATE decoder, written for WOFF's zlib-wrapped table
//! data. The rest of this tool has no dependencies, and decompressing a
//! `.woff` shouldn't be the exception, so this implements the format
//! directly instead of pulling in a compression crate.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    UnexpectedEnd,
    InvalidBlockType(u8),
    InvalidStoredBlockLength,
    InvalidHuffmanCode,
    InvalidDistance,
}

impl std::fmt::Display for InflateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InflateError::UnexpectedEnd => write!(f, "truncated deflate stream"),
            InflateError::InvalidBlockType(t) => write!(f, "invalid deflate block type {t}"),
            InflateError::InvalidStoredBlockLength => write!(f, "stored block length/complement mismatch"),
            InflateError::InvalidHuffmanCode => write!(f, "invalid huffman code in deflate stream"),
            InflateError::InvalidDistance => write!(f, "back-reference distance points before the start of the output"),
        }
    }
}

impl std::error::Error for InflateError {}

/// Reads bits out of `data` least-significant-bit first within each byte,
/// which is how DEFLATE packs everything except the Huffman codes
/// themselves (see RFC 1951 section 3.1.1).
struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u32,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        BitReader { data, byte_pos: 0, bit_pos: 0 }
    }

    fn read_bit(&mut self) -> Result<u8, InflateError> {
        if self.byte_pos >= self.data.len() {
            return Err(InflateError::UnexpectedEnd);
        }
        let bit = (self.data[self.byte_pos] >> self.bit_pos) & 1;
        self.bit_pos += 1;
        if self.bit_pos == 8 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
        Ok(bit)
    }

    /// Reads `count` bits (at most 16) as a little-endian value: the first
    /// bit read becomes the least significant bit of the result.
    fn read_bits(&mut self, count: u32) -> Result<u32, InflateError> {
        let mut value = 0u32;
        for i in 0..count {
            value |= (self.read_bit()? as u32) << i;
        }
        Ok(value)
    }

    fn align_to_byte(&mut self) {
        if self.bit_pos != 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    fn read_bytes(&mut self, count: usize) -> Result<&'a [u8], InflateError> {
        let end = self.byte_pos.saturating_add(count);
        if end > self.data.len() {
            return Err(InflateError::UnexpectedEnd);
        }
        let bytes = &self.data[self.byte_pos..end];
        self.byte_pos = end;
        Ok(bytes)
    }
}

/// A canonical Huffman decode table: maps (code length in bits, code value
/// built most-significant-bit-first, per the DEFLATE convention) to the
/// symbol it represents.
struct HuffmanTable {
    codes: HashMap<(u8, u16), u16>,
}

impl HuffmanTable {
    /// Builds the canonical codes for a set of code lengths (one per
    /// symbol, 0 meaning "unused") following the algorithm in RFC 1951
    /// section 3.2.2.
    fn from_code_lengths(lengths: &[u8]) -> Self {
        let max_len = lengths.iter().copied().max().unwrap_or(0) as usize;
        let mut count = vec![0u32; max_len + 1];
        for &len in lengths {
            if len > 0 {
                count[len as usize] += 1;
            }
        }

        let mut next_code = vec![0u32; max_len + 1];
        let mut code = 0u32;
        for bits in 1..=max_len {
            code = (code + count[bits - 1]) << 1;
            next_code[bits] = code;
        }

        let mut codes = HashMap::new();
        for (symbol, &len) in lengths.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let assigned = next_code[len as usize];
            next_code[len as usize] += 1;
            codes.insert((len, assigned as u16), symbol as u16);
        }

        HuffmanTable { codes }
    }

    fn decode(&self, reader: &mut BitReader) -> Result<u16, InflateError> {
        let mut code: u16 = 0;
        for len in 1..=15u8 {
            code = (code << 1) | reader.read_bit()? as u16;
            if let Some(&symbol) = self.codes.get(&(len, code)) {
                return Ok(symbol);
            }
        }
        Err(InflateError::InvalidHuffmanCode)
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258,
];
const LENGTH_EXTRA_BITS: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097, 6145,
    8193, 12289, 16385, 24577,
];
const DIST_EXTRA_BITS: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13,
];

/// The fixed literal/length code lengths defined in RFC 1951 section 3.2.6,
/// used by BTYPE 1 blocks instead of a table read from the stream.
fn fixed_literal_table() -> HuffmanTable {
    let mut lengths = vec![0u8; 288];
    for l in lengths[0..144].iter_mut() {
        *l = 8;
    }
    for l in lengths[144..256].iter_mut() {
        *l = 9;
    }
    for l in lengths[256..280].iter_mut() {
        *l = 7;
    }
    for l in lengths[280..288].iter_mut() {
        *l = 8;
    }
    HuffmanTable::from_code_lengths(&lengths)
}

fn fixed_distance_table() -> HuffmanTable {
    HuffmanTable::from_code_lengths(&[5u8; 30])
}

/// The order code-length code lengths themselves are stored in for a
/// dynamic Huffman block (RFC 1951 section 3.2.7) - not ascending, since
/// the common ones (16-18, the repeat codes) are listed first.
const CODE_LENGTH_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn read_dynamic_tables(reader: &mut BitReader) -> Result<(HuffmanTable, HuffmanTable), InflateError> {
    let hlit = reader.read_bits(5)? as usize + 257;
    let hdist = reader.read_bits(5)? as usize + 1;
    let hclen = reader.read_bits(4)? as usize + 4;

    let mut code_length_lengths = [0u8; 19];
    for &index in CODE_LENGTH_ORDER.iter().take(hclen) {
        code_length_lengths[index] = reader.read_bits(3)? as u8;
    }
    let code_length_table = HuffmanTable::from_code_lengths(&code_length_lengths);

    let mut lengths = Vec::with_capacity(hlit + hdist);
    while lengths.len() < hlit + hdist {
        match code_length_table.decode(reader)? {
            symbol @ 0..=15 => lengths.push(symbol as u8),
            16 => {
                let repeat = reader.read_bits(2)? + 3;
                let last = *lengths.last().ok_or(InflateError::InvalidHuffmanCode)?;
                for _ in 0..repeat {
                    lengths.push(last);
                }
            }
            17 => {
                let repeat = reader.read_bits(3)? + 3;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            18 => {
                let repeat = reader.read_bits(7)? + 11;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            _ => return Err(InflateError::InvalidHuffmanCode),
        }
    }
    lengths.truncate(hlit + hdist);

    let literal_table = HuffmanTable::from_code_lengths(&lengths[..hlit]);
    let distance_table = HuffmanTable::from_code_lengths(&lengths[hlit..hlit + hdist]);
    Ok((literal_table, distance_table))
}

/// Decompresses a raw DEFLATE stream (RFC 1951, the payload inside a zlib
/// or gzip wrapper). `expected_len` is only used to preallocate the output
/// buffer; decompression always runs until an end-of-block or end-of-stream
/// marker is seen, regardless of how that compares to the caller's guess.
pub fn inflate(data: &[u8], expected_len: usize) -> Result<Vec<u8>, InflateError> {
    let mut reader = BitReader::new(data);
    let mut out = Vec::with_capacity(expected_len);

    loop {
        let is_final = reader.read_bit()? == 1;
        let block_type = reader.read_bits(2)?;

        match block_type {
            0 => {
                reader.align_to_byte();
                let len_bytes = reader.read_bytes(4)?;
                let len = u16::from_le_bytes([len_bytes[0], len_bytes[1]]);
                let nlen = u16::from_le_bytes([len_bytes[2], len_bytes[3]]);
                if len != !nlen {
                    return Err(InflateError::InvalidStoredBlockLength);
                }
                out.extend_from_slice(reader.read_bytes(len as usize)?);
            }
            1 | 2 => {
                let (literal_table, distance_table) =
                    if block_type == 1 { (fixed_literal_table(), fixed_distance_table()) } else { read_dynamic_tables(&mut reader)? };

                loop {
                    match literal_table.decode(&mut reader)? {
                        symbol @ 0..=255 => out.push(symbol as u8),
                        256 => break,
                        symbol @ 257..=285 => {
                            let index = (symbol - 257) as usize;
                            let length = LENGTH_BASE[index] as usize + reader.read_bits(LENGTH_EXTRA_BITS[index] as u32)? as usize;

                            let dist_symbol = distance_table.decode(&mut reader)? as usize;
                            let distance = *DIST_BASE.get(dist_symbol).ok_or(InflateError::InvalidDistance)? as usize
                                + reader.read_bits(*DIST_EXTRA_BITS.get(dist_symbol).ok_or(InflateError::InvalidDistance)? as u32)? as usize;

                            if distance > out.len() {
                                return Err(InflateError::InvalidDistance);
                            }
                            let start = out.len() - distance;
                            for i in 0..length {
                                out.push(out[start + i]);
                            }
                        }
                        _ => return Err(InflateError::InvalidHuffmanCode),
                    }
                }
            }
            _ => return Err(InflateError::InvalidBlockType(block_type as u8)),
        }

        if is_final {
            break;
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a single-block, stored (uncompressed) DEFLATE stream: the
    /// simplest valid encoding, and easy to build by hand for a test since
    /// it skips Huffman coding entirely.
    fn stored_block(payload: &[u8]) -> Vec<u8> {
        let mut data = vec![0x01]; // BFINAL=1, BTYPE=00, rest of the byte is padding
        let len = payload.len() as u16;
        data.extend_from_slice(&len.to_le_bytes());
        data.extend_from_slice(&(!len).to_le_bytes());
        data.extend_from_slice(payload);
        data
    }

    #[test]
    fn inflates_a_single_stored_block() {
        let data = stored_block(b"hello, deflate");
        assert_eq!(inflate(&data, 0).unwrap(), b"hello, deflate");
    }

    #[test]
    fn rejects_a_stored_block_whose_nlen_does_not_complement_len() {
        let mut data = stored_block(b"hi");
        data[3] = 0x00; // corrupt NLEN so it no longer complements LEN
        data[4] = 0x00;
        assert_eq!(inflate(&data, 0), Err(InflateError::InvalidStoredBlockLength));
    }

    #[test]
    fn rejects_a_truncated_stream() {
        let data = stored_block(b"hello")[..3].to_vec(); // cut off mid-length-field
        assert_eq!(inflate(&data, 0), Err(InflateError::UnexpectedEnd));
    }

    #[test]
    fn builds_canonical_huffman_codes_matching_the_rfc_example() {
        // RFC 1951's own worked example: symbols A-D with code lengths
        // 3, 3, 3, 3, 3, 2, 4, 4 assigns codes 010, 011, 100, 101, 110, 00,
        // 1110, 1111 in symbol order.
        let table = HuffmanTable::from_code_lengths(&[3, 3, 3, 3, 3, 2, 4, 4]);
        assert_eq!(table.codes.get(&(2, 0b00)), Some(&5));
        assert_eq!(table.codes.get(&(3, 0b010)), Some(&0));
        assert_eq!(table.codes.get(&(3, 0b110)), Some(&4));
        assert_eq!(table.codes.get(&(4, 0b1111)), Some(&7));
    }
}
