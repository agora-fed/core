<script lang="ts">
  // Painel do mandato — onde o(a) parlamentar vinculado(a) à conta atual VÊ suas SLAs e
  // RESPONDE publicamente. Sem essa página, o loop da plataforma é unilateral (cidadã cobra,
  // política nunca responde) — o silêncio público existe mas a alternativa não.
  import { onMount } from 'svelte';
  import {
    getMyMandate,
    getSlas,
    getProposal,
    respondToSla,
    DEFAULT_ORG_ID,
    type MyMandateDto,
  } from '../../lib/api';
  import type { SlaDto, ProposalDto, SlaStatus } from '../../lib/types';
  import SlaClock from './SlaClock.svelte';

  let loading = $state(true);
  let mine = $state<MyMandateDto | null>(null);
  let slas = $state<SlaDto[]>([]);
  let proposalsById = $state<Map<string, ProposalDto>>(new Map());
  let loadError = $state<string | null>(null);

  // Per-SLA response form state.
  let openId = $state<string | null>(null);
  let body = $state('');
  let committed = $state(false);
  let busy = $state(false);
  let status = $state<{ kind: 'ok' | 'error'; text: string } | null>(null);

  let pending = $derived(slas.filter((s) => s.status === 'pending'));
  let settled = $derived(slas.filter((s) => s.status !== 'pending'));

  onMount(async () => {
    const mr = await getMyMandate();
    if (!mr.success || !mr.data) {
      loading = false;
      loadError = mr.error?.message ?? 'Não foi possível verificar seu vínculo.';
      return;
    }
    mine = mr.data;
    if (!mine.mandate) {
      loading = false;
      return;
    }
    // Pull SLAs scoped to the org, filter to MY mandate.
    const sr = await getSlas(DEFAULT_ORG_ID, 200);
    if (sr.ok && sr.data) {
      slas = sr.data.filter((s) => s.mandate_id === mine!.mandate!.id);
      // Hydrate proposal titles (one fetch per SLA, parallel).
      const titles = await Promise.all(
        slas.map((s) => getProposal(s.proposal_id)),
      );
      const map = new Map<string, ProposalDto>();
      titles.forEach((p) => {
        if (p.ok && p.data) map.set(p.data.id, p.data);
      });
      proposalsById = map;
    }
    loading = false;
  });

  function open(id: string) {
    openId = openId === id ? null : id;
    body = '';
    committed = false;
    status = null;
  }

  async function submit(slaId: string) {
    if (busy || body.trim().length < 10) return;
    busy = true;
    status = null;
    const res = await respondToSla(slaId, body.trim(), committed);
    busy = false;
    if (res.success) {
      status = {
        kind: 'ok',
        text: committed
          ? 'Resposta com compromisso registrada. Aparece no placar.'
          : 'Resposta registrada. Aparece no placar.',
      };
      // Optimistic: flip the SLA out of "pending" in the local view.
      slas = slas.map((s) =>
        s.id === slaId
          ? { ...s, status: (committed ? 'acted' : 'answered') as SlaStatus }
          : s,
      );
      openId = null;
      body = '';
      committed = false;
    } else {
      status = {
        kind: 'error',
        text: res.error?.message ?? 'Não foi possível registrar a resposta.',
      };
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else if !mine?.mandate}
  <div class="card center">
    <h2>Você ainda não está vinculada a um mandato</h2>
    <p class="muted">
      Esta página existe para parlamentares acompanharem e responderem
      publicamente às demandas direcionadas a eles. Se você é parlamentar,
      precisa aceitar um convite de vinculação primeiro.
    </p>
    <p class="muted">
      <a href="/configuracoes">Voltar para configurações</a>
    </p>
  </div>
{:else}
  <header class="head">
    {#if mine.mandate.avatar_url}
      <img class="avatar" src={mine.mandate.avatar_url} alt="" />
    {:else}
      <span class="avatar avatar-placeholder">👤</span>
    {/if}
    <div>
      <h1>Painel do mandato</h1>
      <p class="muted">
        <strong>{mine.mandate.display_name}</strong> ·
        {mine.mandate.party}/{mine.mandate.uf} ·
        {mine.mandate.house === 'camara' ? 'Câmara' : 'Senado'}
      </p>
      <p class="muted small">
        Vínculo verificado em nível <strong>{mine.binding_level}</strong>.
      </p>
    </div>
  </header>

  <section>
    <h2>
      ⏰ Demandas com prazo correndo ({pending.length})
    </h2>
    {#if pending.length === 0}
      <p class="muted">Nenhuma demanda em prazo aberto neste momento.</p>
    {:else}
      <ul class="sla-list">
        {#each pending as s (s.id)}
          {@const p = proposalsById.get(s.proposal_id)}
          <li class="sla-card">
            <div class="title-row">
              <a class="prop-link" href={`/propostas/${s.proposal_id}`}>
                <strong>{p?.title ?? 'Proposta'}</strong>
              </a>
              <button
                class="btn btn-primary btn-sm"
                type="button"
                onclick={() => open(s.id)}
              >
                {openId === s.id ? 'Cancelar' : 'Responder'}
              </button>
            </div>
            <SlaClock
              dueAt={s.due_at}
              startedAt={s.started_at}
              status={s.status as SlaStatus}
            />
            {#if openId === s.id}
              <form class="respond-form" onsubmit={(e) => { e.preventDefault(); submit(s.id); }}>
                <label for={`b-${s.id}`}>Sua resposta pública</label>
                <textarea
                  id={`b-${s.id}`}
                  class="input"
                  rows="5"
                  bind:value={body}
                  placeholder="Explique como vai agir sobre esta demanda."
                  maxlength="4000"
                ></textarea>
                <label class="commit">
                  <input type="checkbox" bind:checked={committed} />
                  Estou assumindo um <strong>compromisso público</strong> de ação
                  sobre esta demanda (e não só respondendo).
                </label>
                <button
                  class="btn btn-primary"
                  type="submit"
                  disabled={busy || body.trim().length < 10}
                >
                  {busy ? 'Enviando…' : 'Publicar resposta'}
                </button>
                {#if status}
                  <p class={`hint ${status.kind === 'ok' ? 'hint-ok' : 'hint-error'}`}>
                    {status.text}
                  </p>
                {/if}
              </form>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if settled.length > 0}
    <section>
      <h2>Histórico de respostas ({settled.length})</h2>
      <ul class="sla-list compact">
        {#each settled as s (s.id)}
          {@const p = proposalsById.get(s.proposal_id)}
          <li class="sla-card-compact">
            <a class="prop-link" href={`/propostas/${s.proposal_id}`}>
              {p?.title ?? 'Proposta'}
            </a>
            <span class={`badge badge-${s.status}`}>
              {s.status === 'answered'
                ? '✓ Respondida'
                : s.status === 'acted'
                  ? '✓ Com compromisso'
                  : '✗ Silêncio público'}
            </span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    gap: 1.25rem;
    align-items: center;
    margin-bottom: 2rem;
    padding-bottom: 1.5rem;
    border-bottom: 1px solid var(--c-border);
  }
  .head h1 {
    margin: 0 0 0.4rem;
    font-size: 1.5rem;
  }
  .head .muted {
    margin: 0;
  }
  .head .small {
    font-size: 0.85rem;
  }
  .avatar {
    width: 80px;
    height: 80px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    flex-shrink: 0;
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 2rem;
  }
  section {
    margin-bottom: 2.5rem;
  }
  section h2 {
    font-size: 1.05rem;
    margin: 0 0 1rem;
  }
  .sla-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 1rem;
  }
  .sla-card {
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    padding: 1rem;
  }
  .title-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    margin-bottom: 0.85rem;
    flex-wrap: wrap;
  }
  .prop-link {
    color: var(--c-navy);
    text-decoration: none;
  }
  .prop-link:hover {
    text-decoration: underline;
  }
  .btn-sm {
    padding: 0.4rem 0.85rem;
    font-size: 0.9rem;
  }
  .respond-form {
    margin-top: 1rem;
    padding-top: 1rem;
    border-top: 1px solid var(--c-border);
    display: grid;
    gap: 0.6rem;
  }
  .respond-form label {
    font-weight: 600;
    font-size: 0.95rem;
  }
  .respond-form textarea {
    width: 100%;
    resize: vertical;
    font-family: inherit;
  }
  .commit {
    display: flex;
    gap: 0.5rem;
    align-items: flex-start;
    font-weight: 400;
    font-size: 0.92rem;
    color: var(--c-text);
  }
  .commit input {
    margin-top: 0.25rem;
  }
  .sla-card-compact {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.7rem 1rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 10px;
    gap: 1rem;
    flex-wrap: wrap;
  }
  .badge-answered { background: var(--c-green-soft); color: var(--c-green-dark); }
  .badge-acted    { background: var(--c-green-soft); color: var(--c-green-dark); font-weight: 700; }
  .badge-ignored  { background: #fef2f2; color: #b91c1c; }
  .center {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  .hint-ok { color: var(--c-green-dark); }
</style>
