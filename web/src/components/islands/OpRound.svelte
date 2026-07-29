<script lang="ts">
  // Página PÚBLICA de uma rodada de orçamento participativo — o piloto de MANDATO (D8.3).
  // O salto de "medir raiva" para "exercer poder": a base decide onde vai uma fatia REAL de
  // verba (emenda do mandato). Copy honesta em todo lugar: "piloto — verba de emenda do mandato".
  //
  // A ilha reflete a FASE da rodada:
  //   propostas → propor itens; votacao → votar (barras ao vivo); resultado/execucao →
  //   ranking dentro do orçamento + prestação de contas (status de execução).
  import { onMount } from 'svelte';
  import { getOpRound, submitOpItem, castOpVote } from '../../lib/api';
  import type { OpRoundDto, OpItemDto, OpExecutionStatus } from '../../lib/types';

  // O id vem do param da rota (prop) OU do ?round= (fallback SSG).
  let { roundId = '' }: { roundId?: string } = $props();

  let ready = $state(false);
  let round = $state<OpRoundDto | null>(null);
  let loadError = $state<string | null>(null);

  // Propor item (fase propostas).
  let itemTitle = $state('');
  let itemDesc = $state('');
  let itemCost = $state(''); // reais, string; convertido p/ centavos no envio.
  let submitting = $state(false);
  let submitMsg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  // Votar (fase votacao).
  let voting = $state<string | null>(null);
  let votedItem = $state<string | null>(null);
  let voteMsg = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  const brl = new Intl.NumberFormat('pt-BR', { style: 'currency', currency: 'BRL' });
  const money = (cents: number | null | undefined) =>
    cents == null ? '—' : brl.format(cents / 100);

  const PHASE_LABEL: Record<OpRoundDto['phase'], string> = {
    propostas: 'Propostas abertas',
    votacao: 'Votação aberta',
    resultado: 'Resultado',
    execucao: 'Em execução',
  };

  const EXEC_LABEL: Record<OpExecutionStatus, string> = {
    previsto: '📋 Previsto',
    em_andamento: '🚧 Em andamento',
    concluido: '✅ Concluído',
    nao_executado: '⛔ Não executado',
  };

  // Ranking pré-ordenado por votos (o backend já devolve rank + fits_budget).
  const ranked = $derived(
    [...(round?.items ?? [])].sort((a, b) => a.rank - b.rank),
  );
  const maxVotes = $derived(Math.max(1, ...(round?.items ?? []).map((i) => i.votes)));
  const budgetPct = $derived(
    round && round.budget_cents > 0
      ? Math.min(100, Math.round((round.allocated_cents / round.budget_cents) * 100))
      : 0,
  );

  function resolveId(): string {
    if (roundId) return roundId;
    if (typeof window !== 'undefined') {
      return new URLSearchParams(window.location.search).get('round') ?? '';
    }
    return '';
  }

  async function load() {
    const id = resolveId();
    if (!id) {
      loadError = 'Rodada não informada.';
      ready = true;
      return;
    }
    const res = await getOpRound(id);
    ready = true;
    if (res.success && res.data) {
      round = res.data;
      votedItem = null;
    } else {
      loadError = res.error?.message ?? 'Não foi possível carregar a rodada.';
    }
  }

  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString('pt-BR');
    } catch {
      return iso;
    }
  }

  function parseCents(v: string): number | null {
    const cleaned = v.replace(/\./g, '').replace(',', '.').trim();
    if (!cleaned) return null;
    const n = Number(cleaned);
    if (!Number.isFinite(n) || n < 0) return null;
    return Math.round(n * 100);
  }

  async function submit(e: Event) {
    e.preventDefault();
    if (!round || submitting) return;
    const title = itemTitle.trim();
    if (title.length < 3) {
      submitMsg = { kind: 'error', text: 'Dê um título à sua proposta (mín. 3 caracteres).' };
      return;
    }
    const cents = parseCents(itemCost);
    if (itemCost.trim() && cents == null) {
      submitMsg = { kind: 'error', text: 'Custo estimado inválido.' };
      return;
    }
    submitting = true;
    submitMsg = null;
    const res = await submitOpItem(round.id, {
      title,
      description: itemDesc.trim() || undefined,
      estimated_cents: cents ?? undefined,
    });
    submitting = false;
    if (res.success) {
      itemTitle = '';
      itemDesc = '';
      itemCost = '';
      submitMsg = { kind: 'ok', text: 'Proposta registrada!' };
      await load();
    } else {
      submitMsg = {
        kind: 'error',
        text:
          res.error?.code === 'http_401'
            ? 'Entre na sua conta para propor.'
            : res.error?.message ?? 'Não foi possível registrar a proposta.',
      };
    }
  }

  async function vote(item: OpItemDto) {
    if (!round || voting) return;
    voting = item.id;
    voteMsg = null;
    const res = await castOpVote(round.id, item.id);
    voting = null;
    if (res.success) {
      voteMsg = { kind: 'ok', text: 'Voto registrado. Você pode trocar até o fim da votação.' };
      await load();
      votedItem = item.id;
    } else {
      voteMsg = {
        kind: 'error',
        text:
          res.error?.code === 'http_401'
            ? 'Entre na sua conta para votar.'
            : res.error?.message ?? 'Não foi possível votar.',
      };
    }
  }

  onMount(load);
</script>

{#if !ready}
  <p class="muted">Carregando…</p>
{:else if loadError || !round}
  <div class="card center">
    <h1>Rodada não encontrada</h1>
    <p class="muted">{loadError ?? 'Esta rodada de orçamento participativo não existe.'}</p>
    <p class="muted"><a href="/politicos">Ver parlamentares</a></p>
  </div>
{:else}
  <header class="head">
    <span class={`badge phase phase-${round.phase}`}>{PHASE_LABEL[round.phase]}</span>
    <h1>{round.title}</h1>
    <p class="pilot">
      🧪 <strong>Piloto</strong> — verba de emenda do mandato
      {#if round.mandate_name}de <strong>{round.mandate_name}</strong>{/if}.
      A base decide onde vai uma fatia real da verba.
    </p>
    <div class="stat-row">
      <div class="stat">
        <span class="stat-num">{money(round.budget_cents)}</span>
        <span class="stat-lbl">verba da rodada</span>
      </div>
      <div class="stat">
        <span class="stat-num">{round.items.length}</span>
        <span class="stat-lbl">propostas</span>
      </div>
      <div class="stat">
        <span class="stat-num">{round.total_votes}</span>
        <span class="stat-lbl">votos</span>
      </div>
    </div>
    {#if round.uf}
      <p class="muted small">Escopo territorial: {round.uf}</p>
    {/if}
  </header>

  <!-- FASE PROPOSTAS: propor + ver propostas -->
  {#if round.phase === 'propostas'}
    <section>
      <h2>Proponha uma ideia para a verba</h2>
      <form class="form" onsubmit={submit}>
        <label for="op-title">Título</label>
        <input id="op-title" class="input" bind:value={itemTitle} maxlength="160"
          placeholder="Ex.: Reforma da quadra do bairro" />
        <label for="op-desc">Descrição (opcional)</label>
        <textarea id="op-desc" class="input" rows="3" bind:value={itemDesc} maxlength="2000"
          placeholder="Detalhe a proposta."></textarea>
        <label for="op-cost">Custo estimado em reais (opcional)</label>
        <input id="op-cost" class="input" bind:value={itemCost} inputmode="decimal"
          placeholder="Ex.: 15000,00" />
        <button class="btn btn-primary" type="submit" disabled={submitting}>
          {submitting ? 'Enviando…' : 'Enviar proposta'}
        </button>
        {#if submitMsg}
          <p class={`hint ${submitMsg.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}>{submitMsg.text}</p>
        {/if}
      </form>
    </section>
  {/if}

  <!-- FASE VOTACAO: votar com barras ao vivo -->
  {#if round.phase === 'votacao'}
    <section>
      <h2>Vote na proposta que a verba deve financiar</h2>
      <p class="muted small">Um voto por rodada — você pode trocar até a votação fechar.</p>
      {#if voteMsg}
        <p class={`hint ${voteMsg.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}>{voteMsg.text}</p>
      {/if}
    </section>
  {/if}

  <!-- LISTA DE ITENS (todas as fases) -->
  {#if round.items.length === 0}
    <p class="muted">Nenhuma proposta ainda.</p>
  {:else}
    <ul class="items">
      {#each ranked as item (item.id)}
        <li class={`item ${round.phase !== 'propostas' && item.fits_budget ? 'wins' : ''}`}>
          <div class="item-head">
            {#if round.phase === 'resultado' || round.phase === 'execucao'}
              <span class="rank">#{item.rank}</span>
            {/if}
            <div class="item-body">
              <strong>{item.title}</strong>
              {#if item.description}<p class="muted small desc">{item.description}</p>{/if}
              <p class="muted small">
                {#if item.author_handle}
                  por @{item.author_handle}
                {:else}
                  proposta do gabinete
                {/if}
                · custo estimado {money(item.estimated_cents)}
                · {fmtDate(item.created_at)}
              </p>
            </div>
            {#if round.phase === 'votacao'}
              <button class="btn btn-primary btn-sm vote-btn"
                onclick={() => vote(item)} disabled={voting === item.id}>
                {votedItem === item.id ? '✓ Meu voto' : voting === item.id ? '…' : 'Votar'}
              </button>
            {/if}
          </div>

          {#if round.phase !== 'propostas'}
            <div class="bar-row">
              <div class="bar" style={`width:${Math.round((item.votes / maxVotes) * 100)}%`}></div>
              <span class="bar-lbl">{item.votes} voto{item.votes === 1 ? '' : 's'}</span>
            </div>
          {/if}

          {#if (round.phase === 'resultado' || round.phase === 'execucao')}
            <div class="tags">
              {#if item.fits_budget}
                <span class="badge win-tag">✔ cabe na verba</span>
              {:else}
                <span class="badge out-tag">fora da verba</span>
              {/if}
              {#if item.execution_status}
                <span class="badge exec-tag">{EXEC_LABEL[item.execution_status]}</span>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  <!-- RANKING/ORÇAMENTO: resumo do conjunto vencedor -->
  {#if round.phase === 'resultado' || round.phase === 'execucao'}
    <section class="alloc">
      <h2>O que cabe na verba</h2>
      <p class="muted small">
        Por ordem de votos, os itens que cabem em {money(round.budget_cents)} formam o
        conjunto "vencedor". Prestação de contas: cada item ganha um status de execução.
      </p>
      <div class="bar-row big">
        <div class="bar alloc-bar" style={`width:${budgetPct}%`}></div>
        <span class="bar-lbl">
          {money(round.allocated_cents)} de {money(round.budget_cents)} alocados
        </span>
      </div>
    </section>
  {/if}

  <p class="muted small foot">
    Inspirado no Orçamento Participativo de Porto Alegre. Piloto de mandato — não substitui o
    orçamento público oficial.
  </p>
{/if}

<style>
  .head { margin-bottom: 2rem; }
  .head h1 { margin: 0.5rem 0; font-size: 1.6rem; }
  .pilot {
    background: #fff7e6; border: 1px solid #ffd591; border-radius: 10px;
    padding: 0.75rem 1rem; margin: 0 0 1rem; font-size: 0.95rem;
  }
  .stat-row { display: flex; gap: 1rem; flex-wrap: wrap; }
  .stat {
    flex: 1; min-width: 8rem; text-align: center;
    background: var(--c-bg, #f2f4f7); border: 1px solid var(--c-border);
    border-radius: 10px; padding: 0.75rem;
  }
  .stat-num { display: block; font-size: 1.35rem; font-weight: 700; font-variant-numeric: tabular-nums; }
  .stat-lbl { display: block; font-size: 0.8rem; color: var(--c-text-muted); }
  section { margin-bottom: 2rem; }
  section h2 { font-size: 1.1rem; margin: 0 0 0.75rem; }
  .form { display: grid; gap: 0.5rem; max-width: 34rem; }
  .form label { font-weight: 600; font-size: 0.9rem; }
  .items { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.85rem; }
  .item {
    background: var(--c-paper); border: 1px solid var(--c-border);
    border-radius: 12px; padding: 1rem;
  }
  .item.wins { border-color: #52c41a; box-shadow: 0 0 0 1px #52c41a33; }
  .item-head { display: flex; gap: 0.85rem; align-items: flex-start; }
  .rank { font-size: 1.1rem; font-weight: 800; color: var(--c-navy); min-width: 2rem; }
  .item-body { flex: 1; min-width: 0; }
  .desc { margin: 0.25rem 0; }
  .vote-btn { white-space: nowrap; }
  .btn-sm { padding: 0.4rem 0.85rem; font-size: 0.9rem; }
  .bar-row { display: flex; align-items: center; gap: 0.5rem; margin-top: 0.6rem; }
  .bar {
    height: 10px; border-radius: 999px; background: var(--c-navy);
    min-width: 2px; transition: width 0.4s ease;
  }
  .bar-row.big .bar { height: 16px; }
  .alloc-bar { background: #52c41a; }
  .bar-lbl { font-size: 0.82rem; color: var(--c-text-muted); white-space: nowrap; }
  .tags { display: flex; gap: 0.4rem; flex-wrap: wrap; margin-top: 0.6rem; }
  .badge {
    display: inline-block; padding: 0.15rem 0.55rem; border-radius: 999px;
    font-size: 0.78rem; white-space: nowrap;
  }
  .phase { font-weight: 700; }
  .phase-propostas { background: #e6f7ff; color: #096dd9; }
  .phase-votacao { background: #fff7e6; color: #ad6800; }
  .phase-resultado, .phase-execucao { background: #f6ffed; color: #389e0d; }
  .win-tag { background: #f6ffed; color: #389e0d; }
  .out-tag { background: #f2f4f7; color: var(--c-text-muted); }
  .exec-tag { background: #f0f5ff; color: #1d39c4; }
  .hint-ok { color: var(--c-green-dark, #389e0d); }
  .hint-error { color: #cf1322; }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .foot { margin-top: 2rem; border-top: 1px solid var(--c-border); padding-top: 1rem; }
  .small { font-size: 0.85rem; }
</style>
