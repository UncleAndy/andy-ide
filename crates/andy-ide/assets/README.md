# Fonts

`DejaVuSans.ttf` — DejaVu Sans 2.37, the "minimal" build.

It is not the window's text font. It sits **behind** the fonts egui brings (see
`ui::fonts_with_fallback`), so it is only ever reached for a character nothing before it has —
`↳`, `↑`, `↓` and their like, which the built-in fonts do not carry and which came out as empty
boxes. Nothing of the window's ordinary look comes from this file.

Licence: the DejaVu fonts are based on Bitstream Vera and are free to use and redistribute
(see <https://dejavu-fonts.github.io/License.html>). The copy here is the one nixpkgs builds for
`dejavu_fonts.minimal`.

`OpenMoji-black.ttf` — OpenMoji Black 17.0.0 (<https://openmoji.org/>), the monochrome build with
real outlines, cut down to the emoji blocks.

It sits behind the other fonts for the same reason DejaVu does, and it was added because the
NotoEmoji egui brings predates the emoji the agent writes: `🧠` (U+1F9E0, the heading of its context
report) and the circles `🟠🟢🟡` (U+1F7E0..U+1F7E2) of its status lines came out as empty boxes in
the panel, and neither DejaVu nor any other built-in font carries an emoji at all.

The subset was cut once, with fontTools, from the nixpkgs build of `openmoji-black` 17.0.0 — **not**
at build time, so no build of this project needs fontTools, python or a network:

```
pyftsubset OpenMoji-black-glyf.ttf \
  --unicodes=U+2600-27BF,U+2B00-2BFF,U+1F000-1FAFF,U+1F1E6-1F1FF,U+FE0F,U+2139,U+2194-21AA,U+231A-231B,U+2328,U+23CF,U+23E9-23FA,U+24C2,U+25AA-25FE \
  --no-hinting --desubroutinize --drop-tables+=GSUB,GPOS,GDEF --layout-features= \
  --name-IDs=* --notdef-outline --output-file OpenMoji-black.ttf
```

Every emoji block is kept whole (1472 characters, 838 KB of the 1.4 MB original), so an emoji the
agent starts using tomorrow is drawn without touching this file.

What it buys, measured by asking the window for its map of characters (`Font::characters`, the same
question the panel test asks): of the 1472, **523 are drawn by no other font of the window** and were
empty boxes before this file was added — 505 of them at U+1F000 and above (the faces, the pictograms,
the transport, the tools of U+1F9xx), the rest signs such as `🟠 🟡 🟢 🔴 🟣 🟤 🟥 🟦 🟧 🟨 🟩 🟪 🟫`, the
circles the agent marks its status lines with. In both families alike.

Still boxes, and knowingly so: 32 assigned characters the original has and this subset does not — the
power symbols `⏻ ⏼ ⏽ ⏾` (U+23FB..U+23FE), the stick figures U+1FBC5..U+1FBC9 and 25 invisible tag
characters — plus its 353 private-use entries, which are not characters in the first place. CJK,
Korean syllables and full-width punctuation are outside OpenMoji altogether and stay boxes whatever
this file holds.

Licence: **CC BY-SA 4.0** (<https://creativecommons.org/licenses/by-sa/4.0/>) — not the permissive
licence of DejaVu. The name of the set and this line are the attribution, and anything shipped with
the font inside carries it as well (a credits line or an About box beside the binary).
