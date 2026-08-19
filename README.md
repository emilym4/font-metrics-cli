# fontmetrics

A command-line tool that reads the vertical metrics out of a TrueType or
OpenType font file: units per em, ascender/descender, line gap, and (when
present) cap height and x-height.

I wanted this because every font tool that reports these numbers is either a
GUI app, a Python script with five dependencies, or buried inside a much
bigger library. The numbers themselves come straight out of a few fixed-size
binary tables (`head`, `hhea`, `OS/2`) in the font file, so a small
standalone tool with no dependencies is enough.

## Usage

```
$ fontmetrics /System/Library/Fonts/Helvetica.ttc
```

```
units per em:     1000
hhea ascender:    952
hhea descender:   -213
hhea line gap:    0
typo ascender:    952
typo descender:   -213
typo line gap:    0
win ascent:       952
win descent:      213
cap height:       714
x-height:         523
```

If the font has no `OS/2` table (some older or specialty fonts don't), the
tool prints the `hhea` numbers and says so instead of guessing:

```
units per em:     2048
hhea ascender:    1854
hhea descender:   -434
hhea line gap:    67
OS/2 table:       not present
```

## How it works

An sfnt font file (the container format behind both `.ttf` and `.otf`)
starts with a table directory: a list of four-byte tags, each pointing at an
offset and length elsewhere in the file. `src/sfnt.rs` parses that directory
and slices out individual tables. `src/tables.rs` knows the byte layout of
the specific tables this tool cares about and turns them into plain structs.

Every parsing function takes a byte slice and returns a value or an error —
no file I/O, no globals, no hidden state. `src/main.rs` is the only place
that touches the filesystem; everything else is tested with byte arrays
built by hand in `#[cfg(test)]` modules.

## Building

Standard library only, no external crates:

```
cargo build --release
```

## Limitations (for now)

- Font collections (`.ttc`) are not parsed as collections; only the first
  font's table directory format is handled, and only if it happens to sit
  at the start of the file.
- No support for the `OS/2` version 0/1 fallback rules used by very old
  fonts that predate `usWinAscent`/`usWinDescent`.
- CFF-flavored OpenType fonts (`OTTO`) report metrics fine since `head`,
  `hhea`, and `OS/2` are the same either way, but nothing CFF-specific
  (like glyph outlines) is read.

## License

MIT, see `LICENSE`.
