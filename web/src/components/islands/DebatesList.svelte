<script lang="ts">
  // Lista de debates carregada NO CLIENTE — reflete o banco em tempo real
  // (SSG mostraria debates congelados no build).
  import { onMount } from 'svelte';
  import { getDebates, DEFAULT_ORG_ID } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import { ufName } from '../../lib/ufs';
  import type { DebateDto } from '../../lib/types';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<DebateDto[]>([]);
  let filterUf = $state(''); // '' = todos; 'NAC' = nacional; senão o código da UF

  // UFs presentes nos debates carregados (para montar o seletor sem estados vazios).
  const presentUfs = $derived(
    [...new Set(items.map((d) => d.uf).filter((u): u is string => !!u))].sort(),
  );
  const filtered = $derived(
    filterUf === ''
      ? items
      : filterUf === 'NAC'
        ? items.filter((d) => !d.uf)
        : items.filter((d) => d.uf === filterUf),
  );

  onMount(async () => {
    const res = await getDebates(DEFAULT_ORG_ID, 100);
    loading = false;
    if (res.ok && res.data) items = res.data;
    else error = res.error ?? 'Não foi possível carregar os debates.';
  });
</script>

{#if loading}
  <p class="muted">Carregando debates…</p>
{:else if error}
  <div class="card state" role="alert">
    <h2>Não foi possível carregar os debates</h2>
    <p class="muted">{error}</p>
  </div>
{:else if items.length === 0}
  <div class="card state">
    <h2>Nenhum debate em aberto</h2>
    <p class="muted">Abra o primeiro debate acima.</p>
  </div>
{:else}
  {#if presentUfs.length > 0}
    <div class="filterbar">
      <label class="filter-label" for="uf-filter">Filtrar por abrangência:</label>
      <select id="uf-filter" bind:value={filterUf}>
        <option value="">Todos</option>
        <option value="NAC">Nacional</option>
        {#each presentUfs as code (code)}
          <option value={code}>{ufName(code)} ({code})</option>
        {/each}
      </select>
    </div>
  {/if}
  {#if filtered.length === 0}
    <div class="card state"><p class="muted">Nenhum debate para esse filtro.</p></div>
  {:else}
    <ul class="list grid grid-2">
      {#each filtered as d (d.id)}
        <li class="card item">
          <a class="item-link" href={`/debate/?id=${d.id}`}>
            <span class="uf-chip">{d.uf ? ufName(d.uf) : 'Nacional'}</span>
            <h2 class="item-title">{d.title}</h2>
            {#if d.framing}<p class="muted clamp">{d.framing}</p>{/if}
            {#if d.created_at}
              <time datetime={d.created_at} class="muted small">{formatDate(d.created_at)}</time>
            {/if}
          </a>
        </li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  .list { list-style: none; padding: 0; margin: 0; }
  .item { padding: 0; }
  .item:hover { transform: translateY(-2px); box-shadow: 0 6px 16px rgba(0,0,0,0.07); transition: transform 100ms ease, box-shadow 100ms ease; }
  .item-link { display: block; padding: 1.25rem; text-decoration: none; color: inherit; }
  .item-title { font-size: 1.15rem; margin: 0 0 0.4rem; }
  .clamp {
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .small { font-size: 0.85rem; }
  .card {
    background: var(--surface-1, var(--c-paper));
    border: 1px solid var(--border-subtle, var(--c-border));
    border-radius: 12px;
  }
  .state { text-align: center; padding: 3rem 1.5rem; }
  .filterbar { display: flex; align-items: center; gap: 0.6rem; margin-bottom: 1.2rem; flex-wrap: wrap; }
  .filter-label { font-size: 0.9rem; color: var(--muted, #64748b); font-weight: 600; }
  .filterbar select { padding: 0.45rem 0.6rem; border-radius: 8px; border: 1px solid var(--c-border, #cbd5e1); background: var(--surface-1, var(--c-paper)); color: inherit; font: inherit; }
  .uf-chip { display: inline-block; font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.02em; color: var(--c-green-dark, #15803d); background: var(--c-green-soft, #dcfce7); padding: 0.12rem 0.5rem; border-radius: 999px; margin-bottom: 0.5rem; }
</style>
