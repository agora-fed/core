//! # Placar embedável (item 7 do plano, 0.30.2).
//!
//! O placar só gera consequência se circula FORA da plataforma: portais de
//! notícia e blogs embedam `GET /embed/placar/{mandate_id}` num iframe e o
//! placar do mandato (respondidas × silêncios) aparece com a marca e o link
//! da fonte. HTML autocontido (CSS inline, zero JS, ~2 KB), sem cabeçalhos
//! anti-frame — embedar é o PONTO. O JSON pra imprensa já existe na rota
//! pública `GET /api/v1/scorecards/{mandate_id}` (crate scorecard).

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use dsoc_app::AppState;
use uuid::Uuid;

pub fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/embed/placar/{mandate_id}", get(placar))
        .with_state(state)
}

/// Escape mínimo pra interpolar texto em HTML.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn placar(State(state): State<AppState>, Path(mandate_id): Path<Uuid>) -> Response {
    let row: Option<(String, String, i64, i64)> = sqlx::query_as(
        r"SELECT m.display_name,
                 m.office,
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
    let Some((name, office, answered, ignored)) = row else {
        return (StatusCode::NOT_FOUND, "mandato não encontrado").into_response();
    };
    let total = answered + ignored;
    #[allow(clippy::cast_precision_loss)]
    let rate = if total > 0 {
        format!("{:.0}%", (answered as f64 / total as f64) * 100.0)
    } else {
        "—".to_owned()
    };
    let origin = std::env::var("PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "https://democracia.social.br".to_owned());
    let origin = origin.trim_end_matches('/');
    let placar_url = format!("{origin}/politicos/{mandate_id}/placar");
    let (name, office) = (esc(&name), esc(&office));
    let html = format!(
        r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Placar — {name}</title>
<style>
body{{margin:0;font:14px/1.45 system-ui,sans-serif;background:#fff;color:#1a1a1a}}
.card{{border:1px solid #d8d8d8;border-radius:10px;padding:14px 16px;max-width:420px}}
.head{{font-weight:700;margin-bottom:2px}}.sub{{color:#666;font-size:12px;margin-bottom:10px}}
.nums{{display:flex;gap:18px}}.n{{display:flex;flex-direction:column}}
.n b{{font-size:22px}}.ok b{{color:#1a7f37}}.bad b{{color:#b42318}}
.n span{{font-size:11px;color:#666;text-transform:uppercase;letter-spacing:.04em}}
.src{{margin-top:10px;font-size:11px}}
.src a{{color:#1a7f37;text-decoration:none;font-weight:600}}
</style></head><body>
<div class="card">
  <div class="head">{name}</div>
  <div class="sub">{office} · prazos públicos de resposta</div>
  <div class="nums">
    <div class="n ok"><b>{answered}</b><span>respondidas</span></div>
    <div class="n bad"><b>{ignored}</b><span>silêncios registrados</span></div>
    <div class="n"><b>{rate}</b><span>taxa de resposta</span></div>
  </div>
  <div class="src">Fonte: <a href="{placar_url}" target="_blank" rel="noopener">DemocraciaBR — placar público verificável</a></div>
</div>
</body></html>"#
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            // Cache curto: o placar muda quando SLAs resolvem, não por request.
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        html,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::esc;

    #[test]
    fn escapes_html_metacharacters() {
        assert_eq!(
            esc(r#"<b>"A&B"</b>"#),
            "&lt;b&gt;&quot;A&amp;B&quot;&lt;/b&gt;"
        );
    }
}
