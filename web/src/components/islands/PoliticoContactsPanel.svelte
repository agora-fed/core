<script lang="ts">
  // Painel admin: contatos (e-mail) de todos os políticos, com selo real vs
  // placeholder. Paginado/filtrado — nunca puxa os ~70k de uma vez.
  import { onMount } from 'svelte';
  import {
    getPoliticoContactsOverview,
    getPoliticoContacts,
    type PoliticoContactOverviewRow,
    type PoliticoContact,
  } from '../../lib/api';
  import { UFS } from '../../lib/ufs';

  const CARGOS = [
    { v: '', label: 'Todos os cargos' },
    { v: 'vereador', label: 'Vereador(a)' },
    { v: 'dep_estadual', label: 'Dep. Estadual/Distrital' },
    { v: 'dep_federal', label: 'Dep. Federal' },
    { v: 'senador', label: 'Senador(a)' },
    { v: 'governador', label: 'Governador(a)' },
    { v: 'prefeito', label: 'Prefeito(a)/Vice' },
  ];
  const CARGO_LABEL: Record<string, string> = {
    vereador: 'Vereador',
    dep_estadual: 'Dep. Estadual',
    dep_federal: 'Dep. Federal',
    senador: 'Senador',
    governador: 'Governador',
    prefeito: 'Prefeito/Vice',
    outro: 'Outro',
  };

  const PAGE = 50;
  let overview = $state<PoliticoContactOverviewRow[]>([]);
  let items = $state<PoliticoContact[]>([]);
  let total = $state(0);
  let offset = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let cargo = $state('');
  let uf = $state('');
  let status = $state('');
  let q = $state('');

  async function loadOverview() {
    const res = await getPoliticoContactsOverview();
    if (res.success && res.data) overview = res.data;
  }
  async function loadPage() {
    loading = true;
    error = null;
    const res = await getPoliticoContacts({ cargo, uf, status, q, limit: PAGE, offset });
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
    if (offset + PAGE < total) {
      offset += PAGE;
      loadPage();
    }
  }
  function prev() {
    if (offset > 0) {
      offset = Math.max(0, offset - PAGE);
      loadPage();
    }
  }

  onMount(async () => {
    await Promise.all([loadOverview(), loadPage()]);
  });

  const from = $derived(total === 0 ? 0 : offset + 1);
  const to = $derived(Math.min(offset + PAGE, total));
</script>

<div class="funnel">
  {#each overview as o (o.cargo)}
    <div class="stat">
      <span class="cargo">{CARGO_LABEL[o.cargo] ?? o.cargo}</span>
      <span class="nums">
        <b class="real">{o.com_email.toLocaleString('pt-BR')}</b> reais
        · <span class="ph">{o.placeholder.toLocaleString('pt-BR')}</span> placeholder
      </span>
      <span class="muted small">de {o.total.toLocaleString('pt-BR')}</span>
    </div>
  {/each}
</div>

<div class="filters">
  <select bind:value={cargo} onchange={applyFilters}>
    {#each CARGOS as c (c.v)}<option value={c.v}>{c.label}</option>{/each}
  </select>
  <select bind:value={uf} onchange={applyFilters}>
    <option value="">UF (todas)</option>
    {#each UFS as u (u.code)}<option value={u.code}>{u.code}</option>{/each}
  </select>
  <select bind:value={status} onchange={applyFilters}>
    <option value="">E-mail: todos</option>
    <option value="real">Só com e-mail real</option>
    <option value="placeholder">Só placeholder</option>
  </select>
  <input type="search" bind:value={q} placeholder="Buscar nome / município…"
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
      <thead><tr><th>Nome</th><th>Cargo</th><th>UF/Município</th><th>Partido</th><th>E-mail</th></tr></thead>
      <tbody>
        {#if loading}
          <tr><td colspan="5" class="muted center">Carregando…</td></tr>
        {:else if items.length === 0}
          <tr><td colspan="5" class="muted center">Nenhum resultado.</td></tr>
        {:else}
          {#each items as p (p.id)}
            <tr>
              <td>{p.display_name}</td>
              <td class="small">{p.office}</td>
              <td class="small">{p.uf ?? '—'}{p.municipio ? ' · ' + p.municipio : ''}</td>
              <td class="small">{p.party ?? '—'}</td>
              <td class="email">
                {#if p.email_real}
                  <span class="dot real"></span>{p.public_email}
                {:else}
                  <span class="dot ph"></span><span class="muted">placeholder (não entrega)</span>
                {/if}
              </td>
            </tr>
          {/each}
        {/if}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .funnel { display: flex; gap: 0.7rem; flex-wrap: wrap; margin-bottom: 1.2rem; }
  .stat { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 10px; padding: 0.7rem 1rem; display: grid; gap: 0.1rem; min-width: 10rem; }
  .cargo { font-weight: 700; }
  .nums { font-size: 0.9rem; }
  .real { color: var(--c-green-dark, #15803d); }
  .ph { color: #b45309; }
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
  .center { text-align: center; padding: 1.5rem; }
  .email { font-family: ui-monospace, monospace; font-size: 0.85rem; }
  .dot { display: inline-block; width: 0.55rem; height: 0.55rem; border-radius: 50%; margin-right: 0.4rem; vertical-align: middle; }
  .dot.real { background: #22c55e; }
  .dot.ph { background: #f59e0b; }
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1.5rem; }
  .err { color: #dc2626; }
</style>
