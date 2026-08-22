//! Field-level parsing for the specific sfnt tables this tool reads:
//! `head`, `hhea`, and `OS/2`. Each parser takes the raw bytes of a single
//! table (already sliced out by `sfnt::table_bytes`) and returns a plain
//! struct, so they can be tested with hand-built byte arrays.

use crate::sfnt::{read_i16, read_u16, ParseError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadTable {
    pub units_per_em: u16,
    pub mac_style: u16,
}

pub fn parse_head(data: &[u8]) -> Result<HeadTable, ParseError> {
    const MIN_LEN: usize = 46;
    if data.len() < MIN_LEN {
        return Err(ParseError::UnexpectedTableLength { table: "head", needed: MIN_LEN, have: data.len() });
    }

    Ok(HeadTable {
        units_per_em: read_u16(data, 18),
        mac_style: read_u16(data, 44),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HheaTable {
    pub ascender: i16,
    pub descender: i16,
    pub line_gap: i16,
    pub number_of_h_metrics: u16,
}

pub fn parse_hhea(data: &[u8]) -> Result<HheaTable, ParseError> {
    const MIN_LEN: usize = 36;
    if data.len() < MIN_LEN {
        return Err(ParseError::UnexpectedTableLength { table: "hhea", needed: MIN_LEN, have: data.len() });
    }

    Ok(HheaTable {
        ascender: read_i16(data, 4),
        descender: read_i16(data, 6),
        line_gap: read_i16(data, 8),
        number_of_h_metrics: read_u16(data, 34),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Os2Table {
    pub version: u16,
    // The original OS/2 version 0 table, as written by TrueType 1.0 era
    // tools, stops at usLastCharIndex (68 bytes) and never got the typo
    // and win metrics added afterward. Fonts that old exist in the wild,
    // so these are None rather than a parse error; callers fall back to
    // hhea for them.
    pub typo_ascender: Option<i16>,
    pub typo_descender: Option<i16>,
    pub typo_line_gap: Option<i16>,
    pub win_ascent: Option<u16>,
    pub win_descent: Option<u16>,
    // sxHeight / sCapHeight were only added in OS/2 version 2, so older
    // fonts (or fonts that just don't declare them) leave these unset.
    pub x_height: Option<i16>,
    pub cap_height: Option<i16>,
}

pub fn parse_os2(data: &[u8]) -> Result<Os2Table, ParseError> {
    // 68 bytes covers every OS/2 version ever shipped, including the
    // truncated version 0 tables that predate usWinAscent/usWinDescent.
    const MIN_LEN: usize = 68;
    if data.len() < MIN_LEN {
        return Err(ParseError::UnexpectedTableLength { table: "OS/2", needed: MIN_LEN, have: data.len() });
    }

    let version = read_u16(data, 0);

    const TYPO_AND_WIN_MIN_LEN: usize = 78;
    let (typo_ascender, typo_descender, typo_line_gap, win_ascent, win_descent) =
        if data.len() >= TYPO_AND_WIN_MIN_LEN {
            (
                Some(read_i16(data, 68)),
                Some(read_i16(data, 70)),
                Some(read_i16(data, 72)),
                Some(read_u16(data, 74)),
                Some(read_u16(data, 76)),
            )
        } else {
            (None, None, None, None, None)
        };

    const V2_MIN_LEN: usize = 96;
    let (x_height, cap_height) = if version >= 2 && data.len() >= V2_MIN_LEN {
        (Some(read_i16(data, 86)), Some(read_i16(data, 88)))
    } else {
        (None, None)
    };

    Ok(Os2Table {
        version,
        typo_ascender,
        typo_descender,
        typo_line_gap,
        win_ascent,
        win_descent,
        x_height,
        cap_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_u16(buf: &mut [u8], offset: usize, value: u16) {
        buf[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
    }

    fn set_i16(buf: &mut [u8], offset: usize, value: i16) {
        set_u16(buf, offset, value as u16);
    }

    #[test]
    fn parses_units_per_em_and_mac_style_from_head() {
        let mut data = vec![0u8; 46];
        set_u16(&mut data, 18, 2048);
        set_u16(&mut data, 44, 1);

        let head = parse_head(&data).unwrap();
        assert_eq!(head.units_per_em, 2048);
        assert_eq!(head.mac_style, 1);
    }

    #[test]
    fn rejects_a_head_table_that_is_too_short() {
        let data = vec![0u8; 10];
        assert!(parse_head(&data).is_err());
    }

    #[test]
    fn parses_ascender_descender_and_line_gap_from_hhea() {
        let mut data = vec![0u8; 36];
        set_i16(&mut data, 4, 1000);
        set_i16(&mut data, 6, -200);
        set_i16(&mut data, 8, 90);
        set_u16(&mut data, 34, 5);

        let hhea = parse_hhea(&data).unwrap();
        assert_eq!(hhea.ascender, 1000);
        assert_eq!(hhea.descender, -200);
        assert_eq!(hhea.line_gap, 90);
        assert_eq!(hhea.number_of_h_metrics, 5);
    }

    #[test]
    fn parses_os2_version_2_including_cap_and_x_height() {
        let mut data = vec![0u8; 96];
        set_u16(&mut data, 0, 2);
        set_i16(&mut data, 68, 1000);
        set_i16(&mut data, 70, -200);
        set_i16(&mut data, 72, 0);
        set_u16(&mut data, 74, 1024);
        set_u16(&mut data, 76, 300);
        set_i16(&mut data, 86, 500);
        set_i16(&mut data, 88, 700);

        let os2 = parse_os2(&data).unwrap();
        assert_eq!(os2.version, 2);
        assert_eq!(os2.typo_ascender, Some(1000));
        assert_eq!(os2.typo_descender, Some(-200));
        assert_eq!(os2.win_ascent, Some(1024));
        assert_eq!(os2.x_height, Some(500));
        assert_eq!(os2.cap_height, Some(700));
    }

    #[test]
    fn leaves_cap_and_x_height_unset_for_os2_version_0() {
        let mut data = vec![0u8; 78];
        set_u16(&mut data, 0, 0);
        set_i16(&mut data, 68, 800);

        let os2 = parse_os2(&data).unwrap();
        assert_eq!(os2.version, 0);
        assert_eq!(os2.typo_ascender, Some(800));
        assert_eq!(os2.x_height, None);
        assert_eq!(os2.cap_height, None);
    }

    #[test]
    fn leaves_typo_and_win_metrics_unset_for_a_legacy_68_byte_os2_v0_table() {
        // The TrueType 1.0 era OS/2 layout stops right after
        // usLastCharIndex, 10 bytes short of sTypoAscender.
        let mut data = vec![0u8; 68];
        set_u16(&mut data, 0, 0);

        let os2 = parse_os2(&data).unwrap();
        assert_eq!(os2.version, 0);
        assert_eq!(os2.typo_ascender, None);
        assert_eq!(os2.typo_descender, None);
        assert_eq!(os2.typo_line_gap, None);
        assert_eq!(os2.win_ascent, None);
        assert_eq!(os2.win_descent, None);
    }

    #[test]
    fn rejects_an_os2_table_shorter_than_the_legacy_v0_layout() {
        let data = vec![0u8; 60];
        assert!(parse_os2(&data).is_err());
    }
}
