// Branding panel <-> API contract helpers, extracted PURE so vitest can pin
// them (production incident 2026-08-05: the island read `res.ok` — a Fetched
// field — on an ApiResponse envelope, so every load fell into the error
// branch; no frontend test existed to catch it).
import type { ApiResponse } from './types';
import type { BrandingDto } from './api';

export type BrandingLoad =
  | { loaded: BrandingDto }
  | { failed: string };

/** Decide what the panel shows from the admin GET/PUT envelope. */
export function brandingLoadState(res: ApiResponse<BrandingDto>): BrandingLoad {
  if (res.success && res.data) return { loaded: res.data };
  return { failed: res.error?.message ?? 'Falha ao carregar identidade visual.' };
}
