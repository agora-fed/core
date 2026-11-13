<script lang="ts">
  // /abaixo-assinados: the list of petitions, and the form a member uses to start one.
  import { onMount } from 'svelte';
  import { createPetition, listPetitions, progress, type PetitionListItem } from '../../lib/petitions';
  import { toast } from '../../lib/toasts';

  let loading = $state(true);
  let signedIn = $state(false);
  let isAdmin = $state(false);
  let tab = $state<'open' | 'closed' | 'hidden'>('open');
  let items = $state<PetitionListItem[]>([]);
  let showForm = $state(false);
  let busy = $state(false);
  let draft = $state({ title: '', body: '', threshold: 100 });

  async function load() {
    loading = true;
    const res = await listPetitions(tab);
    items = res.success && res.data ? res.data : [];
    loading = false;
  }

  async function create(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    const res = await createPetition({ ...draft, threshold: Number(draft.threshold) });
    busy = false;
    if (!res.success || !res.data) {
      toast.error(res.error?.message ?? 'Não foi possível publicar.');
      return;
    }
    window.location.href = `/abaixo-assinado/?id=${encodeURIComponent(res.data.id)}`;
  }

  function choose(next: 'open' | 'closed' | 'hidden') {
    tab = next;
    void load();
  }

  onMount(async () => {
    try {
      signedIn = Boolean(localStorage.getItem('dsoc_citizen'));
      isAdmin = localStorage.getItem('dsoc_is_admin') === '1';
    } catch {
      signedIn = false;
    }
    await load();
  });
</script>

<header class="head">
  <h1>Abaixo-assinados</h1>
  {#if signedIn}
    <button class="btn btn-primary" onclick={() => (showForm = !showForm)}>
      {showForm ? 'Cancelar' : 'Criar abaixo-assinado'}
    </button>
  {:else}
    <a class="btn" href="/entrar/?next=/abaixo-assinados">Entre para criar um</a>
  {/if}
</header>
<p class="muted">
  Uma ideia precisa de apoio? Escreva o pedido e reúna assinaturas. Qualquer pessoa pode assinar,
  mesmo sem conta: basta confirmar o e-mail.
</p>

{#if showForm}
  <form class="card form" onsubmit={create}>
    <label>
      Título
      <input required minlength="5" maxlength="160" bind:value={draft.title} placeholder="O que você pede, em uma frase" />
    </label>
    <label>
      Texto
      <textarea required minlength="20" maxlength="20000" rows="8" bind:value={draft.body}
        placeholder="Explique o problema, o que deve mudar e a quem o pedido se dirige."></textarea>
    </label>
    <label>
      Meta de assinaturas
      <input type="number" required min="1" max="10000000" bind:value={draft.threshold} />
    </label>
    <p class="muted small">O abaixo-assinado é publicado na hora. A administração pode ocultar textos que violem as regras da comunidade.</p>
    <button class="btn btn-primary" disabled={busy}>Publicar</button>
  </form>
{/if}

<div class="tabs" role="tablist">
  <button role="tab" aria-selected={tab === 'open'} class:active={tab === 'open'} onclick={() => choose('open')}>Abertos</button>
  <button role="tab" aria-selected={tab === 'closed'} class:active={tab === 'closed'} onclick={() => choose('closed')}>Encerrados</button>
  {#if isAdmin}
    <button role="tab" aria-selected={tab === 'hidden'} class:active={tab === 'hidden'} onclick={() => choose('hidden')}>Ocultos</button>
  {/if}
</div>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if items.length === 0}
  <p class="muted">Nenhum abaixo-assinado aqui ainda.</p>
{:else}
  <ul class="list">
    {#each items as p (p.id)}
      <li class="card">
        <a class="title" href={`/abaixo-assinado/?id=${encodeURIComponent(p.id)}`}>{p.title}</a>
        <p class="summary">{p.summary}</p>
        <div class="bar" aria-hidden="true"><span style={`width:${progress(p.signature_count, p.threshold)}%`}></span></div>
        <p class="muted small">
          {p.signature_count.toLocaleString('pt-BR')} de {p.threshold.toLocaleString('pt-BR')} assinaturas
          {#if p.reached}· <strong>meta alcançada</strong>{/if}
          {#if p.author_name}· por {p.author_name}{/if}
        </p>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .head { display: flex; justify-content: space-between; align-items: center; gap: 1rem; flex-wrap: wrap; }
  .form { display: grid; gap: 0.75rem; margin: 1rem 0; }
  .form label { display: grid; gap: 0.25rem; font-weight: 600; }
  .tabs { display: flex; gap: 0.5rem; margin: 1.25rem 0 0.75rem; }
  .tabs button { background: none; border: 1px solid var(--border-subtle); border-radius: 999px; padding: 0.3rem 0.9rem; cursor: pointer; color: inherit; }
  .tabs button.active { background: var(--accent); border-color: var(--accent); color: var(--accent-contrast); }
  .list { list-style: none; padding: 0; display: grid; gap: 0.75rem; }
  .title { font-weight: 700; font-size: 1.1rem; }
  .summary { margin: 0.4rem 0; white-space: pre-line; }
  .bar { height: 0.5rem; background: var(--surface-2); border-radius: 999px; overflow: hidden; }
  .bar span { display: block; height: 100%; background: var(--accent); }
  .small { font-size: 0.85rem; }
</style>
