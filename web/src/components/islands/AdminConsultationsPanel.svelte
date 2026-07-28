<script lang="ts">
  // Painel admin de consultas (consultations_consultation). Lista paginada com nº de perguntas e
  // respostas, detalhe expansível com o agregado por pergunta, e ação de encerrar (status→closed).
  import { onMount } from 'svelte';
  import {
    getAdminConsultations,
    getAdminConsultation,
    closeAdminConsultation,
    type AdminConsultation,
    type AdminConsultationDetail,
  } from '../../lib/api';

  const PAGE = 50;
  let items = $state<AdminConsultation[]>([]);
  let total = $state(0);
  let offset = $state(0);
  let loading = $state(true);
  let error = $state<string | null>(null);

  let status = $state('');
  let q = $state('');

  // Detalhe carregado sob demanda por linha expandida.
  let expandedId = $state<string | null>(null);
  let detail = $state<AdminConsultationDetail | null>(null);
  let detailLoading = $state(false);
  let closingId = $state<string | null>(null);

  async function loadPage() {
    loading = true;
    error = null;
    const res = await getAdminConsultations({ status, q, limit: PAGE, offset });
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
    expandedId = null;
    detail = null;
    loadPage();
  }
  function next() {
    if (offset + PAGE < total) { offset += PAGE; expandedId = null; detail = null; loadPage(); }
  }
  function prev() {
    if (offset > 0) { offset = Math.max(0, offset - PAGE); expandedId = null; detail = null; loadPage(); }
  }

  async function toggle(id: string) {
    if (expandedId === id) {
      expandedId = null;
      detail = null;
      return;
    }
    expandedId = id;
    detail = null;
    detailLoading = true;
    const res = await getAdminConsultation(id);
    detailLoading = false;
    if (res.success && res.data) detail = res.data;
    else error = res.error?.message ?? 'Não foi possível carregar o detalhe.';
  }

  async function close(id: string) {
    if (!confirm('Encerrar esta consulta? Não é possível reabrir.')) return;
    closingId = id;
    const res = await closeAdminConsultation(id);
    closingId = null;
    if (res.success && res.data) {
      // Atualiza a linha in-place (imutável) e o detalhe aberto, se for o mesmo.
      items = items.map((c) => (c.id === id ? { ...c, status: res.data!.status } : c));
      if (detail && detail.id === id) detail = { ...detail, status: res.data.status };
    } else {
      error = res.error?.message ?? 'Não foi possível encerrar.';
    }
  }

  function fmtDate(s: string | null): string {
    if (!s) return '—';
    return new Date(s).toLocaleDateString('pt-BR');
  }
  const STATUS_LABEL: Record<string, string> = { open: 'Aberta', closed: 'Fechada' };

  onMount(loadPage);

  const from = $derived(total === 0 ? 0 : offset + 1);
  const to = $derived(Math.min(offset + PAGE, total));
</script>

<div class="filters">
  <select bind:value={status} onchange={applyFilters}>
    <option value="">Status: todos</option>
    <option value="open">Abertas</option>
    <option value="closed">Fechadas</option>
  </select>
  <input type="search" bind:value={q} placeholder="Buscar por título…"
    onkeydown={(e) => e.key === 'Enter' && applyFilters()} />
  <button class="btn" onclick={applyFilters}>Filtrar</button>
</div>

{#if error}
  <div class="card err" role="alert">{error}</div>
{/if}

<div class="pager">
  <span class="muted">{from.toLocaleString('pt-BR')}–{to.toLocaleString('pt-BR')} de {total.toLocaleString('pt-BR')}</span>
  <span class="pgbtns">
    <button class="btn ghost" onclick={prev} disabled={offset === 0 || loading}>‹ Anterior</button>
    <button class="btn ghost" onclick={next} disabled={offset + PAGE >= total || loading}>Próxima ›</button>
  </span>
</div>

<div class="table-wrap">
  <table>
    <thead>
      <tr>
        <th>Título</th><th>Status</th><th>Abre</th><th>Fecha</th>
        <th class="center">Perguntas</th><th class="center">Respostas</th><th></th>
      </tr>
    </thead>
    <tbody>
      {#if loading}
        <tr><td colspan="7" class="muted center">Carregando…</td></tr>
      {:else if items.length === 0}
        <tr><td colspan="7" class="muted center">Nenhuma consulta.</td></tr>
      {:else}
        {#each items as c (c.id)}
          <tr class="row" class:open={expandedId === c.id}>
            <td>
              <button class="linklike" onclick={() => toggle(c.id)}>{c.title}</button>
            </td>
            <td class="small"><span class="badge {c.status}">{STATUS_LABEL[c.status] ?? c.status}</span></td>
            <td class="small">{fmtDate(c.opens_at)}</td>
            <td class="small">{fmtDate(c.closes_at)}</td>
            <td class="small center">{c.question_count.toLocaleString('pt-BR')}</td>
            <td class="small center">{c.response_count.toLocaleString('pt-BR')}</td>
            <td class="small right">
              {#if c.status === 'open'}
                <button class="btn ghost sm" onclick={() => close(c.id)} disabled={closingId === c.id}>
                  {closingId === c.id ? 'Encerrando…' : 'Encerrar'}
                </button>
              {:else}
                <span class="muted small">—</span>
              {/if}
            </td>
          </tr>
          {#if expandedId === c.id}
            <tr class="detail-row">
              <td colspan="7">
                {#if detailLoading}
                  <span class="muted small">Carregando detalhe…</span>
                {:else if detail}
                  {#if detail.questions.length === 0}
                    <span class="muted small">Sem perguntas nesta consulta.</span>
                  {:else}
                    <ol class="qs">
                      {#each detail.questions as qd (qd.id)}
                        <li>
                          <div class="prompt">{qd.prompt}</div>
                          <div class="tally small">
                            <span class="t concordo">Concordo {qd.concordo}</span>
                            <span class="t neutro">Neutro {qd.neutro}</span>
                            <span class="t discordo">Discordo {qd.discordo}</span>
                            <span class="muted">· total {qd.total}</span>
                          </div>
                        </li>
                      {/each}
                    </ol>
                  {/if}
                {/if}
              </td>
            </tr>
          {/if}
        {/each}
      {/if}
    </tbody>
  </table>
</div>

<style>
  .filters { display: flex; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 1rem; }
  .filters select, .filters input { padding: 0.5rem 0.6rem; border-radius: 8px; border: 1px solid var(--border-subtle, #cbd5e1); background: var(--surface-1, #fff); color: inherit; font: inherit; }
  .filters input { flex: 1; min-width: 12rem; }
  .btn { padding: 0.5rem 1rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .btn.ghost { border-color: var(--border-subtle, #cbd5e1); }
  .btn.sm { padding: 0.3rem 0.6rem; font-size: 0.82rem; }
  .btn:disabled { opacity: 0.5; cursor: not-allowed; }
  .pager { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.6rem; }
  .pgbtns { display: flex; gap: 0.5rem; }
  .table-wrap { overflow-x: auto; border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; }
  table { width: 100%; border-collapse: collapse; font-size: 0.92rem; }
  th, td { text-align: left; padding: 0.55rem 0.8rem; border-bottom: 1px solid var(--border-subtle, rgba(0,0,0,0.06)); }
  th { font-size: 0.75rem; text-transform: uppercase; letter-spacing: 0.03em; color: var(--muted, #64748b); }
  .small { font-size: 0.82rem; }
  .center { text-align: center; }
  .right { text-align: right; }
  .row.open { background: var(--surface-2, #f8fafc); }
  .linklike { background: none; border: none; padding: 0; color: var(--c-blue, #1d4ed8); font: inherit; font-weight: 600; cursor: pointer; text-align: left; }
  .linklike:hover { text-decoration: underline; }
  .badge { padding: 0.1rem 0.5rem; border-radius: 999px; font-size: 0.75rem; font-weight: 600; }
  .badge.open { background: #dcfce7; color: #15803d; }
  .badge.closed { background: #e2e8f0; color: #475569; }
  .detail-row td { background: var(--surface-2, #f8fafc); }
  .qs { margin: 0; padding-left: 1.2rem; display: grid; gap: 0.6rem; }
  .prompt { font-weight: 600; }
  .tally { display: flex; gap: 0.8rem; flex-wrap: wrap; margin-top: 0.2rem; }
  .t { font-weight: 600; }
  .t.concordo { color: #15803d; }
  .t.neutro { color: #b45309; }
  .t.discordo { color: #b91c1c; }
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; padding: 1rem; margin-bottom: 1rem; }
  .err { color: #dc2626; }
  .muted { color: var(--muted, #64748b); }
</style>
