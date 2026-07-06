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

const PT_RELATIVE =
  typeof Intl !== 'undefined' && 'RelativeTimeFormat' in Intl
    ? new Intl.RelativeTimeFormat('pt-BR', { numeric: 'auto' })
    : null;

/** Relative pt-BR timestamp for feeds: "agora", "há 5 minutos", "há 2 horas",
 *  "ontem"/"há 3 dias" — falls back to the full date beyond a week. */
export function formatRelative(iso: string | undefined | null): string {
  if (!iso) return '—';
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return '—';
  const diffSec = Math.round((d.getTime() - Date.now()) / 1000);
  const abs = Math.abs(diffSec);
  if (abs < 60 || !PT_RELATIVE) {
    return abs < 60 ? 'agora' : formatDate(iso);
  }
  if (abs < 3600) return PT_RELATIVE.format(Math.trunc(diffSec / 60), 'minute');
  if (abs < 86400) return PT_RELATIVE.format(Math.trunc(diffSec / 3600), 'hour');
  if (abs < 7 * 86400) return PT_RELATIVE.format(Math.trunc(diffSec / 86400), 'day');
  return formatDate(iso);
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
