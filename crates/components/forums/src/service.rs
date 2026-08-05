//! The forums service: path resolution/materialization, topics, votes,
//! comments and threshold firing — all transactional. Federated interactions
//! NEVER enter the count that fires (decision of plan v3).

use std::sync::Arc;

use dsoc_core::ids::{CitizenId, OrgId};
use dsoc_core::{Clock, Error, Result};
use dsoc_db::Db;
use uuid::Uuid;

use crate::domain::{self, territorial_sections, NewTopic, Stance, TerritorialSection};
use crate::queries::{self, CommentRow, DispatchRow, ForumRow, TopicRow};

// --- Config of the forums' proportional threshold (D3 of the critique plan) ---
//
// The office-dispatch trigger is no longer the fixed "10 points" of ADR-0019;
// it is now PROPORTIONAL to the electorate of the forum's territory, using the
// SAME formula as proposals (`dsoc_core::proportional_threshold`):
//
//     threshold = clamp( ceil(fraction × electorate), floor, ceiling )
//
// The default fraction (0.05%) is the proposals' own — ONE yardstick (B1). The
// floor of 10 preserves ADR-0019's behaviour exactly (a territory without data,
// or a small municipality, still requires 10). The ceiling avoids impossible
// targets in a capital/national forum. All env-overridable; fail-safe defaults.
const DEFAULT_FORUM_FRACTION: f64 = 0.0005;
const DEFAULT_FORUM_FLOOR: i64 = 10;
const DEFAULT_FORUM_CEIL: i64 = 10_000;

/// Privacy graduated by municipality size (D5/D6). Below this electorate, ONLY
/// the topic aggregate is public — the individual stance (who argued for or
/// against) is not attributed, so it never becomes a retaliation map in a small,
/// clientelist territory. Overridable via `SMALL_MUNICIPALITY_ELECTORATE`. Only
/// municipalities fall below it (the smallest UF has hundreds of thousands of
/// voters), so in practice this is the municipal yardstick.
const DEFAULT_SMALL_MUNICIPALITY_ELECTORATE: i64 = 5_000;

/// Read a fraction in `(0,1]` from env; out of range or absent → default (fail-safe).
fn env_f64(key: &str, default: f64) -> f64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| *v > 0.0 && *v <= 1.0)
        .unwrap_or(default)
}

/// Read a positive integer from env; absent/invalid → default (fail-safe).
fn env_i64(key: &str, default: i64) -> i64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &i64| *v > 0)
        .unwrap_or(default)
}

/// A child section in the tree: materialized (a real row) or virtual (a
/// territorial template with no topics yet — becomes a row on first use).
#[derive(Debug, Clone)]
pub struct ChildEntry {
    /// Segmento do caminho.
    pub slug: String,
    /// Caminho completo.
    pub full_path: String,
    /// Display name.
    pub name: String,
    /// `true` while not yet materialized.
    pub virtual_section: bool,
}

/// Detail of a topic already enriched with the territory-derived values:
/// patamar proporcional efetivo (D3) e privacidade agregada (D5/D6).
#[derive(Debug, Clone)]
pub struct TopicDetail {
    /// The topic.
    pub topic: TopicRow,
    /// Approved comments (local + federated).
    pub comments: Vec<CommentRow>,
    /// Institutional dispatch receipts.
    pub dispatches: Vec<DispatchRow>,
    /// This forum's effective proportional dispatch threshold (D3) — the score
    /// the scoreboard must cross to summon the office. The UI displays
    /// "faltam N" a partir daqui, em vez do antigo 10 fixo.
    pub escalation_threshold: i64,
    /// Small municipality (D5/D6): when `true`, individual stance attribution
    /// was omitted — only the aggregate is public.
    pub aggregate_only: bool,
    /// WHOM the scoreboard dispatches to once the threshold is crossed: names of
    /// reachable target mandates (B1) or the section's curated institutional contact.
    /// `None` = no reachable channel — the UI says the dispatch is pending
    /// instead of promising a delivery that never happens (Tier 0).
    pub escalation_destination: Option<String>,
}

/// A scored bridging argument (D8.2) — ready for the consensus UI. Carries the
/// comment, the cross-side endorsements and the bridge score (harmonic mean).
#[derive(Debug, Clone)]
pub struct BridgeComment {
    /// The comment/argument.
    pub comment: CommentRow,
    /// Endorsers whose stance on the topic is `favor`.
    pub favor_side: i64,
    /// Endorsers whose stance on the topic is `contra`.
    pub contra_side: i64,
    /// Bridge score = `domain::bridge_score(favor_side, contra_side)`.
    pub bridge_score: f64,
}

/// A topic's consensus (D8.2): the top bridging claims + the aggregate-privacy
/// flag (D5/D6), which the UI uses to pseudonymize the author.
#[derive(Debug, Clone)]
pub struct TopicConsensus {
    /// The bridging claims, already ordered by bridge score (desc) and cut at N.
    pub bridges: Vec<BridgeComment>,
    /// Small municipality: omit individual author attribution (D5/D6).
    pub aggregate_only: bool,
}

/// The forums service.
#[derive(Clone)]
pub struct ForumService {
    db: Db,
    clock: Arc<dyn Clock>,
}

impl std::fmt::Debug for ForumService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForumService").finish_non_exhaustive()
    }
}

impl ForumService {
    /// Build with explicit pool and clock (tests).
    #[must_use]
    pub fn new(db: Db, clock: Arc<dyn Clock>) -> Self {
        Self { db, clock }
    }

    /// Build from the shared state.
    #[must_use]
    pub fn from_state(state: &dsoc_app::AppState) -> Self {
        Self::new(state.db.clone(), state.clock.clone())
    }

    /// Resolve a path to its forum, MATERIALIZING the default territorial section
    /// when the last segment is a template (7 sections) under a state/municipality.
    ///
    /// Known collision: a municipality named after a section (e.g. the health
    /// section in BA under `ba/saude`) takes precedence — the same-named state
    /// section lives at `<uf>/<slug>-estado`.
    /// # Errors
    /// [`Error::NotFound`] when the path neither exists nor can be materialized;
    /// [`Error::Validation`] for a malformed path.
    pub async fn resolve_or_materialize(&self, org: OrgId, path: &str) -> Result<ForumRow> {
        let segments = domain::validate_path(path)?;
        let full_path = segments.join("/");
        match queries::get_forum_by_path(&self.db, org.as_uuid(), &full_path).await {
            Ok(row) => return Ok(row),
            Err(sqlx::Error::RowNotFound) => {}
            Err(e) => return Err(map_sqlx(e)),
        }
        // Does not exist — materialize only if the LAST segment is a template section
        // under an existing territorial parent.
        if segments.len() < 2 {
            return Err(Error::NotFound("fórum não encontrado".to_owned()));
        }
        let last: &str = segments.last().copied().unwrap_or_default();
        let parent_path = segments[..segments.len() - 1].join("/");
        let parent = queries::get_forum_by_path(&self.db, org.as_uuid(), &parent_path)
            .await
            .map_err(map_sqlx)?;
        let municipal = parent.esfera.as_deref() == Some("municipal");
        let estadual = parent.esfera.as_deref() == Some("estadual") && parent.parent_id.is_none();
        if !municipal && !estadual {
            return Err(Error::NotFound("fórum não encontrado".to_owned()));
        }
        // Accepts both the bare slug and the '-estado' variant (municipality collision).
        let base = last.strip_suffix("-estado").unwrap_or(last);
        let section: Option<TerritorialSection> = territorial_sections(municipal)
            .into_iter()
            .find(|s| s.slug == base);
        let Some(section) = section else {
            return Err(Error::NotFound("fórum não encontrado".to_owned()));
        };
        queries::insert_forum_idempotent(
            &self.db,
            Uuid::now_v7(),
            org.as_uuid(),
            Some(parent.id),
            last,
            &full_path,
            section.name,
            "institucional",
            parent.esfera.as_deref(),
            parent.uf.as_deref(),
            parent.municipio.as_deref(),
        )
        .await
        .map_err(map_sqlx)?;
        queries::get_forum_by_path(&self.db, org.as_uuid(), &full_path)
            .await
            .map_err(map_sqlx)
    }

    /// One level of the tree: the forum (when `path` is present) + materialized
    /// children + virtual sections of the territorial template not yet created.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`] as resolution dictates.
    pub async fn tree(
        &self,
        org: OrgId,
        path: Option<&str>,
        esfera: Option<&str>,
    ) -> Result<(Option<ForumRow>, Vec<ChildEntry>)> {
        let forum = match path {
            Some(p) => Some(self.resolve_or_materialize(org, p).await?),
            None => None,
        };
        let rows = queries::list_children(
            &self.db,
            org.as_uuid(),
            forum.as_ref().map(|f| f.id),
            esfera,
            6000,
        )
        .await
        .map_err(map_sqlx)?;
        let mut children: Vec<ChildEntry> = rows
            .iter()
            .map(|r| ChildEntry {
                slug: r.slug.clone(),
                full_path: r.full_path.clone(),
                name: r.name.clone(),
                virtual_section: false,
            })
            .collect();
        // Virtual template sections for territorial forums (state root / municipality).
        if let Some(f) = &forum {
            let municipal = f.esfera.as_deref() == Some("municipal");
            let estadual = f.esfera.as_deref() == Some("estadual") && f.parent_id.is_none();
            if municipal || estadual {
                for s in territorial_sections(municipal) {
                    // Collision with a same-named municipality: the state section uses '-estado'.
                    let slug = if estadual
                        && rows
                            .iter()
                            .any(|r| r.slug == s.slug && r.esfera.as_deref() == Some("municipal"))
                    {
                        format!("{}-estado", s.slug)
                    } else {
                        s.slug.to_owned()
                    };
                    let taken = children.iter().any(|c| c.slug == slug);
                    if !taken {
                        children.push(ChildEntry {
                            full_path: format!("{}/{}", f.full_path, slug),
                            slug,
                            name: s.name.to_owned(),
                            virtual_section: true,
                        });
                    }
                }
            }
        }
        Ok((forum, children))
    }

    /// Create a topic at `path` (materializing the section if needed), with
    /// OPTIONAL direction at mandate(s) (B1). `targets` are mandate_ids: the list
    /// is normalized (dedupe + cap [`domain::MAX_TOPIC_TARGETS`]), every mandate
    /// is validated as EXISTING, and the rows are written to `forum_topic_target`
    /// — all in the SAME transaction as the topic (a non-existent target aborts
    /// the whole creation). An empty list = topic with no target (current
    /// behaviour: dispatches to the section's curated contact once the threshold crosses).
    ///
    /// # Errors
    /// [`Error::Validation`] when a target does not exist; NotFound/Storage as applicable.
    /// as demais etapas.
    pub async fn create_topic(
        &self,
        org: OrgId,
        path: &str,
        author: CitizenId,
        new: &NewTopic,
        targets: &[Uuid],
    ) -> Result<TopicRow> {
        let forum = self.resolve_or_materialize(org, path).await?;
        let targets = domain::sanitize_targets(targets);
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let topic = queries::insert_topic(
            &mut *tx,
            Uuid::now_v7(),
            forum.id,
            author.as_uuid(),
            &new.title,
            &new.body,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        for mandate_id in &targets {
            // Validate BEFORE writing: a missing target → Validation (not an FK Conflict),
            // and the tx rollback discards the topic with it (creation is atomic).
            if !queries::mandate_exists(&mut *tx, *mandate_id)
                .await
                .map_err(map_sqlx)?
            {
                return Err(Error::Validation(format!(
                    "mandato inexistente: {mandate_id}"
                )));
            }
            queries::insert_topic_target(&mut *tx, topic.id, *mandate_id, now)
                .await
                .map_err(map_sqlx)?;
        }
        tx.commit().await.map_err(map_sqlx)?;
        Ok(topic)
    }

    /// List topics (`hot` = by score; otherwise most recent).
    ///
    /// # Errors
    /// [`Error::Storage`].
    pub async fn list_topics(
        &self,
        org: OrgId,
        path: &str,
        hot: bool,
        limit: i64,
        offset: i64,
    ) -> Result<(ForumRow, Vec<TopicRow>)> {
        let forum = self.resolve_or_materialize(org, path).await?;
        let topics =
            queries::list_topics(&self.db, forum.id, hot, limit.clamp(1, 100), offset.max(0))
                .await
                .map_err(map_sqlx)?;
        Ok((forum, topics))
    }

    /// Council transparency for a MUNICIPAL forum's header: cross-references the
    /// forum's `(uf, municipio)` with the `civic_source` catalog (0662/0669) and
    /// devolve `(status, site_oficial)` — `plena` | `parcial` | `ausente`.
    ///
    /// Returns `None` (no banner) when the forum is not municipal, when it has
    /// no `uf`/`municipio`, or when the catalog is unavailable — this is an
    /// ADDITIVE enrichment and must never break the topic listing.
    /// An uncatalogued municipality returns `Some(("ausente", None))`: the ABSENCE
    /// of data is precisely the public demand we want to display.
    pub async fn municipal_transparency(
        &self,
        forum: &ForumRow,
    ) -> Option<(String, Option<String>)> {
        if forum.esfera.as_deref() != Some("municipal") {
            return None;
        }
        let uf = forum.uf.as_deref()?;
        let municipio = forum.municipio.as_deref()?;
        match queries::municipal_transparency(&self.db, uf, municipio).await {
            Ok(Some(pair)) => Some(pair),
            Ok(None) => Some(("ausente".to_owned(), None)),
            Err(_) => None,
        }
    }

    /// Latest topics across all forums (the /f home feed).
    ///
    /// # Errors
    /// [`Error::Storage`].
    pub async fn recent_topics(
        &self,
        org: OrgId,
        limit: i64,
    ) -> Result<Vec<queries::RecentTopicRow>> {
        queries::list_recent_topics(&self.db, org.as_uuid(), limit.clamp(1, 50))
            .await
            .map_err(map_sqlx)
    }

    /// Topic detail + approved comments + dispatch receipts, plus the
    /// territory-derived values: the effective proportional threshold (D3) and the flag
    /// `aggregate_only` (D5/D6).
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`].
    pub async fn get_topic(&self, id: Uuid) -> Result<TopicDetail> {
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(topic) = queries::lock_topic(&mut *tx, id).await.map_err(map_sqlx)? else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        let comments = queries::list_comments(&mut *tx, id, None, 200)
            .await
            .map_err(map_sqlx)?;
        let dispatches = queries::list_dispatches(&mut *tx, id)
            .await
            .map_err(map_sqlx)?;
        // One electorate lookup feeds both derivations (D3 + D5/D6).
        let voters = queries::forum_territory_voters(&mut *tx, topic.forum_id)
            .await
            .map_err(map_sqlx)?;
        // Destino do encaminhamento, NOMEADO (a resposta a "encaminhar pra quem?"):
        // the topic's reachable targets (B1) or the section with a curated contact. None
        // channel → None (the UI says "pending"; we never promise a dead inbox — Tier 0).
        let targets = queries::topic_target_names(&mut *tx, id)
            .await
            .map_err(map_sqlx)?;
        let escalation_destination = if targets.is_empty() {
            queries::effective_contact_name(&mut *tx, topic.forum_id)
                .await
                .map_err(map_sqlx)?
        } else {
            let reachable: Vec<String> = targets
                .into_iter()
                .filter_map(|(name, ok)| ok.then_some(name))
                .collect();
            (!reachable.is_empty()).then(|| reachable.join(" · "))
        };
        tx.commit().await.map_err(map_sqlx)?;
        let escalation_threshold = dsoc_core::proportional_threshold(
            voters,
            env_f64("FORUM_THRESHOLD_FRACTION", DEFAULT_FORUM_FRACTION),
            env_i64("FORUM_THRESHOLD_FLOOR", DEFAULT_FORUM_FLOOR),
            env_i64("FORUM_THRESHOLD_CEIL", DEFAULT_FORUM_CEIL),
        );
        let aggregate_only = dsoc_core::is_small_electorate(
            voters,
            env_i64(
                "SMALL_MUNICIPALITY_ELECTORATE",
                DEFAULT_SMALL_MUNICIPALITY_ELECTORATE,
            ),
        );
        Ok(TopicDetail {
            topic,
            comments,
            dispatches,
            escalation_threshold,
            aggregate_only,
            escalation_destination,
        })
    }

    /// **Topic consensus** (D8.2): the top bridging claims — arguments endorsed
    /// ACROSS the for×against divide — ordered by bridge score (harmonic mean of
    /// both sides; see `domain::bridge_score`). An ADDITIVE layer on top of the
    /// cheering scoreboard: it highlights what UNITES those who disagree.
    ///
    /// Orders by bridge score desc, breaks ties by cross-endorsement volume and
    /// then by recency (deterministic), and cuts at `limit` (1..=20). Applies the
    /// SAME aggregate-privacy rule as the detail (D5/D6): in a small municipality
    /// the author is pseudonymized by the caller via `aggregate_only`.
    ///
    /// # Errors
    /// [`Error::NotFound`] when the topic does not exist; [`Error::Storage`] on I/O failure.
    pub async fn topic_consensus(&self, topic_id: Uuid, limit: usize) -> Result<TopicConsensus> {
        let limit = limit.clamp(1, 20);
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        // lock_topic doubles as the existence check and yields forum_id for the electorate.
        let Some(topic) = queries::lock_topic(&mut *tx, topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        let rows = queries::list_bridge_comments(&mut *tx, topic_id)
            .await
            .map_err(map_sqlx)?;
        let voters = queries::forum_territory_voters(&mut *tx, topic.forum_id)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;

        let aggregate_only = dsoc_core::is_small_electorate(
            voters,
            env_i64(
                "SMALL_MUNICIPALITY_ELECTORATE",
                DEFAULT_SMALL_MUNICIPALITY_ELECTORATE,
            ),
        );

        let mut bridges: Vec<BridgeComment> = rows
            .into_iter()
            .map(|r| BridgeComment {
                bridge_score: domain::bridge_score(r.favor_side, r.contra_side),
                comment: r.comment,
                favor_side: r.favor_side,
                contra_side: r.contra_side,
            })
            .collect();
        // Stable, deterministic ordering: score ↓, then cross-side volume ↓,
        // then most recent (a v7 id is monotonic in time) ↓.
        bridges.sort_by(|a, b| {
            b.bridge_score
                .total_cmp(&a.bridge_score)
                .then((b.favor_side + b.contra_side).cmp(&(a.favor_side + a.contra_side)))
                .then(b.comment.id.cmp(&a.comment.id))
        });
        bridges.truncate(limit);
        Ok(TopicConsensus {
            bridges,
            aggregate_only,
        })
    }

    /// Citizen stance (upsert; for/against/neutral) — recomputes
    /// contadores e dispara patamares pendentes.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Validation`]/[`Error::Storage`].
    pub async fn vote(
        &self,
        topic_id: Uuid,
        citizen: CitizenId,
        stance: Stance,
    ) -> Result<TopicRow> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(topic) = queries::lock_topic(&mut *tx, topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        queries::upsert_vote(&mut *tx, topic_id, citizen.as_uuid(), stance.as_str(), now)
            .await
            .map_err(map_sqlx)?;
        let updated = Self::after_interaction(&mut tx, &topic, now).await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(updated)
    }

    /// Local comment — with an optional stance (the debate model: argument +
    /// stance together; the stance also records/updates the author's vote).
    /// Recalcula contadores e dispara patamares pendentes.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Validation`]/[`Error::Storage`].
    pub async fn comment(
        &self,
        topic_id: Uuid,
        citizen: CitizenId,
        body: &str,
        stance: Option<Stance>,
    ) -> Result<TopicRow> {
        let body = domain::validate_comment(body)?;
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let Some(topic) = queries::lock_topic(&mut *tx, topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        queries::insert_local_comment(
            &mut *tx,
            Uuid::now_v7(),
            topic_id,
            citizen.as_uuid(),
            stance.map(Stance::as_str),
            &body,
            now,
        )
        .await
        .map_err(map_sqlx)?;
        if let Some(stance) = stance {
            queries::upsert_vote(&mut *tx, topic_id, citizen.as_uuid(), stance.as_str(), now)
                .await
                .map_err(map_sqlx)?;
        }
        let updated = Self::after_interaction(&mut tx, &topic, now).await?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok(updated)
    }

    /// Stance on an ARGUMENT (0545, StackOverflow style) — upsert; recomputes
    /// the argument's and the topic's counters (a vote on an argument is a
    /// countable interaction) and fires pending thresholds.
    ///
    /// # Errors
    /// [`Error::NotFound`]/[`Error::Storage`].
    pub async fn vote_comment(
        &self,
        comment_id: Uuid,
        citizen: CitizenId,
        stance: Stance,
    ) -> Result<(queries::CommentRow, TopicRow)> {
        let now = self.clock.now();
        let mut tx = self.db.begin().await.map_err(map_sqlx)?;
        let comment = queries::get_comment(&mut *tx, comment_id)
            .await
            .map_err(|e| match e {
                sqlx::Error::RowNotFound => Error::NotFound("argumento não encontrado".to_owned()),
                other => map_sqlx(other),
            })?;
        let Some(topic) = queries::lock_topic(&mut *tx, comment.topic_id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        // Karma (ADR-0019): the argument's AUTHOR gains/loses reputation from the votes received
        // (SO: favor=+10, contra=−2). Delta = value(new) − value(previous) to cover a vote switch.
        // Voting on your own comment yields no karma (anti-self-vote, SO style).
        let prev_stance = queries::comment_vote_stance(&mut *tx, comment_id, citizen.as_uuid())
            .await
            .map_err(map_sqlx)?;
        queries::upsert_comment_vote(
            &mut *tx,
            comment_id,
            citizen.as_uuid(),
            stance.as_str(),
            now,
        )
        .await
        .map_err(map_sqlx)?;
        if let Some(author) = comment.author_id {
            if author != citizen.as_uuid() {
                let delta = queries::karma_value(stance.as_str())
                    - prev_stance.as_deref().map_or(0, queries::karma_value);
                queries::add_citizen_karma(&mut *tx, author, delta)
                    .await
                    .map_err(map_sqlx)?;
            }
        }
        queries::refresh_comment_counters(&mut *tx, comment_id)
            .await
            .map_err(map_sqlx)?;
        let updated_topic = Self::after_interaction(&mut tx, &topic, now).await?;
        let updated_comment = queries::get_comment(&mut *tx, comment_id)
            .await
            .map_err(map_sqlx)?;
        tx.commit().await.map_err(map_sqlx)?;
        Ok((updated_comment, updated_topic))
    }

    /// Post-interaction (under the row lock): recount + thresholds. The threshold
    /// index only advances once a receipt is created — a forum WITHOUT a curated
    /// e-mail stays pending and fires retroactively when curation fills it in.
    async fn after_interaction(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        topic: &TopicRow,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<TopicRow> {
        let (_interactions, _fed, score) = queries::refresh_topic_counters(&mut **tx, topic.id)
            .await
            .map_err(map_sqlx)?;

        // ADR-0019 + D3: dispatch to the office when the SCOREBOARD (signed points) crosses the
        // threshold — exactly once. `next_threshold_idx` acts as a flag: 0 = not escalated, 1 = escalated.
        // The threshold is now PROPORTIONAL to the electorate of the forum's territory (floor 10 = the
        // former fixed cut; the ceiling avoids impossible targets in a capital/national forum): the same
        // relative effort in Roraima and in SP, and raw volume stops being trivially gameable. Only a
        // demand with net support (score ≥ threshold) escalates; a controversial one (low net) does not.
        // (A forum without a curated e-mail stays pending and fires when curation fills it in.)
        let escalation = Self::forum_escalation_threshold(&mut **tx, topic.forum_id).await?;
        if score >= escalation && topic.next_threshold_idx == 0 {
            // The receipt records the threshold ACTUALLY crossed (proportional), not the fixed "10".
            let recorded = i32::try_from(escalation).unwrap_or(i32::MAX);
            // B1 — Propose ≡ Forum merge: IF the topic has target(s), dispatch to the office of
            // EACH reachable target (one receipt per target); otherwise to the section's curated
            // contact (current behaviour). A single lookup decides the branch.
            let targets = queries::topic_targets(&mut **tx, topic.id)
                .await
                .map_err(map_sqlx)?;
            let dispatched = if targets.is_empty() {
                // No target → the section's curated contact.
                let email = queries::effective_contact_email(&mut **tx, topic.forum_id)
                    .await
                    .map_err(map_sqlx)?;
                if let Some(email) = email {
                    queries::insert_dispatch(
                        &mut **tx,
                        Uuid::now_v7(),
                        topic.id,
                        recorded,
                        &email,
                        None,
                        now,
                    )
                    .await
                    .map_err(map_sqlx)?;
                    true
                } else {
                    false
                }
            } else {
                // Directed (B1) — Tier 0: the placeholder already arrives as a NULL e-mail from
                // `topic_targets` (same filter as proposal_delivery). We NEVER record
                // a receipt for an unreachable target — the silence would be the platform's, not
                // the official's. One receipt per REACHABLE target (mandate_id discriminates in the UNIQUE).
                let mut any = false;
                for (mandate_id, reachable_email) in &targets {
                    if let Some(email) = reachable_email {
                        queries::insert_dispatch(
                            &mut **tx,
                            Uuid::now_v7(),
                            topic.id,
                            recorded,
                            email,
                            Some(*mandate_id),
                            now,
                        )
                        .await
                        .map_err(map_sqlx)?;
                        any = true;
                    }
                }
                any
            };
            // The index only advances when SOME receipt is created — a forum/office with no
            // reachable channel stays PENDING and fires retroactively once a real e-mail
            // appears (section curation OR the mandate's public_email).
            if dispatched {
                let _ = queries::advance_threshold_idx(
                    &mut **tx,
                    topic.id,
                    topic.next_threshold_idx,
                    1,
                )
                .await
                .map_err(map_sqlx)?;
            }
        }

        let Some(updated) = queries::lock_topic(&mut **tx, topic.id)
            .await
            .map_err(map_sqlx)?
        else {
            return Err(Error::NotFound("tópico não encontrado".to_owned()));
        };
        Ok(updated)
    }

    /// Dispatch threshold proportional to the electorate of the forum's
    /// territory (D3). `clamp(ceil(fraction × electorate), floor, ceiling)`. A territory
    /// without sphere/electorate → floor (fail-safe; never switches the trigger off).
    ///
    /// # Errors
    /// [`Error::Storage`] when the electorate query fails.
    async fn forum_escalation_threshold(
        executor: impl sqlx::PgExecutor<'_>,
        forum_id: Uuid,
    ) -> Result<i64> {
        let voters = queries::forum_territory_voters(executor, forum_id)
            .await
            .map_err(map_sqlx)?;
        let fraction = env_f64("FORUM_THRESHOLD_FRACTION", DEFAULT_FORUM_FRACTION);
        let floor = env_i64("FORUM_THRESHOLD_FLOOR", DEFAULT_FORUM_FLOOR);
        let ceil = env_i64("FORUM_THRESHOLD_CEIL", DEFAULT_FORUM_CEIL);
        Ok(dsoc_core::proportional_threshold(
            voters, fraction, floor, ceil,
        ))
    }
}

/// Map sqlx failures onto the canonical model (CONTRIBUTING.md convention).
fn map_sqlx(err: sqlx::Error) -> Error {
    match err {
        sqlx::Error::RowNotFound => Error::NotFound("fórum não encontrado".to_owned()),
        sqlx::Error::Database(ref db) if db.is_unique_violation() => {
            Error::Conflict("registro já existe".to_owned())
        }
        sqlx::Error::Database(ref db) if db.is_foreign_key_violation() => {
            Error::Conflict("referência inexistente".to_owned())
        }
        other => Error::Storage(Box::new(other)),
    }
}
