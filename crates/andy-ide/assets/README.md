# Fonts

`DejaVuSans.ttf` — DejaVu Sans 2.37, the "minimal" build.

It is not the window's text font. It sits **behind** the fonts egui brings (see
`ui::fonts_with_fallback`), so it is only ever reached for a character nothing before it has —
`↳`, `↑`, `↓` and their like, which the built-in fonts do not carry and which came out as empty
boxes. Nothing of the window's ordinary look comes from this file.

Licence: the DejaVu fonts are based on Bitstream Vera and are free to use and redistribute
(see <https://dejavu-fonts.github.io/License.html>). The copy here is the one nixpkgs builds for
`dejavu_fonts.minimal`.
