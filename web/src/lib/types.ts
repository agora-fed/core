// Public DTO shapes mirrored from the Rust `dsoc-api-contract` crate.
// These are the *frozen* public shapes consumed by web and mobile as peers.

/** Stable machine error code + Portuguese, non-sensitive message. */
export interface ApiError {
  code: string;
  message: string;
}

/** Pagination metadata for list responses. */
export interface PageMeta {
  total: number;
  limit: number;
  offset: number;
}

/** The uniform envelope. Exactly one of `data`/`error` is populated. */
export interface ApiResponse<T> {
  success: boolean;
  data: T | null;
  error: ApiError | null;
  meta: PageMeta | null;
}

/** Public view of a proposal. */
export interface ProposalDto {
  id: string;
  title: string;
  body: string;
  mandate_id: string;
  cluster_id: string | null;
  support_count: number;
  /** Support count at which the consequence loop fires. */
  threshold: number;
  /** Author user-chosen handle (`@fulana`). `null` for anonymous / platform-seeded proposals. */
  author_handle: string | null;
  /** Opaque public handle (`u-<hex>`) of the author. UI fallback when no @handle. */
  author_public_handle: string | null;
  /** Author avatar URL (already composed with MEDIA_BASE). `null` ⇒ render initials. */
  author_avatar_url: string | null;
  created_at: string;
}

/** Authenticated citizen's own profile (returned by `GET /me`). Never carries CPF/e-mail. */
export interface ProfileDto {
  citizen_id: string;
  org_id: string;
  handle: string | null;
  public_handle: string;
  display_name: string | null;
  bio: string | null;
  avatar_url: string | null;
  cover_url: string | null;
  is_public: boolean;
  verification_level: string;
  created_at: string;
}

/** What `GET /me/mandate` returns — the mandate the authenticated citizen operates, if any. */
export interface MyMandateDto {
  mandate: MandateDto | null;
  binding_level: string | null;
}

/** One active session of the authenticated citizen, returned by `GET /me/sessions`. */
export interface SessionInfoDto {
  id: string;
  issued_at: string;
  expires_at: string;
  current: boolean;
}

/** Editable subset of `ProfileDto` accepted by `PATCH /me`. */
export interface ProfileUpdateDto {
  display_name?: string;
  bio?: string;
  handle?: string;
  is_public?: boolean;
}

/** Public view of a mandate / candidacy. */
export interface MandateDto {
  id: string;
  office: string;
  display_name: string;
  is_candidate: boolean;
  onboarded: boolean;
  party: string | null;
  uf: string | null;
  house: 'camara' | 'senado' | null;
  avatar_url: string | null;
  /** Public accountability e-mail (added in F1.1). */
  public_email?: string | null;
  /** Federative sphere (added in F1.2, migration 0203). Legacy = 'federal'. */
  sphere?: 'federal' | 'estadual' | 'municipal';
  /** Aggregate signal: this mandate has a verified operator bound (LGPD-safe boolean). */
  has_verified_operator?: boolean;
}

/** State of a consequence SLA — the emotional core of the UI. */
export type SlaStatus = 'pending' | 'answered' | 'acted' | 'ignored';

/** Public per-politician scorecard summary. */
export interface ScorecardDto {
  mandate_id: string;
  answered: number;
  ignored: number;
  median_response_hours: number | null;
}

/** Public view of an SLA clock. */
export interface SlaDto {
  id: string;
  org_id: string;
  mandate_id: string;
  cluster_id: string;
  proposal_id: string;
  status: SlaStatus;
  started_at: string;
  due_at: string;
  created_at: string;
}

/** Public view of a promise ("promises vs delivery"). */
export interface PromiseDto {
  id: string;
  scorecard_id: string;
  text: string;
  made_at: string;
  delivered: boolean;
  delivered_at: string | null;
}

/** Public view of a debate. `framing` é o enunciado (não `body`). */
export interface DebateDto {
  id: string;
  org_id: string;
  title: string;
  framing: string;
  created_at: string;
}

/** Uma contribuição a um debate (posição pró/contra/neutro). */
export interface ContributionDto {
  id: string;
  debate_id: string;
  author_id: string;
  stance: 'pro' | 'con' | 'neutral';
  body: string;
  created_at: string;
}

/** Public view of a consultation / survey (listing page). */
export interface ConsultationDto {
  id: string;
  title: string;
  body?: string;
  created_at?: string;
}

// --- Consultas participativas (Fase 3.3, migration 0531) ----------------------

/** Item da lista pública de consultas. */
export interface ConsultaSummary {
  id: string;
  title: string;
  status: 'open' | 'closed';
  opens_at: string;
  closes_at: string;
  question_count: number;
}

/** Agregado das respostas de uma pergunta. */
export interface ConsultaTally {
  concordo: number;
  neutro: number;
  discordo: number;
  total: number;
}

/** Pergunta de uma consulta, com agregado e a resposta do caller (se logado). */
export interface ConsultaQuestion {
  id: string;
  prompt: string;
  position: number;
  tally: ConsultaTally;
  my_answer: 'concordo' | 'neutro' | 'discordo' | null;
}

/** Detalhe público de uma consulta. */
export interface ConsultaDetail {
  id: string;
  title: string;
  status: 'open' | 'closed';
  opens_at: string;
  closes_at: string;
  questions: ConsultaQuestion[];
}

// --- Parties (Fase 2B, migration 0204) ----------------------------------------

/** Public view of a political party. */
export interface PartyDto {
  sigla: string;
  name: string;
  tse_number: number | null;
  logo_url: string | null;
  website: string | null;
  founded_year: number | null;
  /** Mandates currently attributed to this sigla in the org (derived, não-ocultos). */
  mandate_count: number;
  federal_count: number;
  estadual_count: number;
  municipal_count: number;
}

/** Public view of a subnational directory of a party. */
export interface PartyDirectoryDto {
  id: string;
  party_sigla: string;
  esfera: 'federal' | 'estadual' | 'municipal';
  uf: string | null;
  municipio: string | null;
  name: string;
  parent_directory_id: string | null;
}

/** Public (privacy-safe) view of a party administrator. Never carries the citizen id/email. */
export interface AdminBriefDto {
  public_handle: string | null;
  display_name: string | null;
  role: 'admin' | 'moderador';
  directory_id: string | null;
}

/** Detail response for a single party. PartyDto fields are flattened at the top level. */
export interface PartyDetailDto extends PartyDto {
  directories: PartyDirectoryDto[];
  administrators: AdminBriefDto[];
}

// --- Mandate invite (F1.4 bypass — invite by e-mail) ---------------------------

/** Public summary of a mandate-invite (nothing that identifies inviter or recipient). */
export interface MandateInviteSummaryDto {
  mandate_display_name: string;
  party: string | null;
  uf: string | null;
  office: string;
  expires_at: string;
}

// --- Deliberation (comments) -------------------------------------------------

/** Public view of a comment in a proposal's debate thread. */
export interface CommentDto {
  id: string;
  org_id: string;
  proposal_id: string;
  /** Parent comment id, or `null` for a root. */
  parent_id: string | null;
  author_id: string;
  body: string;
  /** Nesting depth (0 for a root). */
  depth: number;
  /** `visible` | `flagged` | `hidden`. */
  status: string;
  /** Author user-chosen handle (`@fulana`). `null` if the citizen never picked one. */
  author_handle: string | null;
  /** Author public display name, if any. */
  author_display_name: string | null;
  /** Opaque public handle (`u-<hex>`) — fallback when there is no `@handle`. */
  author_public_handle: string | null;
  /** Author avatar URL (already composed with MEDIA_BASE). `null` ⇒ render initials. */
  author_avatar_url: string | null;
  created_at: string;
}

// --- Fediverse feed & reactions (migration 0403) ------------------------------

/** One item in the authenticated citizen's federated feed (own + followed notes). */
export interface FeedItemDto {
  /** Canonical ActivityPub object URI of the note (stable reaction target). */
  object_uri: string;
  /** `@user` for local authors, `user@remote.tld` for remote ones. */
  author_handle: string;
  author_display_name: string | null;
  author_avatar_url: string | null;
  /** Sanitized HTML content of the note. */
  content_html: string;
  published_at: string;
  is_remote: boolean;
  like_count: number;
  boost_count: number;
  liked_by_me: boolean;
  boosted_by_me: boolean;
  /** 0.18.0: parent Note URI for threaded replies. Absent = top-level. */
  in_reply_to_uri?: string | null;
  /** 0.18.0: Mastodon-style sensitive/nsfw flag. */
  sensitive?: boolean;
  /** 0.18.0: content-warning header (shown before the collapsed content). */
  spoiler_text?: string | null;
  /** 0.18.0-gamma: media attachments (empty when the note has none). */
  attachments?: MediaAttachmentDto[];
  /** 0.18.0-rc1: when the author edited the note. Absent = never edited. */
  edited_at?: string | null;
  /** 0.18.0-rc1: poll — undefined for regular Notes. */
  poll?: PollDto | null;
}

/** One media attachment on a Note (image today; video/audio reserved). */
export interface MediaAttachmentDto {
  id: string;
  url: string;
  kind: 'image' | 'video' | 'audio';
  content_type: string;
  alt_text?: string | null;
  width?: number | null;
  height?: number | null;
}

/** One poll option (bar in PollView). */
export interface PollOptionDto {
  id: string;
  sort_order: number;
  text: string;
  vote_count: number;
}

/** Poll bundled with a note in FeedItemDto. */
export interface PollDto {
  id: string;
  multiple: boolean;
  expires_at: string;
  closed_at?: string | null;
  options: PollOptionDto[];
  total_votes: number;
  voted_option_ids: string[];
}

/** Result of toggling a Like on a note. */
export interface LikeResultDto {
  liked: boolean;
  like_count: number;
}

/** Result of toggling an Announce (boost) on a note. */
export interface BoostResultDto {
  boosted: boolean;
  boost_count: number;
}

// --- Parliamentarian public activity (proxy Câmara/Senado) ---

/** One upcoming/recent public event or session from the house's agenda. */
export interface AgendaItem {
  title: string;
  date_time: string | null;
  location: string | null;
  kind: string | null;
  url: string | null;
}

/** One authored proposição / matéria (legislative production). */
export interface ProductionItem {
  id: string | null;
  kind: string | null;
  number: string | null;
  year: string | null;
  summary: string | null;
  url: string | null;
}

/** One floor speech (Câmara discurso / Senado pronunciamento). */
export interface SpeechItem {
  date_time: string | null;
  summary: string | null;
  url: string | null;
}

/** One nominal vote cast (Senado only for now; Câmara omits). */
export interface VoteItem {
  date: string | null;
  matter: string | null;
  summary: string | null;
  vote: string | null;
  result: string | null;
}

/** One category inside the annual expense summary. */
export interface ExpenseCategory {
  name: string;
  amount: number;
}

/** Expense summary (Câmara cota parlamentar). `null` for Senado / unavailable. */
export interface ExpenseSummary {
  year: number | null;
  total: number;
  top_categories: ExpenseCategory[];
}

/** Extra profile bits from the house's API (not on `MandateDto`). */
export interface ProfileExtra {
  social: string[];
  office_contact: string | null;
  photo_url: string | null;
  page_url: string | null;
}

/** Normalized public activity payload for a mandate. Empty vecs = no data. */
export interface ActivityDto {
  /** `camara` | `senado` | `manual` | null (the mandate's `source`). */
  source: string | null;
  /** Numeric external id in that house's API. */
  external_id: string | null;
  agenda: AgendaItem[];
  production: ProductionItem[];
  speeches: SpeechItem[];
  votes: VoteItem[];
  expenses: ExpenseSummary | null;
  profile_extra: ProfileExtra | null;
}
