<script lang="ts">
  // Página detalhada de proposta — CSR. Foca em VOTAR: contador grande, barra de progresso até o
  // threshold (se houver), botão Apoiar. Mostra também o político-alvo (foto + nome + link), o
  // status da SLA (relógio rolando ou silêncio público), e o corpo da demanda. Comentários
  // ficam abaixo.
  import { onMount } from 'svelte';
  import {
    getProposal,
    getMandate,
    getSlas,
    DEFAULT_ORG_ID,
    apiPost,
  } from '../../lib/api';
  import type { ProposalDto, MandateDto, SlaDto, SlaStatus } from '../../lib/types';
  import SlaClock from './SlaClock.svelte';
  import CommentForm from './CommentForm.svelte';

  let { proposalId }: { proposalId: string } = $props();

  let loading = $state(true);
  let proposal = $state<ProposalDto | null>(null);
  let mandate = $state<MandateDto | null>(null);
  let sla = $state<SlaDto | null>(null);
  let loadError = $state<string | null>(null);

  // Voting state
  let count = $state(0);
  let supported = $state(false);
  let voting = $state(false);
  let voteMsg = $state<{ kind: 'error' | 'info' | 'ok'; text: string } | null>(null);

  // Threshold é informação local; o backend conhece mas não retorna no ProposalDto. Por ora,
  // um padrão visual: barra cresce até 100 apoios (depois acumula). Quando a SLA existe,
  // significa que o threshold foi atingido.
  const SOFT_THRESHOLD = 100;
  let progressPct = $derived(
    sla ? 100 : Math.min(100, Math.round((count / SOFT_THRESHOLD) * 100)),
  );
  let thresholdCrossed = $derived(sla !== null);

  onMount(async () => {
    const [pr, slr] = await Promise.all([
      getProposal(proposalId),
      getSlas(DEFAULT_ORG_ID, 500),
    ]);
    if (!pr.ok || !pr.data) {
      loading = false;
      loadError = pr.error ?? 'Proposta não encontrada.';
      return;
    }
    proposal = pr.data;
    count = proposal.support_count;
    if (slr.ok && slr.data) {
      sla = slr.data.find((s) => s.proposal_id === proposalId) ?? null;
    }
    const mr = await getMandate(proposal.mandate_id);
    if (mr.ok && mr.data) mandate = mr.data;
    loading = false;
  });

  function readCitizenId(): string | null {
    try {
      return localStorage.getItem('dsoc_citizen');
    } catch {
      return null;
    }
  }

  async function support() {
    if (voting || supported) return;
    voting = true;
    voteMsg = null;
    const citizenId = readCitizenId();
    if (!citizenId) {
      voteMsg = { kind: 'info', text: 'Entre na sua conta para apoiar.' };
      voting = false;
      return;
    }
    count += 1;
    supported = true;
    const res = await apiPost<ProposalDto>('/api/v1/votes', {
      org_id: DEFAULT_ORG_ID,
      proposal_id: proposalId,
      citizen_id: citizenId,
    });
    if (!res.success) {
      count -= 1;
      supported = false;
      voteMsg = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível apoiar.',
      };
    } else {
      voteMsg = { kind: 'ok', text: 'Apoio registrado. Obrigado.' };
    }
    voting = false;
  }

  function share() {
    const url = window.location.href;
    const title = proposal?.title ?? 'Demanda pública na DemocraciaBR';
    if (navigator.share) {
      navigator.share({ title, url }).catch(() => {});
    } else {
      navigator.clipboard?.writeText(url);
      voteMsg = { kind: 'ok', text: 'Link copiado.' };
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <div class="card center" role="alert">
    <h2>{loadError}</h2>
    <p class="muted"><a href="/propostas">Voltar para propostas</a></p>
  </div>
{:else if proposal}
  <article class="proposal">
    <!-- Cabeçalho com o destinatário em destaque (o "a quem isto se dirige") -->
    {#if mandate}
      <a class="target" href={`/politicos/${mandate.id}`}>
        {#if mandate.avatar_url}
          <img class="target-avatar" src={mandate.avatar_url} alt="" />
        {:else}
          <span class="target-avatar avatar-placeholder">👤</span>
        {/if}
        <div>
          <span class="target-label muted">Demanda direcionada a:</span>
          <strong class="target-name">{mandate.display_name}</strong>
          <span class="muted">
            {mandate.party}/{mandate.uf} ·
            {mandate.house === 'camara' ? 'Câmara' : mandate.house === 'senado' ? 'Senado' : ''}
          </span>
        </div>
      </a>
    {/if}

    <h1>{proposal.title}</h1>

    <!-- Bloco GRANDE de votação — o coração da página -->
    <section class="vote-block" class:crossed={thresholdCrossed}>
      <div class="vote-stats">
        <p class="count">
          <strong>{count.toLocaleString('pt-BR')}</strong>
          {count === 1 ? 'pessoa apoia' : 'pessoas apoiam'}
        </p>
        {#if thresholdCrossed}
          <p class="threshold-met">
            ✓ Limiar atingido — relógio de resposta correndo
          </p>
        {:else}
          <p class="threshold muted">
            Quando atingir <strong>{SOFT_THRESHOLD}</strong> apoios, começa o
            prazo de resposta.
          </p>
        {/if}
      </div>
      <div class="progress" aria-label={`${progressPct}%`}>
        <div class="progress-fill" style={`width:${progressPct}%`}></div>
      </div>
      <div class="actions">
        <button
          class="btn btn-primary btn-lg"
          type="button"
          onclick={support}
          disabled={voting || supported}
          aria-pressed={supported}
        >
          {#if supported}
            ✓ Você apoia
          {:else}
            Apoiar esta demanda
          {/if}
        </button>
        <button class="btn btn-ghost" type="button" onclick={share}>
          Compartilhar
        </button>
      </div>
      {#if voteMsg}
        <p class={`vote-msg ${voteMsg.kind}`} role="status">
          {voteMsg.text}
          {#if voteMsg.kind === 'info'}<a href="/entrar">Entrar</a>{/if}
        </p>
      {/if}
    </section>

    <!-- SLA clock quando o limiar já foi cruzado -->
    {#if sla}
      <section class="sla-block">
        <h2>⏰ Prazo de resposta</h2>
        <SlaClock
          dueAt={sla.due_at}
          status={sla.status as SlaStatus}
        />
        <p class="muted">
          Se o prazo expirar sem resposta, fica como <strong>silêncio público</strong>
          permanente no placar do(a) parlamentar.
        </p>
      </section>
    {/if}

    <!-- Corpo da proposta -->
    <section class="body">
      <h2>Sobre a demanda</h2>
      <p class="body-text">{proposal.body}</p>
    </section>

    <!-- Comentários -->
    <section class="comments">
      <h2>Comentários</h2>
      <CommentForm proposalId={proposalId} />
    </section>
  </article>
{/if}

<style>
  .proposal {
    max-width: 44rem;
    margin: 0 auto;
  }
  .target {
    display: flex;
    gap: 0.85rem;
    align-items: center;
    padding: 0.85rem 1rem;
    background: var(--c-bg);
    border-radius: 12px;
    text-decoration: none;
    color: inherit;
    margin-bottom: 1.5rem;
    border: 1px solid var(--c-border);
  }
  .target:hover {
    background: var(--c-paper);
  }
  .target-avatar {
    width: 52px;
    height: 52px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-paper);
    flex-shrink: 0;
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 1.4rem;
  }
  .target-label {
    display: block;
    font-size: 0.78rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .target-name {
    display: block;
    font-size: 1.05rem;
  }
  h1 {
    font-size: 1.7rem;
    margin: 0 0 1.5rem;
    line-height: 1.25;
  }
  .vote-block {
    background: var(--c-paper);
    border: 2px solid var(--c-border);
    border-radius: 14px;
    padding: 1.5rem;
    margin-bottom: 2rem;
  }
  .vote-block.crossed {
    border-color: var(--c-green-dark);
    background: var(--c-green-soft);
  }
  .vote-stats {
    text-align: center;
    margin-bottom: 1rem;
  }
  .count {
    margin: 0;
    font-size: 1rem;
  }
  .count strong {
    font-size: 2.4rem;
    color: var(--c-navy);
    display: block;
    line-height: 1;
    font-variant-numeric: tabular-nums;
  }
  .threshold {
    margin: 0.5rem 0 0;
    font-size: 0.92rem;
  }
  .threshold-met {
    margin: 0.5rem 0 0;
    color: var(--c-green-dark);
    font-weight: 600;
  }
  .progress {
    height: 10px;
    background: var(--c-bg);
    border-radius: 999px;
    overflow: hidden;
    margin-bottom: 1.25rem;
  }
  .progress-fill {
    display: block;
    height: 100%;
    background: var(--c-green);
    transition: width 240ms ease;
  }
  .vote-block.crossed .progress-fill {
    background: var(--c-green-dark);
  }
  .actions {
    display: flex;
    gap: 0.6rem;
    justify-content: center;
    flex-wrap: wrap;
  }
  .vote-msg {
    margin: 0.85rem 0 0;
    text-align: center;
    font-size: 0.92rem;
  }
  .vote-msg.error { color: var(--c-ignored); }
  .vote-msg.ok    { color: var(--c-green-dark); font-weight: 600; }
  .vote-msg.info  { color: var(--c-text-muted); }
  .sla-block {
    border: 1px solid var(--c-border);
    border-radius: 12px;
    padding: 1.25rem;
    margin-bottom: 2rem;
  }
  .sla-block h2 {
    margin: 0 0 0.8rem;
    font-size: 1.05rem;
  }
  .body h2, .comments h2 {
    margin: 0 0 0.8rem;
    font-size: 1.05rem;
  }
  .body-text {
    white-space: pre-wrap;
    line-height: 1.6;
    margin: 0 0 2.5rem;
  }
  .center {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
</style>
