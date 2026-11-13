//! # Public signing flow: request a signature, then confirm the e-mail address.
//!
//! ```text
//! visitor ──form + ALTCHA──▶ request()
//!   local mode: store pending (token hash) ─▶ this server e-mails  /abaixo-assinado/confirmar/?t=TOKEN
//!   sso mode:   create/find provider user ─▶ provider e-mails      /abaixo-assinado/confirmar/?p=ID&u=USER&c=CODE
//! visitor ──clicks link──▶ confirm()
//!   check token (local) or code at provider (sso)
//!   [provisioning] make sure a provider user exists; new users get a "choose a password" link
//!   one transaction: find/create citizen ─▶ record signature ─▶ delete pending
//! ```
//!
//! A signature counts only after `confirm`. An address that already has a verified
//! provider account must sign in instead (the provider cannot send it a code without
//! un-verifying the account). That answer tells the visitor an account exists; the
//! proof-of-work and the rate limits in the handler make bulk probing expensive.

use sqlx::PgPool;
use uuid::Uuid;

use super::config::{PetitionCfg, ProviderCfg, Verification};
use super::domain::{self, Signer};
use super::provider::{Created, EmailMode, ProviderError, Zitadel};
use super::store::{self, PendingInput, SignError, Tally};
use crate::outbound::{guarded_get, OutboundError, OutboundPolicy};

const USERNAME_ATTEMPTS: usize = 5;
const CLEANUP_BATCH: i64 = 20;

/// Every way the flow can stop. The handler maps each one to a public message.
#[derive(Debug)]
pub(crate) enum FlowError {
    NotFound,
    Closed,
    Blocked,
    AccountExists,
    InvalidLink,
    MailUnavailable,
    /// The same address asked for this petition too recently; seconds left.
    Cooldown(i64),
    /// The instance-wide hourly cap of confirmation e-mails was reached.
    Busy,
    Provider(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for FlowError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(e)
    }
}
impl From<ProviderError> for FlowError {
    fn from(e: ProviderError) -> Self {
        Self::Provider(e.to_string())
    }
}
impl From<SignError> for FlowError {
    fn from(e: SignError) -> Self {
        match e {
            SignError::NotFound => Self::NotFound,
            SignError::Closed => Self::Closed,
            SignError::Blocked => Self::Blocked,
            SignError::Db(e) => Self::Db(e),
        }
    }
}

/// What the visitor asked for.
pub(crate) struct Request<'a> {
    pub org: Uuid,
    pub petition: Uuid,
    pub signer: &'a Signer,
    pub show_name: bool,
}

/// Step 1. On success the visitor must check the inbox.
pub(crate) async fn request(
    db: &PgPool,
    cfg: &PetitionCfg,
    req: &Request<'_>,
) -> Result<(), FlowError> {
    let petition = store::get(db, req.org, req.petition, false)
        .await?
        .ok_or(FlowError::NotFound)?;
    if petition.status != "open" {
        return Err(FlowError::Closed);
    }
    cleanup_expired(db, cfg).await;
    // Limits kept in the database, so they hold across restarts and processes.
    if let Some(age) = store::seconds_since_sent(db, req.petition, &req.signer.email).await? {
        if age < cfg.resend_cooldown_secs {
            return Err(FlowError::Cooldown(cfg.resend_cooldown_secs - age));
        }
    }
    if store::sent_last_hour(db).await? >= cfg.max_emails_per_hour {
        tracing::warn!("petitions: hourly confirmation e-mail cap reached");
        return Err(FlowError::Busy);
    }
    let existing = store::pending_for_email(db, req.petition, &req.signer.email).await?;
    let id = existing.as_ref().map_or_else(Uuid::now_v7, |p| p.id);
    match cfg.verification {
        Verification::Local => request_local(db, cfg, req, id, &petition.title).await,
        Verification::Sso => {
            let provider = cfg.provider.as_ref().ok_or(FlowError::MailUnavailable)?;
            request_sso(db, cfg, provider, req, id).await
        }
    }
}

fn pending<'a>(req: &'a Request<'a>, id: Uuid, ttl: i64) -> PendingInput<'a> {
    PendingInput {
        id,
        initiative_id: req.petition,
        email: &req.signer.email,
        name: &req.signer.name,
        city: req.signer.city.as_deref(),
        show_name: req.show_name,
        token_hash: None,
        sso_user_id: None,
        sso_created: false,
        ttl_hours: ttl,
    }
}

async fn request_local(
    db: &PgPool,
    cfg: &PetitionCfg,
    req: &Request<'_>,
    id: Uuid,
    title: &str,
) -> Result<(), FlowError> {
    let smtp = crate::proposal_delivery::smtp_from_env().ok_or(FlowError::MailUnavailable)?;
    let (token, hash) = domain::new_token();
    let mut row = pending(req, id, cfg.pending_ttl_hours);
    row.token_hash = Some(&hash);
    store::upsert_pending(db, &row).await?;
    let link = format!("{}/abaixo-assinado/confirmar/?t={token}", cfg.origin);
    let body = format!(
        "Olá, {name}!\n\n\
         Recebemos seu pedido para assinar o abaixo-assinado:\n\n  \"{title}\"\n\n\
         Para confirmar sua assinatura, abra este link:\n\n{link}\n\n\
         {shown}\
         O link vale por {hours} horas. Se você não pediu isso, ignore esta mensagem: \
         nada será registrado.\n",
        shown = if req.show_name {
            "Seu nome aparecerá na lista pública de quem assinou.\n\n"
        } else {
            "Seu nome NÃO aparecerá na lista pública.\n\n"
        },
        name = req.signer.name,
        hours = cfg.pending_ttl_hours,
    );
    crate::mailer::send_html(&smtp, &req.signer.email, "Confirme sua assinatura", &body)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "petitions: confirmation e-mail failed");
            FlowError::MailUnavailable
        })
}

fn sso_template(cfg: &PetitionCfg, id: Uuid) -> String {
    format!(
        "{}/abaixo-assinado/confirmar/?p={id}&u={{{{.UserID}}}}&c={{{{.Code}}}}",
        cfg.origin
    )
}

async fn request_sso(
    db: &PgPool,
    cfg: &PetitionCfg,
    provider: &ProviderCfg,
    req: &Request<'_>,
    id: Uuid,
) -> Result<(), FlowError> {
    let z = Zitadel::new(provider);
    let template = sso_template(cfg, id);
    let mut row = pending(req, id, cfg.pending_ttl_hours);
    let user_id = match z.find_by_email(&req.signer.email).await? {
        Some(user) if user.email_verified => return Err(FlowError::AccountExists),
        Some(user) => {
            z.resend_code(&user.id, &template).await?;
            user.id
        }
        None => {
            let uid = create_provider_user(
                &z,
                cfg,
                &req.signer.name,
                &req.signer.email,
                EmailMode::SendCode(&template),
            )
            .await?;
            row.sso_created = true;
            uid
        }
    };
    row.sso_user_id = Some(&user_id);
    store::upsert_pending(db, &row).await?;
    Ok(())
}

/// Is this username free on the operator's other services? 404 means free. Any other
/// answer, or an error, counts as taken (fail closed).
async fn handle_free(cfg: &PetitionCfg, username: &str) -> bool {
    let Some(prefix) = cfg.handle_check_url.as_deref() else {
        return true;
    };
    matches!(
        guarded_get(
            &format!("{prefix}{username}"),
            &[],
            &OutboundPolicy::default()
        )
        .await,
        Err(OutboundError::Status(404))
    )
}

async fn create_provider_user(
    z: &Zitadel<'_>,
    cfg: &PetitionCfg,
    name: &str,
    email: &str,
    mode: EmailMode<'_>,
) -> Result<String, FlowError> {
    let stem = domain::username_stem(name);
    for _ in 0..USERNAME_ATTEMPTS {
        let candidate = domain::username_candidate(&stem, rand::random::<u16>());
        if !handle_free(cfg, &candidate).await {
            continue;
        }
        let mode = match &mode {
            EmailMode::SendCode(t) => EmailMode::SendCode(t),
            EmailMode::Verified => EmailMode::Verified,
        };
        match z.create_user(&candidate, name, email, mode).await? {
            Created::User(id) => return Ok(id),
            Created::UsernameTaken => {}
        }
    }
    Err(FlowError::Provider("no free username".to_owned()))
}

/// What the confirmation link carries.
pub(crate) enum Proof<'a> {
    /// local mode
    Token(&'a str),
    /// sso mode
    Code {
        pending: Uuid,
        user: &'a str,
        code: &'a str,
    },
}

/// The result shown on the confirmation page.
#[derive(Debug, serde::Serialize)]
pub(crate) struct Confirmed {
    #[serde(flatten)]
    pub tally: Tally,
    /// A provider account was created; a "choose your password" e-mail is on its way.
    pub account_created: bool,
}

/// Step 2. Everything runs in ONE transaction that first locks the pending row, so the
/// same link opened twice at once counts once. Provider calls happen while the row is
/// locked; the "choose your password" e-mail goes out only after the commit.
pub(crate) async fn confirm(
    db: &PgPool,
    cfg: &PetitionCfg,
    org: Uuid,
    proof: &Proof<'_>,
) -> Result<Confirmed, FlowError> {
    let mut tx = db.begin().await?;
    let (pending, subject, password_link_for) = match proof {
        Proof::Token(token) => {
            let p = store::claim_by_token(&mut tx, &domain::token_hash(token))
                .await?
                .ok_or(FlowError::InvalidLink)?;
            store::lock_address(&mut tx, &p.email).await?;
            let (subject, created) = match cfg.provider.as_ref() {
                Some(provider) => provision_verified(cfg, provider, &p.name, &p.email).await,
                None => (None, false),
            };
            let link_for = subject.clone().filter(|_| created);
            (p, subject, link_for)
        }
        Proof::Code {
            pending,
            user,
            code,
        } => {
            let provider = cfg.provider.as_ref().ok_or(FlowError::InvalidLink)?;
            let p = store::claim_by_id(&mut tx, *pending)
                .await?
                .filter(|p| p.sso_user_id.as_deref() == Some(*user))
                .ok_or(FlowError::InvalidLink)?;
            store::lock_address(&mut tx, &p.email).await?;
            let z = Zitadel::new(provider);
            // A code works once. If an earlier attempt used it and then failed here (the
            // pending row still exists), the address is already verified: accept that.
            let ok = z.verify_email(user, code).await? || z.email_verified(user).await?;
            if !ok {
                return Err(FlowError::InvalidLink);
            }
            let link_for = p.sso_created.then(|| (*user).to_owned());
            (p, Some((*user).to_owned()), link_for)
        }
    };

    let citizen = store::ensure_citizen(
        &mut tx,
        org,
        subject.as_deref(),
        &pending.email,
        &pending.name,
        None,
    )
    .await?;
    let public_name = pending.show_name.then_some(pending.name.as_str());
    let tally = store::sign(
        &mut tx,
        org,
        pending.initiative_id,
        citizen,
        public_name,
        pending.city.as_deref(),
    )
    .await?;
    store::delete_pending(&mut tx, pending.id).await?;
    tx.commit().await?;

    if let (Some(user), Some(provider)) = (password_link_for.as_deref(), cfg.provider.as_ref()) {
        if let Err(e) = Zitadel::new(provider).send_password_link(user).await {
            tracing::warn!(error = %e, "petitions: password link not sent");
        }
    }
    Ok(Confirmed {
        tally,
        account_created: password_link_for.is_some(),
    })
}

/// local mode with provisioning: the address is confirmed here, so the provider user is
/// created already verified. A provider failure does not lose the signature: the account
/// stays local, and a later SSO login links it by the verified address.
async fn provision_verified(
    cfg: &PetitionCfg,
    provider: &ProviderCfg,
    name: &str,
    email: &str,
) -> (Option<String>, bool) {
    let z = Zitadel::new(provider);
    let result = async {
        match z.find_by_email(email).await? {
            Some(user) if user.email_verified => return Ok::<_, FlowError>((user.id, false)),
            // An unverified provider user with this address may belong to someone else
            // who typed it; never link to it. The account stays local.
            Some(_) => {
                return Err(FlowError::Provider(
                    "address has an unverified provider user".into(),
                ))
            }
            None => {}
        }
        let id = create_provider_user(&z, cfg, name, email, EmailMode::Verified).await?;
        Ok((id, true))
    }
    .await;
    match result {
        Ok((id, created)) => (Some(id), created),
        Err(e) => {
            tracing::warn!(error = ?e, "petitions: provider account not created; kept local");
            (None, false)
        }
    }
}

/// Drop expired requests, and the never-confirmed provider users this server made.
async fn cleanup_expired(db: &PgPool, cfg: &PetitionCfg) {
    let ids = match store::take_expired(db, CLEANUP_BATCH).await {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(error = %e, "petitions: cleanup failed");
            return;
        }
    };
    let Some(provider) = cfg.provider.as_ref() else {
        return;
    };
    let z = Zitadel::new(provider);
    for id in ids {
        // Only users that are STILL unverified: a person may have confirmed through the
        // provider in another way meanwhile.
        if let Err(e) = z.delete_unverified(&id).await {
            tracing::warn!(error = %e, user = %id, "petitions: unconfirmed provider user not removed");
        }
    }
}
