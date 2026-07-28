<script lang="ts">
  // Painel admin: mapa município→plataforma (civic_source, #72/ADR-0017). Qual sistema cada
  // câmara roda (SAPL/Interlegis…), URL-base e status do probe. Paginado/filtrado.
  import { onMount } from 'svelte';
  import {
    getCivicSourcesOverview,
    getCivicSources,
    type CivicSourceOverviewRow,
    type CivicSource,
  } from '../../lib/api';
  import { UFS } from '../../lib/ufs';

  const PLATFORMS = [
    { v: '', label: 'Todas as plataformas' },
    { v: 'sapl', label: 'SAPL / Interlegis' },
    { v: 'camaraonline', label: 'camaraonline' },
    { v: 'ipm', label: 'IPM' },
    { v: 'camarasempapel', label: 'Câmara Sem Papel' },
    { v: 'other', label: 'Outra' },
    { v: 'unknown', label: 'Desconhecida' },
    { v: 'none', label: 'Sem portal' },
  ];
  const PLAT_LABEL: Record<string, string> = {
    sapl: 'SAPL', camaraonline: 'camaraonline', ipm: 'IPM',
    camarasempapel: 'Câmara Sem Papel', other: 'Outra', unknown: 'Desconhecida', none: 'Sem portal',
  };
  const STATUS_LABEL: Record<string, string> = {
    ok: 'OK', dead: 'Sem resposta', pending: 'A probar', blocked: 'Bloqueada',
  };

  const PAGE = 50;
  let overview = $state<CivicSourceOverviewRow[]>([]);
  let items = $state<CivicSource[]>([]);
  let total = $state(0);
  let offset = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let uf = $state('');
  let platform = $state('');
  let status = $state('');
  let q = $state('');

  async function loadOverview() {
    const res = await getCivicSourcesOverview();
    if (res.success && res.data) overview = res.data;
  }
  async function loadPage() {
    loading = true;
    error = null;
    const res = await getCivicSources({ uf, platform, status, q, limit: PAGE, offset });
    loading = false;
    if (res.success && res.data) {
      items = res.data.items;
      total = res.data.total;
    } else {
      error = res.error?.message ?? 'Não foi possível carregar.';
    }
  }
  function applyFilters() {
    offset = 0;
    loadPage();
  }
  function next() {
    if (offset + PAGE < total) { offset += PAGE; loadPage(); }
  }
  function prev() {
    if (offset > 0) { offset = Math.max(0, offset - PAGE); loadPage(); }
  }
  function fmtDate(s: string | null): string {
    if (!s) return '—';
    return new Date(s).toLocaleDateString('pt-BR');
  }

  onMount(async () => {
    await Promise.all([loadOverview(), loadPage()]);
  });

  const from = $derived(total === 0 ? 0 : offset + 1);
  const to = $derived(Math.min(offset + PAGE, total));
  const saplOk = $derived(
    overview.find((o) => o.platform === 'sapl' && o.probe_status === 'ok')?.total ?? 0,
  );
</script>

<div class="funnel">
  <div class="stat hl">
    <span class="cargo">SAPL extraível</span>
    <span class="nums"><b class="real">{saplOk.toLocaleString('pt-BR')}</b> câmaras OK</span>
    <span class="muted small">prontas para extração</span>
  </div>
  {#each overview as o (o.platform + o.probe_status)}
    <div class="stat">
      <span class="cargo">{PLAT_LABEL[o.platform] ?? o.platform}</span>
      <span class="nums">
        <b>{o.total.toLocaleString('pt-BR')}</b>
        · <span class="muted small">{STATUS_LABEL[o.probe_status] ?? o.probe_status}</span>
      </span>
      {#if o.parlamentares}<span class="muted small">{o.parlamentares.toLocaleString('pt-BR')} parlamentares</span>{/if}
    </div>
  {/each}
</div>

<div class="filters">
  <select bind:value={uf} onchange={applyFilters}>
    <option value="">UF (todas)</option>
    {#each UFS as u (u.code)}<option value={u.code}>{u.code}</option>{/each}
  </select>
  <select bind:value={platform} onchange={applyFilters}>
    {#each PLATFORMS as pf (pf.v)}<option value={pf.v}>{pf.label}</option>{/each}
  </select>
  <select bind:value={status} onchange={applyFilters}>
    <option value="">Status: todos</option>
    <option value="ok">OK</option>
    <option value="dead">Sem resposta</option>
    <option value="pending">A probar</option>
    <option value="blocked">Bloqueada</option>
  </select>
  <input type="search" bind:value={q} placeholder="Buscar município…"
    onkeydown={(e) => e.key === 'Enter' && applyFilters()} />
  <button class="btn" onclick={applyFilters}>Filtrar</button>
</div>

{#if error}
  <div class="card err" role="alert">{error}</div>
{:else}
  <div class="pager">
    <span class="muted">{from.toLocaleString('pt-BR')}–{to.toLocaleString('pt-BR')} de {total.toLocaleString('pt-BR')}</span>
    <span class="pgbtns">
      <button class="btn ghost" onclick={prev} disabled={offset === 0 || loading}>‹ Anterior</button>
      <button class="btn ghost" onclick={next} disabled={offset + PAGE >= total || loading}>Próxima ›</button>
    </span>
  </div>
  <div class="table-wrap">
    <table>
      <thead><tr><th>UF</th><th>Município</th><th>Plataforma</th><th>Status</th><th>Fonte</th><th>Parl.</th><th>Último probe</th></tr></thead>
      <tbody>
        {#if loading}
          <tr><td colspan="7" class="muted center">Carregando…</td></tr>
        {:else if items.length === 0}
          <tr><td colspan="7" class="muted center">Nenhum resultado.</td></tr>
        {:else}
          {#each items as s (s.id)}
            <tr>
              <td class="small">{s.uf}</td>
              <td>{s.municipio}</td>
              <td class="small">
                <span class="tag {s.platform}">{PLAT_LABEL[s.platform] ?? s.platform}</span>
              </td>
              <td class="small">
                <span class="dot {s.probe_status}"></span>{STATUS_LABEL[s.probe_status] ?? s.probe_status}
              </td>
              <td class="src">
                {#if s.base_url}<a href={s.base_url} target="_blank" rel="noopener">{s.base_url.replace('https://', '')}</a>{:else}—{/if}
              </td>
              <td class="small center">{s.parlamentares_found ?? '—'}</td>
              <td class="small">{fmtDate(s.last_probed_at)}</td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .funnel { display: flex; gap: 0.7rem; flex-wrap: wrap; margin-bottom: 1.2rem; }
  .stat { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 10px; padding: 0.7rem 1rem; display: grid; gap: 0.1rem; min-width: 9rem; }
  .stat.hl { border-color: var(--c-green-dark, #15803d); }
  .cargo { font-weight: 700; }
  .nums { font-size: 0.9rem; }
  .real { color: var(--c-green-dark, #15803d); }
  .filters { display: flex; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 1rem; }
  .filters select, .filters input { padding: 0.5rem 0.6rem; border-radius: 8px; border: 1px solid var(--border-subtle, #cbd5e1); background: var(--surface-1, #fff); color: inherit; font: inherit; }
  .filters input { flex: 1; min-width: 12rem; }
  .btn { padding: 0.5rem 1rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .btn.ghost { border-color: var(--border-subtle, #cbd5e1); }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .pager { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.6rem; }
  .pgbtns { display: flex; gap: 0.5rem; }
  .table-wrap { overflow-x: auto; border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
  th, td { text-align: left; padding: 0.55rem 0.8rem; border-bottom: 1px solid var(--border-subtle, rgba(0,0,0,0.06)); }
  th { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.03em; color: var(--muted, #64748b); }
  .small { font-size: 0.82rem; }
  .center { text-align: center; }
  .src { font-family: ui-monospace, monospace; font-size: 0.8rem; }
  .src a { color: var(--c-blue, #1d4ed8); text-decoration: none; }
  .src a:hover { text-decoration: underline; }
  .tag { padding: 0.1rem 0.45rem; border-radius: 6px; font-size: 0.75rem; font-weight: 600; background: var(--surface-2, #f1f5f9); }
  .tag.sapl { background: #dcfce7; color: #15803d; }
  .dot { display: inline-block; width: 0.55rem; height: 0.55rem; border-radius: 50%; margin-right: 0.4rem; vertical-align: middle; }
  .dot.ok { background: #22c55e; }
  .dot.dead { background: #94a3b8; }
  .dot.pending { background: #f59e0b; }
  .dot.blocked { background: #ef4444; }
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1.5rem; }
  .err { color: #dc2626; }
</style>
