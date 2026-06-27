<script lang="ts">
  // Página individual do parlamentar — CSR (renderiza pra qualquer id existente, sem rebuild).
  // Mostra foto, partido/UF/casa, scorecard quando há, propostas dirigidas + SLAs pendentes.
  // Botão proeminente "Propor demanda a Fulano" leva pra /propor?mandate=<id> com pré-seleção.
  import { onMount } from 'svelte';
  import {
    getMandate,
    getProposals,
    getScorecard,
    getSlas,
    DEFAULT_ORG_ID,
    type MandateDto,
  } from '../../lib/api';
  import type { ProposalDto, ScorecardDto, SlaDto } from '../../lib/types';
  import { responseRate, formatLatency } from '../../lib/format';

  let { mandateId }: { mandateId: string } = $props();

  let loading = $state(true);
  let mandate = $state<MandateDto | null>(null);
  let scorecard = $state<ScorecardDto | null>(null);
  let myProposals = $state<ProposalDto[]>([]);
  let mySlas = $state<SlaDto[]>([]);
  let loadError = $state<string | null>(null);

  let pendingSlas = $derived(mySlas.filter((s) => s.status === 'pending'));

  onMount(async () => {
    const [mr, scr, pr, slr] = await Promise.all([
      getMandate(mandateId),
      getScorecard(mandateId),
      getProposals(DEFAULT_ORG_ID, 200),
      getSlas(DEFAULT_ORG_ID, 200),
    ]);
    loading = false;
    if (!mr.ok || !mr.data) {
      loadError =
        mr.error?.includes('encontrado') || mr.error?.includes('not found')
          ? 'Político não encontrado.'
          : (mr.error ?? 'Não foi possível carregar o político.');
      return;
    }
    mandate = mr.data;
    if (scr.ok && scr.data) scorecard = scr.data;
    if (pr.ok && pr.data) {
      myProposals = pr.data.filter((p) => p.mandate_id === mandateId);
    }
    if (slr.ok && slr.data) {
      mySlas = slr.data.filter((s) => s.mandate_id === mandateId);
    }
  });

  function badgeRate(s: ScorecardDto | null) {
    if (!s) return null;
    return responseRate(s.answered, s.ignored);
  }

  function formatDateTime(iso: string): string {
    try {
      return new Date(iso).toLocaleString('pt-BR', {
        day: '2-digit',
        month: 'short',
        year: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return iso;
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <div class="card center" role="alert">
    <h2>{loadError}</h2>
    <p class="muted">
      <a href="/politicos">Voltar para a lista</a>
    </p>
  </div>
{:else if mandate}
  <header class="head">
    {#if mandate.avatar_url}
      <img class="avatar-lg" src={mandate.avatar_url} alt="" />
    {:else}
      <span class="avatar-lg avatar-placeholder">👤</span>
    {/if}
    <div class="head-meta">
      <h1>{mandate.display_name}</h1>
      <p class="office">
        {#if mandate.party && mandate.uf}
          <strong>{mandate.party}</strong>/{mandate.uf} ·
        {/if}
        {mandate.office}
      </p>
      {#if scorecard}
        {@const rate = badgeRate(scorecard)}
        <p class="stats">
          <span class="stat ok"><strong>{scorecard.answered}</strong> respondidas</span>
          <span class="stat bad"><strong>{scorecard.ignored}</strong> ignoradas</span>
          {#if scorecard.median_response_hours !== null}
            <span class="stat"><strong>{formatLatency(scorecard.median_response_hours)}</strong> tempo médio</span>
          {/if}
          {#if rate !== null}
            <span class="rate">{rate}% de resposta</span>
          {/if}
        </p>
      {:else}
        <p class="muted">Sem demandas registradas ainda.</p>
      {/if}
    </div>
    <a class="btn btn-primary cta" href={`/propor?mandate=${mandate.id}`}>
      Propor demanda
    </a>
  </header>

  {#if pendingSlas.length > 0}
    <section class="urgent">
      <h2>⏰ Prazos correndo agora</h2>
      <ul class="sla-list">
        {#each pendingSlas as s (s.id)}
          <li>
            <span>Iniciado em {formatDateTime(s.started_at)}</span>
            <strong>Prazo até {formatDateTime(s.due_at)}</strong>
            <a class="btn btn-ghost btn-sm" href={`/propostas/${s.proposal_id}`}>Ver proposta</a>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  <section class="proposals">
    <h2>Propostas dirigidas ({myProposals.length})</h2>
    {#if myProposals.length === 0}
      <div class="card center">
        <p>Ainda ninguém propôs demanda direta a {mandate.display_name.split(' ')[0]}.</p>
        <p class="muted">Você pode ser a primeira pessoa.</p>
        <a class="btn btn-primary" href={`/propor?mandate=${mandate.id}`}>
          Propor demanda
        </a>
      </div>
    {:else}
      <ul class="prop-list">
        {#each myProposals as p (p.id)}
          <li class="card">
            <a href={`/propostas/${p.id}`} class="prop-link">
              <h3>{p.title}</h3>
              <p class="muted">{p.body.length > 200 ? p.body.slice(0, 197) + '…' : p.body}</p>
              <p class="meta">
                <span>{p.support_count} apoios</span>
                {#if p.cluster_id}<span class="badge badge-acted">agrupada</span>{/if}
              </p>
            </a>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .head {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: 1.5rem;
    align-items: center;
    padding-bottom: 2rem;
    border-bottom: 1px solid var(--c-border);
    margin-bottom: 2rem;
  }
  @media (max-width: 640px) {
    .head {
      grid-template-columns: auto 1fr;
    }
    .cta {
      grid-column: 1 / -1;
      justify-self: stretch;
      text-align: center;
    }
  }
  .avatar-lg {
    width: 96px;
    height: 96px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 2.5rem;
  }
  .head h1 {
    margin: 0 0 0.3rem;
    font-size: 1.6rem;
  }
  .office {
    margin: 0 0 0.6rem;
    color: var(--c-text-muted);
    font-size: 0.95rem;
  }
  .stats {
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.7rem;
    align-items: center;
  }
  .stat {
    font-size: 0.92rem;
  }
  .stat strong {
    font-variant-numeric: tabular-nums;
  }
  .stat.ok strong { color: var(--c-green-dark); }
  .stat.bad strong { color: var(--c-ignored); }
  .rate {
    font-weight: 700;
    color: var(--c-navy);
  }
  .cta {
    align-self: center;
  }
  .urgent {
    background: #fff7e6;
    border: 1px solid #f4c873;
    border-radius: 12px;
    padding: 1.25rem;
    margin-bottom: 2rem;
  }
  .urgent h2 {
    margin: 0 0 0.7rem;
    font-size: 1.05rem;
  }
  .sla-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.5rem;
  }
  .sla-list li {
    display: flex;
    flex-wrap: wrap;
    gap: 0.75rem;
    align-items: center;
    justify-content: space-between;
    font-size: 0.92rem;
  }
  .btn-sm {
    padding: 0.35rem 0.75rem;
    font-size: 0.85rem;
  }
  .proposals h2 {
    margin: 0 0 1rem;
    font-size: 1.05rem;
  }
  .prop-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.75rem;
  }
  .prop-link {
    display: block;
    padding: 1rem 1.25rem;
    color: inherit;
    text-decoration: none;
  }
  .prop-link h3 {
    margin: 0 0 0.4rem;
    font-size: 1rem;
  }
  .meta {
    margin: 0.5rem 0 0;
    font-size: 0.88rem;
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .center {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
</style>
