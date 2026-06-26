// Portuguese (pt-BR) formatting helpers for dates, latency and counts.

const PT_DATE = new Intl.DateTimeFormat('pt-BR', {
  day: '2-digit',
  month: 'long',
  year: 'numeric',
});

/** Format an ISO-8601 timestamp as e.g. "25 de junho de 2026". */
export function formatDate(iso: string | undefined | null): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  return PT_DATE.format(d);
}

/** Human latency from hours, e.g. 2.5 → "2h 30min", 40 → "1 dia 16h". */
export function formatLatency(hours: number | null | undefined): string {
  if (hours == null) return 'sem resposta ainda';
  if (hours < 1) return `${Math.round(hours * 60)} min`;
  const days = Math.floor(hours / 24);
  const h = Math.floor(hours % 24);
  const m = Math.round((hours - Math.floor(hours)) * 60);
  if (days > 0) return `${days} dia${days > 1 ? 's' : ''} ${h}h`;
  if (m > 0 && h < 10) return `${h}h ${m}min`;
  return `${h}h`;
}

/** Pluralizing count helper: countLabel(3, "proposta", "propostas"). */
export function countLabel(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** Responsiveness rate (answered / total) as a 0–100 integer, or null. */
export function responseRate(
  answered: number,
  ignored: number,
): number | null {
  const total = answered + ignored;
  if (total === 0) return null;
  return Math.round((answered / total) * 100);
}
