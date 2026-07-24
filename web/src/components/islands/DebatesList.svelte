<script lang="ts">
  // Lista de debates carregada NO CLIENTE — reflete o banco em tempo real
  // (SSG mostraria debates congelados no build).
  import { onMount } from 'svelte';
  import { getDebates, DEFAULT_ORG_ID } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import type { DebateDto } from '../../lib/types';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<DebateDto[]>([]);

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
  <ul class="list grid grid-2">
    {#each items as d (d.id)}
      <li class="card item">
        <a class="item-link" href={`/debate/?id=${d.id}`}>
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
</style>
