//! # Forum postman (F3) — sends the pending `forum_dispatch` rows.
//!
//! When a topic crosses an interaction threshold, `dsoc-forums` writes a
//! `forum_dispatch` with `sent_at NULL`. This sweep (a worker loop) takes the
//! pending ones, assembles the institutional e-mail — the debate's link + a sample of the
//! comments — sends it through the same SMTP as the other notifications and stamps
//! `sent_at` (guarded by `IS NULL`: a resend never duplicates). Without SMTP configured
//! (dev), loga e carimba — mesmo contrato do proposal_delivery.

use sqlx::PgPool;
use uuid::Uuid;

use crate::proposal_delivery::{send_email, smtp_from_env};

#[derive(sqlx::FromRow, Debug)]
struct PendingDispatch {
    id: Uuid,
    topic_id: Uuid,
    threshold: i32,
    contact_email: String,
    title: String,
    interaction_count: i64,
    score: i64,
    forum_name: String,
    full_path: String,
}

/// One pass of the postman: processes up to 10 pending rows (the worker loop repeats).
pub(crate) async fn sweep(db: &PgPool, public_origin: &str) {
    // F4: the same tick also sweeps the forums' federated Announces.
    crate::forum_federation::announce_sweep(db, public_origin).await;
    let rows: Vec<PendingDispatch> = match sqlx::query_as(
        r"SELECT d.id, d.topic_id, d.threshold, d.contact_email,
                 t.title, t.interaction_count, t.score,
                 f.name AS forum_name, f.full_path
            FROM forum_dispatch d
            JOIN forum_topic t ON t.id = d.topic_id
            JOIN forum f ON f.id = t.forum_id
           WHERE d.sent_at IS NULL
           ORDER BY d.created_at
           LIMIT 10",
    )
    .fetch_all(db)
    .await
    {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(?err, "forum_mailer: lookup falhou");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }
    let smtp = smtp_from_env();
    let origin = public_origin.trim_end_matches('/');

    for d in rows {
        // Debate sample: up to 5 approved local comments (oldest first —
        // v1; once comments have their own votes, switch to most-voted).
        let comments: Vec<(String,)> = sqlx::query_as(
            r"SELECT body FROM forum_topic_comment
               WHERE topic_id = $1 AND moderation = 'approved' AND NOT federated
               ORDER BY id LIMIT 5",
        )
        .bind(d.topic_id)
        .fetch_all(db)
        .await
        .unwrap_or_default();
        let sample: String = comments
            .iter()
            .map(|(b,)| {
                let short: String = b.chars().take(280).collect();
                format!("• {short}\n")
            })
            .collect();

        let url = format!("{origin}/f/topico/{}", d.topic_id);
        let subject = format!(
            "[DemocraciaBR] Debate público atingiu {} interações — {}",
            d.threshold, d.title
        );
        let body = format!(
            "Prezada(o) responsável — {},\n\n\
             Um debate público na plataforma DemocraciaBR, no fórum \"{}\" (/f/{}),\n\
             cruzou o patamar de {} interações de cidadãos verificados\n\
             (total atual: {}, saldo de votos: {}).\n\n\
             Tópico: {}\n\
             Link público: {}\n\n\
             Amostra das contribuições:\n{}\n\
             Este é um aviso automático com registro público — o envio fica\n\
             carimbado no próprio tópico, visível a qualquer cidadão.\n\n\
             — DemocraciaBR (sistema automático)\n",
            d.forum_name,
            d.forum_name,
            d.full_path,
            d.threshold,
            d.interaction_count,
            d.score,
            d.title,
            url,
            if sample.is_empty() {
                "(sem comentários ainda — o debate está nos votos)\n".to_owned()
            } else {
                sample
            },
        );

        let sent = match &smtp {
            Some(cfg) => match send_email(cfg, &d.contact_email, &subject, &body).await {
                Ok(()) => true,
                Err(err) => {
                    tracing::warn!(?err, to = %d.contact_email, "forum_mailer: SMTP falhou");
                    false
                }
            },
            None => {
                tracing::info!(
                    target: "forum_mailer",
                    to = %d.contact_email, subject = %subject,
                    "DEV: SMTP unconfigured; e-mail logado em vez de enviado."
                );
                true
            }
        };
        if sent {
            if let Err(err) = sqlx::query(
                "UPDATE forum_dispatch SET sent_at = now() WHERE id = $1 AND sent_at IS NULL",
            )
            .bind(d.id)
            .execute(db)
            .await
            {
                tracing::warn!(?err, dispatch = %d.id, "forum_mailer: stamp falhou");
            } else {
                tracing::info!(topic = %d.topic_id, threshold = d.threshold,
                    "forum_mailer: envio institucional carimbado");
            }
        }
    }
}
