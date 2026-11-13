//! # Public petitions ("abaixo-assinados"): a member writes a text that needs support,
//! and anyone can sign it, with or without an account.
//!
//! The data lives in the `dsoc-initiatives` tables (`initiative`, `initiative_signature`)
//! plus migration 0693. The module switch is `module.initiatives`.
//!
//! * Members (signed in, e-mail level) create petitions and sign with one click.
//! * Visitors sign with name and e-mail. They pass a self-hosted proof-of-work check
//!   ([`altcha`]) and confirm the address. The confirmation creates their account here,
//!   and, when the operator turns it on, at the identity provider too ([`provider`]).
//! * The operator chooses who sends the confirmation e-mail ([`config`]): this server
//!   (`local`) or the identity provider (`sso`). Guide: `docs/petitions.md`.
//! * The author or an admin closes a petition; an admin hides one (moderation).
//!
//! Routes (under `/api/v1`):
//!
//! | Method | Path | Who |
//! |---|---|---|
//! | GET | `/petitions?status=open\|closed\|hidden` | anyone (hidden: admins) |
//! | POST | `/petitions` | member |
//! | GET | `/petitions/settings` | anyone |
//! | GET | `/petitions/altcha` | anyone |
//! | POST | `/petitions/confirm` | anyone with a link |
//! | GET, PATCH | `/petitions/{id}` | anyone / author or admin |
//! | POST | `/petitions/{id}/sign` | member |
//! | POST | `/petitions/{id}/public-signatures` | anyone |

pub(crate) mod altcha;
pub(crate) mod config;
pub(crate) mod domain;
pub(crate) mod flow;
pub(crate) mod provider;
pub(crate) mod store;

use std::sync::OnceLock;

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dsoc_api_contract::ApiResponse;
use dsoc_app::{AppState, CallerId};
use dsoc_core::VerificationLevel;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::admin_branding::DEFAULT_ORG_UUID;
use config::{PetitionCfg, Verification};
use flow::{FlowError, Proof};

const MODULE: &str = "initiatives";
const LIST_LIMIT: i64 = 100;
const SIGNERS_SHOWN: i64 = 30;
const SUMMARY_CHARS: usize = 280;
/// Per minute. Public signing sends e-mail, so it is strict.
const PUBLIC_PER_IP: u32 = 5;
const PUBLIC_PER_EMAIL: u32 = 2;
const CHALLENGES_PER_IP: u32 = 20;
const CONFIRMS_PER_IP: u32 = 10;

pub(crate) fn routes(state: AppState) -> Router<()> {
    Router::new()
        .route("/petitions", get(list).post(create))
        .route("/petitions/settings", get(settings))
        .route("/petitions/altcha", get(challenge))
        .route("/petitions/confirm", post(confirm))
        .route("/petitions/{id}", get(detail).patch(update))
        .route("/petitions/{id}/sign", post(member_sign))
        .route("/petitions/{id}/public-signatures", post(public_sign))
        .route_layer(middleware::from_fn_with_state(state.clone(), module_gate))
        .with_state(state)
}

/// The configuration, read once. A bad configuration turns anonymous signing off and is
/// logged once, so a typo never opens an unguarded path.
fn cfg() -> Option<&'static PetitionCfg> {
    static CFG: OnceLock<Option<PetitionCfg>> = OnceLock::new();
    CFG.get_or_init(|| match config::from_env() {
        Ok(c) => Some(c),
        Err(e) => {
            tracing::error!(error = %e, "petitions: bad configuration; anonymous signing is off");
            None
        }
    })
    .as_ref()
}

/// Anonymous signing needs a valid configuration AND the ALTCHA key.
fn anonymous_cfg() -> Option<&'static PetitionCfg> {
    cfg().filter(|c| c.altcha_key.is_some())
}

fn org_of(headers: &HeaderMap) -> Uuid {
    headers
        .get("x-dsoc-org-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_ORG_UUID)
}

/// The signed-in citizen, when there is one (the gateway sets this header from the session
/// and strips it from client requests).
fn viewer(headers: &HeaderMap) -> Option<Uuid> {
    headers
        .get("x-dsoc-citizen-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
}

fn client_ip(headers: &HeaderMap) -> String {
    headers
        .get(crate::CLIENT_IP_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_owned()
}

async fn module_gate(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match crate::module_gate::require_module(&state, org_of(req.headers()), MODULE).await {
        Ok(()) => next.run(req).await,
        Err(off) => off,
    }
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response {
    (status, Json(ApiResponse::<()>::fail(code, msg))).into_response()
}
fn ok<T: Serialize>(status: StatusCode, data: T) -> Response {
    (status, Json(ApiResponse::ok(data))).into_response()
}
fn not_found() -> Response {
    fail(
        StatusCode::NOT_FOUND,
        "not_found",
        "Abaixo-assinado não encontrado.",
    )
}
fn invalid(msg: &str) -> Response {
    fail(StatusCode::UNPROCESSABLE_ENTITY, "invalid_input", msg)
}
fn too_many() -> Response {
    fail(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Muitas tentativas em pouco tempo. Aguarde um minuto e tente de novo.",
    )
}
fn storage(err: &sqlx::Error) -> Response {
    tracing::warn!(?err, "petitions: storage error");
    fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "storage_error",
        "Erro interno.",
    )
}

fn flow_error(e: FlowError) -> Response {
    match e {
        FlowError::NotFound => not_found(),
        FlowError::Closed => fail(
            StatusCode::CONFLICT,
            "closed",
            "Este abaixo-assinado não recebe mais assinaturas.",
        ),
        FlowError::Blocked => fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Esta conta não pode assinar.",
        ),
        FlowError::AccountExists => fail(
            StatusCode::CONFLICT,
            "account_exists",
            "Este e-mail já tem uma conta. Entre na sua conta para assinar com um clique.",
        ),
        FlowError::InvalidLink => fail(
            StatusCode::GONE,
            "invalid_link",
            "Este link de confirmação não vale mais. Ele pode ter expirado ou já ter sido usado.",
        ),
        FlowError::Cooldown(secs) => fail(
            StatusCode::TOO_MANY_REQUESTS,
            "cooldown",
            &format!(
                "Já enviamos um e-mail para este endereço. Confira a caixa de entrada e o spam, \
                 ou peça de novo em {} minuto(s).",
                (secs + 59) / 60
            ),
        ),
        FlowError::Busy => fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "busy",
            "Recebemos muitas assinaturas agora. Tente de novo daqui a pouco.",
        ),
        FlowError::MailUnavailable => {
            tracing::error!("petitions: no working e-mail path for confirmations");
            fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "mail_unavailable",
                "Não conseguimos enviar o e-mail de confirmação agora. Tente mais tarde.",
            )
        }
        FlowError::Provider(e) => {
            tracing::error!(error = %e, "petitions: identity provider call failed");
            fail(
                StatusCode::BAD_GATEWAY,
                "provider_unavailable",
                "O serviço de contas não respondeu. Tente de novo em alguns minutos.",
            )
        }
        FlowError::Db(e) => storage(&e),
    }
}

#[derive(Serialize)]
struct ListItem {
    id: Uuid,
    title: String,
    summary: String,
    threshold: i32,
    signature_count: i64,
    reached: bool,
    status: String,
    author_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Deserialize)]
struct ListQuery {
    status: Option<String>,
}

async fn is_admin(state: &AppState, org: Uuid, who: Option<Uuid>) -> bool {
    match who {
        Some(c) => store::is_org_admin(&state.db, org, c)
            .await
            .unwrap_or(false),
        None => false,
    }
}

/// `GET /petitions`
async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Response {
    let org = org_of(&headers);
    let status = match q.status.as_deref() {
        None | Some("open") => Some("open"),
        Some("closed") => Some("closed"),
        Some("hidden") => Some("hidden"),
        Some("all") => None,
        Some(_) => return invalid("Filtro de situação inválido."),
    };
    let admin = is_admin(&state, org, viewer(&headers)).await;
    if status == Some("hidden") && !admin {
        return fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Só a administração vê os ocultos.",
        );
    }
    match store::list(&state.db, org, status, admin, LIST_LIMIT).await {
        Ok(rows) => ok(
            StatusCode::OK,
            rows.into_iter()
                .map(|r| ListItem {
                    summary: domain::summary(&r.body, SUMMARY_CHARS),
                    reached: r.reached_at.is_some(),
                    id: r.id,
                    title: r.title,
                    threshold: r.threshold,
                    signature_count: r.signature_count,
                    status: r.status,
                    author_name: r.author_name,
                    created_at: r.created_at,
                })
                .collect::<Vec<_>>(),
        ),
        Err(e) => storage(&e),
    }
}

#[derive(Deserialize)]
struct CreateBody {
    title: String,
    body: String,
    threshold: i32,
}

/// `POST /petitions` — a member at e-mail level publishes right away.
async fn create(
    State(state): State<AppState>,
    caller: CallerId,
    Json(b): Json<CreateBody>,
) -> Response {
    if state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
        .is_err()
    {
        return fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Confirme seu e-mail para criar um abaixo-assinado.",
        );
    }
    let draft = match domain::check_draft(&b.title, &b.body, b.threshold) {
        Ok(d) => d,
        Err(msg) => return invalid(msg),
    };
    let org = caller.org.as_uuid();
    match store::create(
        &state.db,
        org,
        caller.citizen.as_uuid(),
        &draft.title,
        &draft.body,
        draft.threshold,
    )
    .await
    {
        Ok(id) => ok(StatusCode::CREATED, json!({ "id": id })),
        Err(e) => storage(&e),
    }
}

/// `GET /petitions/settings` — what the sign form must offer.
async fn settings() -> Response {
    let anonymous = anonymous_cfg();
    ok(
        StatusCode::OK,
        json!({
            "anonymous_signing": anonymous.is_some(),
            "verification": anonymous.map(|c| match c.verification {
                Verification::Local => "local",
                Verification::Sso => "sso",
            }),
            "creates_sso_account": anonymous.is_some_and(|c| c.provider.is_some()),
            "pending_ttl_hours": anonymous.map(|c| c.pending_ttl_hours),
        }),
    )
}

/// `GET /petitions/altcha` — a fresh proof-of-work challenge.
async fn challenge(headers: HeaderMap) -> Response {
    if crate::rate_limit::over_limit(
        &format!("altcha:{}", client_ip(&headers)),
        CHALLENGES_PER_IP,
    ) {
        return too_many();
    }
    let Some(key) = anonymous_cfg().and_then(|c| c.altcha_key.as_deref()) else {
        return not_found();
    };
    match altcha::new_challenge(key, chrono::Utc::now().timestamp()) {
        Some(c) => (StatusCode::OK, [("cache-control", "no-store")], Json(c)).into_response(),
        None => fail(StatusCode::INTERNAL_SERVER_ERROR, "altcha", "Erro interno."),
    }
}

#[derive(Serialize)]
struct Detail {
    id: Uuid,
    title: String,
    body: String,
    threshold: i32,
    signature_count: i64,
    reached: bool,
    status: String,
    author_name: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
    closed_at: Option<chrono::DateTime<chrono::Utc>>,
    signers: Vec<store::PublicSigner>,
    signed_by_me: bool,
    can_close: bool,
    is_admin: bool,
}

/// `GET /petitions/{id}`
async fn detail(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    let org = org_of(&headers);
    let me = viewer(&headers);
    let admin = is_admin(&state, org, me).await;
    let row = match store::get(&state.db, org, id, admin).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => return storage(&e),
    };
    let signers = match store::recent_signers(&state.db, id, SIGNERS_SHOWN).await {
        Ok(s) => s,
        Err(e) => return storage(&e),
    };
    let signed_by_me = match me {
        Some(c) => store::has_signed(&state.db, id, c).await.unwrap_or(false),
        None => false,
    };
    ok(
        StatusCode::OK,
        Detail {
            can_close: admin || (me.is_some() && me == row.author_id),
            is_admin: admin,
            reached: row.reached_at.is_some(),
            id: row.id,
            title: row.title,
            body: row.body,
            threshold: row.threshold,
            signature_count: row.signature_count,
            status: row.status,
            author_name: row.author_name,
            created_at: row.created_at,
            closed_at: row.closed_at,
            signers,
            signed_by_me,
        },
    )
}

#[derive(Deserialize)]
struct UpdateBody {
    status: String,
}

/// `PATCH /petitions/{id}` — the author closes or reopens; an admin also hides.
async fn update(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    Json(b): Json<UpdateBody>,
) -> Response {
    let org = caller.org.as_uuid();
    let me = caller.citizen.as_uuid();
    if !matches!(b.status.as_str(), "open" | "closed" | "hidden") {
        return invalid("Situação inválida.");
    }
    let admin = is_admin(&state, org, Some(me)).await;
    let row = match store::get(&state.db, org, id, true).await {
        Ok(Some(r)) => r,
        Ok(None) => return not_found(),
        Err(e) => return storage(&e),
    };
    let author = row.author_id == Some(me);
    // Only an admin hides, or touches a status an admin set (moderation).
    let allowed = admin || (author && b.status != "hidden" && !row.moderated);
    if !allowed {
        return fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Só quem criou o abaixo-assinado ou a administração pode fazer isso.",
        );
    }
    match store::set_status(&state.db, org, id, &b.status, admin && !author).await {
        Ok(true) => ok(StatusCode::OK, json!({ "ok": true })),
        Ok(false) => not_found(),
        Err(e) => storage(&e),
    }
}

#[derive(Deserialize, Default)]
struct MemberSignBody {
    #[serde(default)]
    show_name: bool,
    #[serde(default)]
    city: Option<String>,
}

/// `POST /petitions/{id}/sign` — a signed-in member signs with one click.
async fn member_sign(
    State(state): State<AppState>,
    caller: CallerId,
    Path(id): Path<Uuid>,
    body: Option<Json<MemberSignBody>>,
) -> Response {
    if state
        .authz
        .require(caller.org, caller.citizen, VerificationLevel::Email)
        .await
        .is_err()
    {
        return fail(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Confirme seu e-mail para assinar.",
        );
    }
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let city = b.city.as_deref().map(str::trim).filter(|c| !c.is_empty());
    if city.is_some_and(|c| c.chars().count() > domain::CITY_MAX) {
        return invalid("A cidade pode ter até 80 caracteres.");
    }
    let org = caller.org.as_uuid();
    let me = caller.citizen.as_uuid();
    let public_name = if b.show_name {
        let name: Option<String> =
            sqlx::query_scalar("SELECT COALESCE(display_name, handle) FROM citizen WHERE id = $1")
                .bind(me)
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten();
        name
    } else {
        None
    };
    let mut tx = match state.db.begin().await {
        Ok(tx) => tx,
        Err(e) => return storage(&e),
    };
    let tally = match store::sign(&mut tx, org, id, me, public_name.as_deref(), city).await {
        Ok(t) => t,
        Err(e) => return flow_error(e.into()),
    };
    if let Err(e) = tx.commit().await {
        return storage(&e);
    }
    let status = if tally.newly_signed {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    ok(status, tally)
}

#[derive(Deserialize)]
struct PublicSignBody {
    name: String,
    email: String,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    show_name: bool,
    /// The visitor agreed to the terms shown on the form (data use and account creation).
    consent: bool,
    /// base64 ALTCHA payload.
    altcha: String,
}

/// `POST /petitions/{id}/public-signatures` — a visitor asks to sign.
async fn public_sign(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(b): Json<PublicSignBody>,
) -> Response {
    let Some(cfg) = anonymous_cfg() else {
        return fail(
            StatusCode::FORBIDDEN,
            "anonymous_off",
            "Entre na sua conta para assinar.",
        );
    };
    let ip = client_ip(&headers);
    if crate::rate_limit::over_limit(&format!("petition-sign-ip:{ip}"), PUBLIC_PER_IP) {
        return too_many();
    }
    if !b.consent {
        return invalid("Para assinar, é preciso aceitar os termos.");
    }
    let signer = match domain::check_signer(&b.name, &b.email, b.city.as_deref()) {
        Ok(s) => s,
        Err(msg) => return invalid(msg),
    };
    if crate::rate_limit::over_limit(
        &format!("petition-sign-email:{}", signer.email),
        PUBLIC_PER_EMAIL,
    ) {
        return too_many();
    }
    if !crate::signup_gates::signup_allowed(&state.db, &headers, &signer.email).await {
        return fail(
            StatusCode::FORBIDDEN,
            "gate_denied",
            "Não é possível assinar a partir desta origem ou com este e-mail. \
             Fale conosco pelo formulário de contato se acredita ser um engano.",
        );
    }
    let now = chrono::Utc::now().timestamp();
    let key = cfg.altcha_key.as_deref().unwrap_or_default();
    let Some(solution) = altcha::decode(&b.altcha) else {
        return invalid("A verificação anti-robô falhou. Recarregue a página e tente de novo.");
    };
    let Some(expires) = altcha::verify(key, &solution, now) else {
        return invalid("A verificação anti-robô falhou. Recarregue a página e tente de novo.");
    };
    match store::spend_altcha(&state.db, &solution.challenge, expires).await {
        Ok(true) => {}
        Ok(false) => return invalid("Esta verificação já foi usada. Recarregue a página."),
        Err(e) => return storage(&e),
    }
    let req = flow::Request {
        org: org_of(&headers),
        petition: id,
        signer: &signer,
        show_name: b.show_name,
    };
    match flow::request(&state.db, cfg, &req).await {
        Ok(()) => ok(StatusCode::ACCEPTED, json!({ "status": "check_email" })),
        Err(e) => flow_error(e),
    }
}

#[derive(Deserialize)]
struct ConfirmBody {
    /// local mode
    #[serde(default)]
    token: Option<String>,
    /// sso mode
    #[serde(default)]
    pending: Option<Uuid>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    code: Option<String>,
}

fn short(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|v| !v.is_empty() && v.len() <= 128)
}

/// `POST /petitions/confirm` — the visitor followed the e-mail link.
async fn confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(b): Json<ConfirmBody>,
) -> Response {
    if crate::rate_limit::over_limit(
        &format!("petition-confirm:{}", client_ip(&headers)),
        CONFIRMS_PER_IP,
    ) {
        return too_many();
    }
    let Some(cfg) = cfg() else {
        return flow_error(FlowError::InvalidLink);
    };
    let proof = match (
        cfg.verification,
        short(&b.token),
        b.pending,
        short(&b.user),
        short(&b.code),
    ) {
        (Verification::Local, Some(token), _, _, _) => Proof::Token(token),
        (Verification::Sso, _, Some(pending), Some(user), Some(code)) => Proof::Code {
            pending,
            user,
            code,
        },
        _ => return flow_error(FlowError::InvalidLink),
    };
    match flow::confirm(&state.db, cfg, org_of(&headers), &proof).await {
        Ok(done) => ok(StatusCode::OK, done),
        Err(e) => flow_error(e),
    }
}
