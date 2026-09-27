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

`.ttc` files bundle several fonts behind one `ttcf` header; by default the
tool reads the first one. Pass `--index` (or `-i`) to pick another:

```
$ fontmetrics --index 1 /System/Library/Fonts/Helvetica.ttc
```

```
outline format:   TrueType (glyf outlines)
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

`outline format` is read straight off the table directory: a `CFF2` or `CFF `
table means PostScript outlines, `glyf` means TrueType outlines. It doesn't
require parsing the outline data itself, and the rest of the report is
identical either way since `head`, `hhea`, and `OS/2` are laid out the same
regardless of outline format.

Pass `--size` (or `-s`) with a point size to also see each metric scaled to
that size, the same `value * size / unitsPerEm` conversion a text layout
engine does to go from font units to output pixels:

```
$ fontmetrics --size 16 /System/Library/Fonts/Helvetica.ttc
```

```
units per em:     1000
hhea ascender:    952  (15.23 at size 16)
hhea descender:   -213  (-3.41 at size 16)
hhea line gap:    0  (0.00 at size 16)
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

Pass `--json` to get the same numbers as a single JSON object instead, for
piping into another tool:

```
$ fontmetrics --json /System/Library/Fonts/Helvetica.ttc
```

```json
{
  "outlineFormat": "TrueType (glyf outlines)",
  "unitsPerEm": 1000,
  "hheaAscender": 952,
  "hheaDescender": -213,
  "hheaLineGap": 0,
  "os2": { "typoAscender": 952, "typoDescender": -213, "typoLineGap": 0, "winAscent": 952, "winDescent": 213, "legacyFallback": false, "capHeight": 714, "xHeight": 523 }
}
```

If the font has no `OS/2` table, `"os2"` is `null`. Combined with `--size`,
each metric becomes an object with `value` (font units) and `scaled` (at the
given size) instead of a bare number.

Pass `--glyph` (or `-g`) with a glyph id to also look up that glyph's advance
width from `hmtx`:

```
$ fontmetrics --glyph 40 /System/Library/Fonts/Helvetica.ttc
```

```
...
glyph 40 advance: 556
```

This takes a raw glyph id. The id must be less than the font's glyph count
(from `maxp`); anything else is an error.

Pass `--char` (or `-c`) with a single character instead, to look up its
glyph id through the font's `cmap` table first:

```
$ fontmetrics --char A /System/Library/Fonts/Helvetica.ttc
```

```
...
glyph 36 ('A' U+0041) advance: 667
```

`--glyph` and `--char` can't be combined. If the font's `cmap` has no entry
for the character, that's an error rather than a silent fallback. Lookups
use whichever `cmap` subtable best covers Unicode - full-repertoire formats
(needed for anything outside the Basic Multilingual Plane, like emoji) are
preferred over BMP-only ones.

Pass `--tables` to list every table tag in the font's sfnt directory instead
of the metrics report:

```
$ fontmetrics --tables /System/Library/Fonts/Helvetica.ttc
```

```
tables (12):
  'cmap'  2556 bytes
  'cvt '  144 bytes
  'fpgm'  1146 bytes
  ...
```

This only reads the table directory, so it works even on a font missing
`head` or `hhea` - useful for checking what's actually in a file before
reaching for the rest of this tool.

Pass `-` as the font file to read the font bytes from stdin instead, so the
tool can sit at the end of a pipeline:

```
$ cat /System/Library/Fonts/Helvetica.ttc | fontmetrics -
```

A `.woff` file works the same as a plain `.ttf`/`.otf`, whether passed as a
path or piped in over stdin - the tool notices the `wOFF` signature and
decompresses the table directory itself before reading it. `.woff2` isn't
supported: it compresses tables with Brotli instead of zlib, which is a much
bigger format to implement from scratch, so for now it needs to be converted
to `.ttf`/`.otf` (or `.woff`) first with something like `woff2_decompress`.

## How it works

An sfnt font file (the container format behind both `.ttf` and `.otf`)
starts with a table directory: a list of four-byte tags, each pointing at an
offset and length elsewhere in the file. `src/sfnt.rs` parses that directory
and slices out individual tables. `src/tables.rs` knows the byte layout of
the specific tables this tool cares about and turns them into plain structs.
`src/cmap.rs` handles the one table with a more involved internal structure:
character-to-glyph mapping, used by `--char`. `src/woff.rs` reconstructs a
plain sfnt file from a WOFF container, using `src/inflate.rs`'s DEFLATE
decoder to undo the zlib compression on each table.

Every parsing function takes a byte slice and returns a value or an error —
no file I/O, no globals, no hidden state. `src/main.rs` is the only place
that touches the filesystem; everything else is tested with byte arrays
built by hand in `#[cfg(test)]` modules.

## Building

Standard library only, no external crates:

```
cargo build --release
```

Some very old fonts have an `OS/2` table in the original TrueType 1.0 layout,
which stops 10 bytes short of `sTypoAscender` and never got
`usWinAscent`/`usWinDescent` at all. When that happens, the tool falls back
to the `hhea` numbers for those fields and says so:

```
typo ascender:    800
typo descender:   -200
typo line gap:    0
win ascent:       800
win descent:      200
  (typo/win fields above are from hhea: this font's OS/2 table is the old version 0 layout without them)
```

## Limitations (for now)

- Per-glyph advance widths are readable with `--glyph`/`--char`, but
  individual glyph outlines (from `glyf` or `CFF `/`CFF2`) are out of scope;
  `outline format` reports which outline table is present without parsing
  its contents.
- `cmap` lookups cover formats 0, 4, and 12, which is every font this tool
  has been tried against. Formats 2, 6, 13, and 14 (used for some CJK
  encodings and Unicode variation selectors) aren't implemented.
- `.woff` input is decompressed automatically; `.woff2` isn't, since it
  uses Brotli rather than zlib for table compression.

## License

MIT, see `LICENSE`.
