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

/** Public view of a debate (listing page). */
export interface DebateDto {
  id: string;
  title: string;
  body?: string;
  created_at?: string;
}

/** Public view of a consultation / survey (listing page). */
export interface ConsultationDto {
  id: string;
  title: string;
  body?: string;
  created_at?: string;
}
