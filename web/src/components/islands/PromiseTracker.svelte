<script lang="ts">
  // Rastreador de promessas — a "outra metade do placar" (Fase 4.2). Leitura
  // PÚBLICA: promessas feitas × cumpridas. Quando o visitante É o dono do
  // mandato (getMyMandate), aparecem os controles de registrar/cumprir — o
  // backend reforça o gate MIN_OFFICIAL_LEVEL de qualquer forma.
  import { onMount } from 'svelte';
  import {
    getPromises,
    recordPromise,
    deliverPromise,
    getMyMandate,
  } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import type { PromiseDto } from '../../lib/types';

  let { mandateId }: { mandateId: string } = $props();

  let loading = $state(true);
  let error = $state<string | null>(null);
  let promises = $state<PromiseDto[]>([]);
  let isOwner = $state(false);

  // Form do dono.
  let newText = $state('');
  let saving = $state(false);
  let formError = $state<string | null>(null);
  let busyId = $state<string | null>(null);

  const delivered = $derived(promises.filter((p) => p.delivered).length);
  const total = $derived(promises.length);
  const pct = $derived(total > 0 ? Math.round((delivered / total) * 100) : 0);

  async function load() {
    const res = await getPromises(mandateId);
    // Um mandato sem scorecard projetado ainda (a maioria) responde 404 — isso
    // é "nenhuma promessa ainda", não um erro. A primeira promessa cria o scorecard.
    promises = res.ok && res.data ? res.data : [];
  }

  async function addPromise(e: SubmitEvent) {
    e.preventDefault();
    if (newText.trim().length === 0 || saving) return;
    saving = true;
    formError = null;
    const res = await recordPromise(mandateId, newText.trim());
    saving = false;
    if (res.success) {
      newText = '';
      await load();
    } else {
      formError = res.error?.message ?? 'Não foi possível registrar a promessa.';
    }
  }

  async function markDelivered(id: string) {
    if (busyId) return;
    busyId = id;
    const res = await deliverPromise(id);
    busyId = null;
    if (res.success) await load();
  }

  onMount(async () => {
    await load();
    // Sou o dono deste mandato? (controles só aparecem então.)
    try {
      const mine = await getMyMandate();
      if (mine.success && mine.data?.mandate?.id === mandateId) isOwner = true;
    } catch {
      isOwner = false;
    }
    loading = false;
  });
</script>

<section class="promises" aria-labelledby="promises-h">
  <div class="head">
    <h2 id="promises-h">Promessas</h2>
    {#if total > 0}
      <span class="count">{delivered} de {total} cumprida{total === 1 ? '' : 's'}</span>
    {/if}
  </div>

  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <p class="hint-error" role="alert">{error}</p>
  {:else}
    {#if total > 0}
      <div class="progress" role="img" aria-label={`${pct}% das promessas cumpridas`}>
        <div class="progress-fill" style={`width:${pct}%`}></div>
      </div>
    {/if}

    {#if isOwner}
      <form class="card add" onsubmit={addPromise}>
        <label for="promise-text">Registrar uma promessa pública</label>
        <textarea id="promise-text" bind:value={newText} maxlength="500" rows="2"
          placeholder="Ex.: Vou apresentar um projeto de creche em tempo integral no primeiro semestre."></textarea>
        {#if formError}<p class="hint-error" role="alert">{formError}</p>{/if}
        <button type="submit" class="btn primary" disabled={saving || newText.trim().length === 0}>
          {saving ? 'Registrando…' : 'Registrar promessa'}
        </button>
      </form>
    {/if}

    {#if total === 0}
      <p class="muted empty">
        {isOwner
          ? 'Você ainda não registrou nenhuma promessa. A transparência começa aqui.'
          : 'Este mandato ainda não registrou promessas públicas.'}
      </p>
    {:else}
      <ul class="list">
        {#each promises as p (p.id)}
          <li class="card promise" class:done={p.delivered}>
            <p class="text">{p.text}</p>
            <div class="foot">
              {#if p.delivered}
                <span class="badge done">✓ Cumprida{p.delivered_at ? ' em ' + formatDate(p.delivered_at) : ''}</span>
              {:else}
                <span class="badge pending">Pendente</span>
              {/if}
              <time class="muted small" datetime={p.made_at}>prometida em {formatDate(p.made_at)}</time>
              {#if isOwner && !p.delivered}
                <button class="btn small" disabled={busyId === p.id} onclick={() => markDelivered(p.id)}>
                  {busyId === p.id ? '…' : 'Marcar como cumprida'}
                </button>
              {/if}
            </div>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  .promises { margin-top: 2rem; }
  .head { display: flex; align-items: baseline; justify-content: space-between; gap: 1rem; margin-bottom: 0.8rem; }
  .head h2 { margin: 0; font-size: 1.25rem; }
  .count { font-weight: 700; color: var(--c-green-dark, #15803d); font-variant-numeric: tabular-nums; }
  .progress { height: 0.5rem; background: var(--surface-2, #f1f5f9); border-radius: 999px; overflow: hidden; margin-bottom: 1.2rem; }
  .progress-fill { height: 100%; background: #22c55e; border-radius: 999px; transition: width 200ms ease; }
  .card { background: var(--surface-1, #fff); border: 1px solid var(--border-subtle, rgba(0,0,0,0.1)); border-radius: 12px; }
  .add { padding: 1.1rem 1.2rem; display: grid; gap: 0.6rem; margin-bottom: 1.2rem; }
  .add label { font-weight: 600; }
  textarea { padding: 0.6rem 0.7rem; border-radius: 8px; border: 1px solid var(--border-subtle, #ccc); background: var(--surface-1, #fff); color: inherit; font: inherit; resize: vertical; }
  .list { list-style: none; padding: 0; margin: 0; display: grid; gap: 0.7rem; }
  .promise { padding: 1rem 1.2rem; display: grid; gap: 0.6rem; }
  .promise.done { border-color: #86efac; }
  .text { margin: 0; }
  .foot { display: flex; align-items: center; gap: 0.75rem; flex-wrap: wrap; }
  .badge { font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; padding: 0.18rem 0.55rem; border-radius: 999px; }
  .badge.done { background: #dcfce7; color: #15803d; }
  .badge.pending { background: #fef3c7; color: #92400e; }
  .small { font-size: 0.82rem; }
  .empty { padding: 1rem 0; }
  .btn { padding: 0.5rem 1rem; border-radius: 8px; border: 1px solid var(--c-ink, #0f172a); background: var(--surface-1, #fff); color: inherit; font-weight: 600; cursor: pointer; }
  .btn.primary { justify-self: start; background: var(--c-green-dark, #15803d); border-color: var(--c-green-dark, #15803d); color: #fff; }
  .btn.small { padding: 0.35rem 0.8rem; font-size: 0.82rem; margin-left: auto; }
  .btn:disabled { opacity: 0.55; cursor: not-allowed; }
  .hint-error { color: #dc2626; margin: 0; }
</style>
