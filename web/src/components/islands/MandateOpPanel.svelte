<script lang="ts">
  // Aba "Orçamento participativo" do painel do mandato (D8.3) — a superfície do OPERADOR.
  // Cria rodadas (verba de emenda), avança as fases do ciclo e presta contas marcando o
  // status de execução de cada item. Copy honesta: "piloto — verba de emenda do mandato".
  //
  // Extraído do MandatePanel pra manter cada ilha coesa e abaixo do teto de tamanho.
  import { onMount } from 'svelte';
  import {
    createOpRound,
    advanceOpPhase,
    markOpExecution,
    getMandateOpRounds,
    getOpRound,
  } from '../../lib/api';
  import type {
    OpRoundDto,
    OpRoundSummaryDto,
    OpPhase,
    OpExecutionStatus,
  } from '../../lib/types';

  let { mandateId }: { mandateId: string } = $props();

  let loading = $state(true);
  let rounds = $state<OpRoundSummaryDto[]>([]);
  let loadError = $state<string | null>(null);

  // Criar rodada.
  let newTitle = $state('');
  let newBudget = $state(''); // reais
  let newUf = $state('');
  let creating = $state(false);
  let createMsg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Rodada expandida (gestão de itens/execução).
  let openRound = $state<string | null>(null);
  let detail = $state<OpRoundDto | null>(null);
  let detailLoading = $state(false);
  let actionMsg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  const brl = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });
  const money = (cents: number | null | undefined) =>
    cents == null ? '—' : brl.format(cents / 100);

  const PHASES: OpPhase[] = ['propostas', 'votacao', 'resultado', 'execucao'];
  const PHASE_LABEL: Record<OpPhase, string> = {
    propostas: 'Propostas',
    votacao: 'Votação',
    resultado: 'Resultado',
    execucao: 'Execução',
  };
  const NEXT_PHASE: Partial<Record<OpPhase, OpPhase>> = {
    propostas: 'votacao',
    votacao: 'resultado',
    resultado: 'execucao',
  };
  const EXEC_OPTS: { value: OpExecutionStatus; label: string }[] = [
    { value: 'previsto', label: 'Previsto' },
    { value: 'em_andamento', label: 'Em andamento' },
    { value: 'concluido', label: 'Concluído' },
    { value: 'nao_executado', label: 'Não executado' },
  ];

  function parseCents(v: string): number | null {
    const cleaned = v.replace(/\./g, '').replace(',', '.').trim();
    if (!cleaned) return null;
    const n = Number(cleaned);
    if (!Number.isFinite(n) || n <= 0) return null;
    return Math.round(n * 100);
  }

  async function loadRounds() {
    loading = true;
    const res = await getMandateOpRounds(mandateId);
    loading = false;
    if (res.success && res.data) {
      rounds = res.data.rounds;
    } else {
      loadError = res.error?.message ?? 'Não foi possível carregar as rodadas.';
    }
  }

  async function create(e: Event) {
    e.preventDefault();
    if (creating) return;
    const title = newTitle.trim();
    if (title.length < 3) {
      createMsg = { kind: 'error', text: 'Dê um título à rodada.' };
      return;
    }
    const cents = parseCents(newBudget);
    if (cents == null) {
      createMsg = { kind: 'error', text: 'Informe a verba (em reais, maior que zero).' };
      return;
    }
    creating = true;
    createMsg = null;
    const res = await createOpRound({
      title,
      budget_cents: cents,
      uf: newUf.trim() ? newUf.trim().toUpperCase() : undefined,
    });
    creating = false;
    if (res.success) {
      newTitle = '';
      newBudget = '';
      newUf = '';
      createMsg = { kind: 'ok', text: 'Rodada criada na fase de propostas.' };
      await loadRounds();
    } else {
      createMsg = { kind: 'error', text: res.error?.message ?? 'Não foi possível criar a rodada.' };
    }
  }

  async function toggle(id: string) {
    actionMsg = null;
    if (openRound === id) {
      openRound = null;
      detail = null;
      return;
    }
    openRound = id;
    detail = null;
    detailLoading = true;
    const res = await getOpRound(id);
    detailLoading = false;
    if (res.success && res.data) detail = res.data;
  }

  async function advance(id: string, phase: OpPhase) {
    actionMsg = null;
    const res = await advanceOpPhase(id, phase);
    if (res.success) {
      actionMsg = { kind: 'ok', text: `Fase avançada para "${PHASE_LABEL[phase]}".` };
      await loadRounds();
      if (openRound === id) {
        const d = await getOpRound(id);
        if (d.success && d.data) detail = d.data;
      }
    } else {
      actionMsg = { kind: 'error', text: res.error?.message ?? 'Não foi possível avançar a fase.' };
    }
  }

  async function setExecution(itemId: string, status: OpExecutionStatus) {
    if (!detail) return;
    actionMsg = null;
    const res = await markOpExecution(detail.id, itemId, status);
    if (res.success) {
      actionMsg = { kind: 'ok', text: 'Status de execução atualizado.' };
      const d = await getOpRound(detail.id);
      if (d.success && d.data) detail = d.data;
    } else {
      actionMsg = { kind: 'error', text: res.error?.message ?? 'Não foi possível atualizar.' };
    }
  }

  onMount(loadRounds);
</script>

<section class="op">
  <p class="intro muted">
    🧪 <strong>Piloto — verba de emenda do mandato.</strong> A base decide onde vai uma fatia
    real da verba. Abra uma rodada, colha propostas, leve à votação e preste contas marcando a
    execução de cada item. Inspirado no Orçamento Participativo de Porto Alegre.
  </p>

  <details class="create" open={rounds.length === 0}>
    <summary>Criar nova rodada</summary>
    <form class="form" onsubmit={create}>
      <label for="op-new-title">Título</label>
      <input id="op-new-title" class="input" bind:value={newTitle} maxlength="160"
        placeholder="Ex.: Emenda 2026 — bairro Centro" />
      <label for="op-new-budget">Verba em reais</label>
      <input id="op-new-budget" class="input" bind:value={newBudget} inputmode="decimal"
        placeholder="Ex.: 500000,00" />
      <label for="op-new-uf">UF (opcional)</label>
      <input id="op-new-uf" class="input uf" bind:value={newUf} maxlength="2" placeholder="SP" />
      <button class="btn btn-primary" type="submit" disabled={creating}>
        {creating ? 'Criando…' : 'Criar rodada'}
      </button>
      {#if createMsg}
        <p class={`hint ${createMsg.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}>{createMsg.text}</p>
      {/if}
    </form>
  </details>

  {#if actionMsg}
    <p class={`hint ${actionMsg.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}>{actionMsg.text}</p>
  {/if}

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if loadError}
    <p class="hint hint-error" role="alert">{loadError}</p>
  {:else if rounds.length === 0}
    <p class="muted">Nenhuma rodada ainda. Crie a primeira acima.</p>
  {:else}
    <ul class="rounds">
      {#each rounds as r (r.id)}
        <li class="round">
          <div class="round-head">
            <button class="round-toggle" onclick={() => toggle(r.id)} aria-expanded={openRound === r.id}>
              <strong>{r.title}</strong>
              <span class="muted small">
                {money(r.budget_cents)} · {r.items_count} propostas · {r.total_votes} votos
              </span>
            </button>
            <span class={`badge phase-${r.phase}`}>{PHASE_LABEL[r.phase]}</span>
          </div>

          {#if openRound === r.id}
            <div class="round-body">
              <div class="phase-nav">
                {#each PHASES as p (p)}
                  <span class={`step ${r.phase === p ? 'now' : ''}`}>{PHASE_LABEL[p]}</span>
                {/each}
              </div>
              <div class="actions">
                {#if NEXT_PHASE[r.phase]}
                  <button class="btn btn-primary btn-sm" onclick={() => advance(r.id, NEXT_PHASE[r.phase]!)}>
                    Avançar para {PHASE_LABEL[NEXT_PHASE[r.phase]!]}
                  </button>
                {:else}
                  <span class="muted small">Ciclo concluído.</span>
                {/if}
                <a class="btn btn-ghost btn-sm" href={`/op/${r.id}`}>Ver página pública</a>
              </div>

              {#if detailLoading}
                <p class="muted">Carregando itens…</p>
              {:else if detail && detail.id === r.id}
                {#if detail.items.length === 0}
                  <p class="muted small">Nenhuma proposta submetida ainda.</p>
                {:else}
                  <table class="items">
                    <thead>
                      <tr><th>#</th><th>Proposta</th><th>Votos</th><th>Custo</th><th>Execução</th></tr>
                    </thead>
                    <tbody>
                      {#each [...detail.items].sort((a, b) => a.rank - b.rank) as it (it.id)}
                        <tr class={it.fits_budget ? 'wins' : ''}>
                          <td>{it.rank}</td>
                          <td>
                            {it.title}
                            {#if it.fits_budget}<span class="win">✔ cabe</span>{/if}
                          </td>
                          <td class="num">{it.votes}</td>
                          <td class="num">{money(it.estimated_cents)}</td>
                          <td>
                            {#if r.phase === 'execucao' || r.phase === 'resultado'}
                              <select class="input exec"
                                value={it.execution_status ?? ''}
                                onchange={(e) => setExecution(it.id, (e.currentTarget as HTMLSelectElement).value as OpExecutionStatus)}>
                                <option value="" disabled>—</option>
                                {#each EXEC_OPTS as o (o.value)}
                                  <option value={o.value}>{o.label}</option>
                                {/each}
                              </select>
                            {:else}
                              <span class="muted small">após votação</span>
                            {/if}
                          </td>
                        </tr>
                      {/each}
                    </tbody>
                  </table>
                {/if}
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<style>
  .intro {
    background: #fff7e6; border: 1px solid #ffd591; border-radius: 10px;
    padding: 0.75rem 1rem; margin: 0 0 1.25rem; font-size: 0.95rem;
  }
  .create { margin: 0 0 1.25rem; border: 1px solid var(--c-border); border-radius: 10px; padding: 0.75rem 1rem; }
  .create summary { font-weight: 600; cursor: pointer; }
  .form { display: grid; gap: 0.5rem; max-width: 30rem; margin-top: 0.75rem; }
  .form label { font-weight: 600; font-size: 0.9rem; }
  .uf { max-width: 6rem; text-transform: uppercase; }
  .rounds { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.75rem; }
  .round { border: 1px solid var(--c-border); border-radius: 12px; background: var(--c-paper); overflow: hidden; }
  .round-head { display: flex; align-items: center; gap: 0.75rem; padding: 0.85rem 1rem; }
  .round-toggle {
    flex: 1; display: grid; gap: 2px; text-align: left; background: none; border: none;
    font: inherit; cursor: pointer; padding: 0;
  }
  .round-body { border-top: 1px solid var(--c-border); padding: 1rem; }
  .phase-nav { display: flex; flex-wrap: wrap; gap: 0.4rem; margin-bottom: 0.85rem; }
  .step {
    font-size: 0.8rem; padding: 0.2rem 0.6rem; border-radius: 999px;
    background: var(--c-bg, #f2f4f7); color: var(--c-text-muted);
  }
  .step.now { background: var(--c-navy); color: #fff; font-weight: 700; }
  .actions { display: flex; gap: 0.5rem; flex-wrap: wrap; margin-bottom: 1rem; align-items: center; }
  .btn-sm { padding: 0.4rem 0.85rem; font-size: 0.9rem; }
  .items { width: 100%; border-collapse: collapse; font-size: 0.9rem; }
  .items th, .items td { text-align: left; padding: 0.45rem 0.6rem; border-bottom: 1px solid var(--c-border); }
  .items th { font-size: 0.75rem; text-transform: uppercase; color: var(--c-text-muted); }
  .items .num { text-align: right; font-variant-numeric: tabular-nums; }
  .items tr.wins td { background: #f6ffed; }
  .win { margin-left: 0.4rem; color: #389e0d; font-size: 0.78rem; }
  .exec { padding: 0.25rem 0.4rem; font-size: 0.85rem; }
  .badge { display: inline-block; padding: 0.15rem 0.55rem; border-radius: 999px; font-size: 0.78rem; white-space: nowrap; }
  .phase-propostas { background: #e6f7ff; color: #096dd9; }
  .phase-votacao { background: #fff7e6; color: #ad6800; }
  .phase-resultado, .phase-execucao { background: #f6ffed; color: #389e0d; }
  .hint-ok { color: #389e0d; }
  .hint-error { color: #cf1322; }
  .small { font-size: 0.85rem; }
  @media (max-width: 560px) {
    .items thead { display: none; }
    .items, .items tbody, .items tr, .items td { display: block; width: 100%; }
    .items td { border-bottom: none; padding: 0.2rem 0.6rem; }
    .items tr { border-bottom: 1px solid var(--c-border); padding: 0.5rem 0; }
    .items .num { text-align: left; }
  }
</style>
