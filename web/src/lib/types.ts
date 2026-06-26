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
  created_at: string;
}

/** Public view of a mandate / candidacy. */
export interface MandateDto {
  id: string;
  office: string;
  display_name: string;
  is_candidate: boolean;
  onboarded: boolean;
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
