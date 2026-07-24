//! # OG cards dinâmicas (item 7 do plano, fatia 2 — 0.33.0).
//!
//! `GET /og/placar/{mandate_id}.png` — card 1200×630 com o placar público do
//! mandato (respondidas × silêncios × taxa), pronto pra `og:image`/Twitter
//! card. A fatia 1 (0.30.2) entregou o widget iframe; esta fecha o item: o
//! placar circula como IMAGEM quando alguém cola o link do político no
//! WhatsApp, X, Telegram ou Mastodon — consequência só existe se circula.
//!
//! Rasterização 100% in-process (`image` + `ab_glyph`, DejaVu Sans embarcada
//! no binário — licença Bitstream Vera, redistribuição livre): zero stack
//! externa de screenshot/navegador. Cache 5 min, mesmo TTL do embed.

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_app::AppState;
use image::{Rgba, RgbaImage};
use std::io::Cursor;
use uuid::Uuid;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};

static FONT_REGULAR: &[u8] = include_bytes!("../assets/fonts/DejaVuSans.ttf");
static FONT_BOLD: &[u8] = include_bytes!("../assets/fonts/DejaVuSans-Bold.ttf");

const W: u32 = 1200;
const H: u32 = 630;

// Paleta — mesma família do brand (verde #15803d) + neutros slate.
const BG: Rgba<u8> = Rgba([255, 255, 255, 255]);
const ACCENT: Rgba<u8> = Rgba([21, 128, 61, 255]); // #15803d
const INK: Rgba<u8> = Rgba([15, 23, 42, 255]); // #0f172a
const MUTED: Rgba<u8> = Rgba([100, 116, 139, 255]); // #64748b
const SUBTLE: Rgba<u8> = Rgba([71, 85, 105, 255]); // #475569
const GREEN: Rgba<u8> = Rgba([21, 128, 61, 255]);
const RED: Rgba<u8> = Rgba([185, 28, 28, 255]); // #b91c1c
const TRACK: Rgba<u8> = Rgba([226, 232, 240, 255]); // #e2e8f0

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        // Axum casa segmento inteiro — o `.png` é removido no handler.
        .route("/og/placar/{file}", get(placar_card))
        .with_state(state)
}

async fn placar_card(State(state): State<AppState>, Path(file): Path<String>) -> Response {
    // Aceita `{uuid}.png` (canônico pros crawlers) e `{uuid}` pelado.
    let Ok(mandate_id) = file.trim_end_matches(".png").parse::<Uuid>() else {
        return (StatusCode::NOT_FOUND, "não encontrado").into_response();
    };
    let row: Option<(String, String, Option<String>, Option<String>, i64, i64)> = sqlx::query_as(
        r"SELECT m.display_name,
                     m.office,
                     m.party,
                     m.uf,
                     COALESCE(s.answered, 0),
                     COALESCE(s.ignored, 0)
                FROM mandate m
                LEFT JOIN scorecard s ON s.mandate_id = m.id
               WHERE m.id = $1",
    )
    .bind(mandate_id)
    .fetch_optional(&state.db)
    .await
    .unwrap_or(None);
    let Some((name, office, party, uf, answered, ignored)) = row else {
        return (StatusCode::NOT_FOUND, "mandato não encontrado").into_response();
    };

    let png = render_placar(
        &name,
        &office,
        party.as_deref(),
        uf.as_deref(),
        answered,
        ignored,
    );
    match png {
        Some(bytes) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/png".to_owned()),
                // 5 min — placar muda devagar; crawler de OG re-busca por link.
                (header::CACHE_CONTROL, "public, max-age=300".to_owned()),
            ],
            bytes,
        )
            .into_response(),
        None => (StatusCode::INTERNAL_SERVER_ERROR, "erro ao gerar card").into_response(),
    }
}

/// Compõe o card. `None` só se o encode PNG falhar (nunca em condições
/// normais — a fonte é estática e validada nos testes).
fn render_placar(
    name: &str,
    office: &str,
    party: Option<&str>,
    uf: Option<&str>,
    answered: i64,
    ignored: i64,
) -> Option<Vec<u8>> {
    let regular = FontRef::try_from_slice(FONT_REGULAR).ok()?;
    let bold = FontRef::try_from_slice(FONT_BOLD).ok()?;

    let mut img = RgbaImage::from_pixel(W, H, BG);

    // Barra de accent à esquerda — identidade visual mínima.
    fill_rect(&mut img, 0, 0, 16, H, ACCENT);

    let x0 = 80i32;

    // Cabeçalho institucional.
    draw_text(
        &mut img,
        &bold,
        28.0,
        x0,
        88,
        ACCENT,
        "DEMOCRACIA.SOCIAL.BR — PLACAR PÚBLICO DE RESPOSTA",
    );

    // Nome (auto-encolhe até caber; depois trunca com …).
    let max_w = 1040.0f32;
    let mut name_px = 68.0f32;
    while name_px > 40.0 && text_width(&bold, name_px, name) > max_w {
        name_px -= 4.0;
    }
    let shown_name = truncate_to_width(&bold, name_px, name, max_w);
    draw_text(&mut img, &bold, name_px, x0, 190, INK, &shown_name);

    // Cargo + partido/UF.
    let mut sub = office.to_owned();
    match (party, uf) {
        (Some(p), Some(u)) if !p.is_empty() && !u.is_empty() => {
            sub = format!("{office} · {p}/{u}");
        }
        (Some(p), _) if !p.is_empty() => sub = format!("{office} · {p}"),
        (_, Some(u)) if !u.is_empty() => sub = format!("{office} · {u}"),
        _ => {}
    }
    let sub = truncate_to_width(&regular, 32.0, &sub, max_w);
    draw_text(&mut img, &regular, 32.0, x0, 245, SUBTLE, &sub);

    // Três blocos de estatística.
    let total = answered + ignored;
    #[allow(clippy::cast_precision_loss)]
    let rate = if total > 0 {
        format!("{:.0}%", (answered as f64 / total as f64) * 100.0)
    } else {
        "—".to_owned()
    };
    let stats: [(&str, String, Rgba<u8>); 3] = [
        ("RESPONDIDAS", answered.to_string(), GREEN),
        ("SILÊNCIOS", ignored.to_string(), RED),
        ("TAXA DE RESPOSTA", rate, INK),
    ];
    for (i, (label, value, color)) in stats.iter().enumerate() {
        let x = x0 + i32::try_from(i).unwrap_or(0) * 373;
        draw_text(&mut img, &bold, 92.0, x, 420, *color, value);
        draw_text(&mut img, &regular, 26.0, x, 465, MUTED, label);
    }

    // Barra de proporção respondidas × silêncios.
    let bar_y = 510u32;
    let bar_w = W - 160;
    fill_rect(&mut img, 80, bar_y, bar_w, 22, TRACK);
    if total > 0 {
        #[allow(
            clippy::cast_precision_loss,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss
        )]
        let green_w = ((answered as f64 / total as f64) * f64::from(bar_w)).round() as u32;
        fill_rect(&mut img, 80, bar_y, green_w, 22, GREEN);
        if green_w < bar_w {
            fill_rect(&mut img, 80 + green_w, bar_y, bar_w - green_w, 22, RED);
        }
    }

    // Rodapé-fonte.
    draw_text(
        &mut img,
        &regular,
        26.0,
        x0,
        592,
        MUTED,
        "Fonte verificável: democracia.social.br — o silêncio também é registro público",
    );

    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

fn fill_rect(img: &mut RgbaImage, x: u32, y: u32, w: u32, h: u32, color: Rgba<u8>) {
    for yy in y..(y + h).min(H) {
        for xx in x..(x + w).min(W) {
            img.put_pixel(xx, yy, color);
        }
    }
}

/// Desenha `text` com baseline em `(x, baseline_y)`; retorna a largura usada.
/// Alpha-blend da cobertura do glyph sobre o pixel existente.
#[allow(clippy::cast_precision_loss)]
fn draw_text(
    img: &mut RgbaImage,
    font: &FontRef<'_>,
    px: f32,
    x: i32,
    baseline_y: i32,
    color: Rgba<u8>,
    text: &str,
) -> f32 {
    let scale = PxScale::from(px);
    let scaled = font.as_scaled(scale);
    let mut cursor = x as f32;
    let mut prev = None;
    for ch in text.chars() {
        let gid = scaled.glyph_id(ch);
        if let Some(p) = prev {
            cursor += scaled.kern(p, gid);
        }
        let glyph = gid.with_scale_and_position(scale, ab_glyph::point(cursor, baseline_y as f32));
        if let Some(outlined) = font.outline_glyph(glyph) {
            let bounds = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                #[allow(clippy::cast_possible_truncation)]
                let px_x = bounds.min.x as i32 + i32::try_from(gx).unwrap_or(0);
                #[allow(clippy::cast_possible_truncation)]
                let px_y = bounds.min.y as i32 + i32::try_from(gy).unwrap_or(0);
                if px_x < 0 || px_y < 0 || cov <= 0.0 {
                    return;
                }
                let (ux, uy) = (px_x.unsigned_abs(), px_y.unsigned_abs());
                if ux >= W || uy >= H {
                    return;
                }
                let dst = *img.get_pixel(ux, uy);
                let a = cov.clamp(0.0, 1.0);
                let blend = |c: u8, d: u8| -> u8 {
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let v = (f32::from(c) * a + f32::from(d) * (1.0 - a)).round() as i32;
                    u8::try_from(v.clamp(0, 255)).unwrap_or(255)
                };
                img.put_pixel(
                    ux,
                    uy,
                    Rgba([
                        blend(color[0], dst[0]),
                        blend(color[1], dst[1]),
                        blend(color[2], dst[2]),
                        255,
                    ]),
                );
            });
        }
        cursor += scaled.h_advance(gid);
        prev = Some(gid);
    }
    cursor - x as f32
}

fn text_width(font: &FontRef<'_>, px: f32, text: &str) -> f32 {
    let scaled = font.as_scaled(PxScale::from(px));
    let mut w = 0.0;
    let mut prev = None;
    for ch in text.chars() {
        let gid = scaled.glyph_id(ch);
        if let Some(p) = prev {
            w += scaled.kern(p, gid);
        }
        w += scaled.h_advance(gid);
        prev = Some(gid);
    }
    w
}

/// Trunca com reticências até caber em `max_w` px.
fn truncate_to_width(font: &FontRef<'_>, px: f32, text: &str, max_w: f32) -> String {
    if text_width(font, px, text) <= max_w {
        return text.to_owned();
    }
    let mut s: Vec<char> = text.chars().collect();
    while !s.is_empty() {
        s.pop();
        let candidate: String = s.iter().collect::<String>() + "…";
        if text_width(font, px, &candidate) <= max_w {
            return candidate;
        }
    }
    "…".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_parse_and_card_renders_png() {
        let png = render_placar(
            "Maria Exemplo da Silva",
            "Deputada Federal",
            Some("XYZ"),
            Some("SP"),
            7,
            3,
        )
        .expect("card renders");
        // Assinatura PNG.
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        // Sanidade: card não-trivial (fundo + texto + barras).
        assert!(
            png.len() > 5_000,
            "png suspeito de vazio: {} bytes",
            png.len()
        );
    }

    #[test]
    fn long_names_truncate_and_zero_total_renders() {
        let png = render_placar(
            "Nome Absurdamente Comprido Que Não Cabe De Jeito Nenhum Na Largura Do Card De Mil E Duzentos Pixels",
            "Vereador",
            None,
            None,
            0,
            0,
        );
        assert!(png.is_some());
    }
}
