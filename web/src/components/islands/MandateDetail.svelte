<script lang="ts">
  // Página individual do parlamentar — CSR (renderiza pra qualquer id existente, sem rebuild).
  // Mostra foto, partido/UF/casa, scorecard quando há, propostas dirigidas + SLAs pendentes.
  // Botão proeminente "Propor demanda a Fulano" leva pra /propor?mandate=<id> com pré-seleção.
  import { onMount } from 'svelte';
  import {
    getMandate,
    getMandateActivity,
    getProposals,
    getScorecard,
    getSlas,
    DEFAULT_ORG_ID,
    type ActivityDto,
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
  let activity = $state<ActivityDto | null>(null);
  let activityLoading = $state(true);

  let pendingSlas = $derived(mySlas.filter((s) => s.status === 'pending'));

  const brlFormatter = new Intl.NumberFormat('pt-BR', {
    style: 'currency',
    currency: 'BRL',
  });
  const formatBrl = (v: number) => brlFormatter.format(v);

  onMount(async () => {
    const [mr, scr, pr, slr, ar] = await Promise.all([
      getMandate(mandateId),
      getScorecard(mandateId),
      getProposals(DEFAULT_ORG_ID, 200),
      getSlas(DEFAULT_ORG_ID, 200),
      getMandateActivity(mandateId),
    ]);
    loading = false;
    activityLoading = false;
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
    // Activity is best-effort: never blocks the page. If failed, section just doesn't render.
    if (ar.ok && ar.data) activity = ar.data;
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

  function truncate(text: string, max = 200): string {
    return text.length > max ? text.slice(0, max - 3) + '…' : text;
  }

  function productionLabel(p: {
    kind: string | null;
    number: string | null;
    year: string | null;
    summary: string | null;
  }): string {
    const head = [p.kind, p.number && p.year ? `${p.number}/${p.year}` : (p.number ?? p.year)]
      .filter(Boolean)
      .join(' ');
    if (head && p.summary) return `${head} — ${p.summary}`;
    return head || p.summary || 'Proposição';
  }

  function houseName(source: string | null | undefined): string {
    if (source === 'camara') return 'Câmara dos Deputados';
    if (source === 'senado') return 'Senado Federal';
    return 'casa';
  }

  let hasActivity = $derived(
    !!activity &&
      (activity.agenda.length > 0 ||
        activity.production.length > 0 ||
        activity.speeches.length > 0 ||
        activity.votes.length > 0 ||
        activity.expenses !== null),
  );
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
      {#if mandate.has_verified_operator}
        <p class="verified-line">
          <span
            class="badge-verified"
            title="Mandato com operador verificado"
            aria-label="Vínculo verificado">✓</span
          >
          <span class="muted">Vínculo verificado</span>
        </p>
      {/if}
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
    <div class="cta-group">
      <a class="btn btn-primary cta" href={`/propor?mandate=${mandate.id}`}>
        Propor demanda
      </a>
      <a class="btn btn-ghost cta-alt" href={`/politicos/${mandate.id}/placar`}>
        📊 Placar público
      </a>
    </div>
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

  {#if !activityLoading && activity && hasActivity}
    <section class="activity">
      <header class="activity-head">
        <h2>Atividade oficial na {houseName(activity.source)}</h2>
        <p class="muted">fonte: dados abertos</p>
      </header>

      {#if activity.agenda.length > 0}
        <div class="sub">
          <h3>Agenda pública</h3>
          <ul class="act-list">
            {#each activity.agenda as ev, i (ev.url ?? `${ev.title}-${i}`)}
              <li class="card">
                <p class="act-title">
                  {#if ev.url}
                    <a href={ev.url} target="_blank" rel="noopener noreferrer">{ev.title}</a>
                  {:else}
                    {ev.title}
                  {/if}
                </p>
                <p class="muted meta">
                  {#if ev.date_time}<span>{formatDateTime(ev.date_time)}</span>{/if}
                  {#if ev.location}<span>· {ev.location}</span>{/if}
                  {#if ev.kind}<span>· {ev.kind}</span>{/if}
                </p>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if activity.production.length > 0}
        <div class="sub">
          <h3>Produção legislativa</h3>
          <ul class="act-list">
            {#each activity.production as p, i (p.id ?? `${p.kind}-${p.number}-${i}`)}
              <li class="card">
                <p class="act-title">
                  {#if p.url}
                    <a href={p.url} target="_blank" rel="noopener noreferrer">{productionLabel(p)}</a>
                  {:else}
                    {productionLabel(p)}
                  {/if}
                </p>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if activity.speeches.length > 0}
        <div class="sub">
          <h3>Discursos recentes</h3>
          <ul class="act-list">
            {#each activity.speeches.slice(0, 10) as s, i (s.url ?? `${s.date_time}-${i}`)}
              <li class="card">
                <p class="muted meta">
                  {#if s.date_time}<span>{formatDateTime(s.date_time)}</span>{/if}
                </p>
                {#if s.summary}
                  <p>{truncate(s.summary, 200)}</p>
                {/if}
                {#if s.url}
                  <p class="meta">
                    <a href={s.url} target="_blank" rel="noopener noreferrer">Ver íntegra</a>
                  </p>
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if activity.votes.length > 0}
        <div class="sub">
          <h3>Votos nominais</h3>
          <ul class="act-list">
            {#each activity.votes as v, i (`${v.matter ?? ''}-${v.date ?? ''}-${i}`)}
              <li class="card">
                <p class="act-title">{v.matter ?? 'Matéria'}</p>
                <p class="meta">
                  {#if v.vote}<span><strong>Voto:</strong> {v.vote}</span>{/if}
                  {#if v.result}<span>· <strong>Resultado:</strong> {v.result}</span>{/if}
                  {#if v.date}<span class="muted">· {v.date}</span>{/if}
                </p>
              </li>
            {/each}
          </ul>
        </div>
      {/if}

      {#if activity.expenses}
        <div class="sub">
          <h3>Despesas — cota parlamentar{activity.expenses.year ? ` (${activity.expenses.year})` : ''}</h3>
          <div class="card">
            <p class="act-title">Total: <strong>{formatBrl(activity.expenses.total)}</strong></p>
            {#if activity.expenses.top_categories.length > 0}
              <ul class="cat-list">
                {#each activity.expenses.top_categories as cat (cat.name)}
                  <li>
                    <span>{cat.name}</span>
                    <span class="cat-amount">{formatBrl(cat.amount)}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        </div>
      {/if}
    </section>
  {/if}
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
  .badge-verified {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--c-green-soft, #d4edda);
    color: var(--c-green-dark, #1e6b3a);
    font-weight: 700;
    border-radius: 999px;
    line-height: 1;
    padding: 0.2rem 0.55rem;
    font-size: 0.85rem;
    margin-right: 0.35rem;
    vertical-align: middle;
  }
  .verified-line {
    margin: 0.25rem 0 0.5rem;
    font-size: 0.85rem;
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
  .cta-group {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    align-items: center;
  }
  .cta-alt {
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
  .activity {
    margin-top: 2.5rem;
    padding-top: 1.5rem;
    border-top: 1px solid var(--c-border);
  }
  .activity-head {
    margin-bottom: 1rem;
  }
  .activity-head h2 {
    margin: 0 0 0.15rem;
    font-size: 1.05rem;
  }
  .activity-head p {
    margin: 0;
    font-size: 0.85rem;
  }
  .sub {
    margin-top: 1.5rem;
  }
  .sub h3 {
    margin: 0 0 0.6rem;
    font-size: 0.98rem;
  }
  .act-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.6rem;
  }
  .act-list li.card {
    padding: 0.85rem 1rem;
  }
  .act-title {
    margin: 0 0 0.3rem;
    font-weight: 600;
    font-size: 0.96rem;
  }
  .act-title a {
    color: inherit;
  }
  .cat-list {
    list-style: none;
    padding: 0;
    margin: 0.6rem 0 0;
    display: grid;
    gap: 0.3rem;
  }
  .cat-list li {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    font-size: 0.92rem;
  }
  .cat-amount {
    font-variant-numeric: tabular-nums;
    color: var(--c-text-muted);
  }
</style>
