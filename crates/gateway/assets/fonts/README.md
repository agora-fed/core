# Fontes embarcadas no binário (OG cards)

`DejaVuSans.ttf` e `DejaVuSans-Bold.ttf` — projeto DejaVu Fonts
(https://dejavu-fonts.github.io/), derivadas da Bitstream Vera.

Licença: Bitstream Vera Fonts Copyright + domínio público (glyphs
adicionais do DejaVu). Redistribuição permitida, inclusive embutida em
binários, sem royalties. Texto completo:
https://dejavu-fonts.github.io/License.html

Usadas por `crates/gateway/src/og_cards.rs` via `include_bytes!` pra
rasterizar o card PNG 1200×630 do placar (`/og/placar/{id}.png`).
