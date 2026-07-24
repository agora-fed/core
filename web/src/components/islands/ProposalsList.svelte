<script lang="ts">
  // Lista de propostas carregada NO CLIENTE — reflete o banco em tempo real
  // (a versão SSG mostrava propostas congeladas no build; ocultar/apagar não
  // sumia até o próximo deploy).
  import { onMount } from 'svelte';
  import { getProposals, DEFAULT_ORG_ID } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import type { ProposalDto } from '../../lib/types';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<ProposalDto[]>([]);

  onMount(async () => {
    const res = await getProposals(DEFAULT_ORG_ID, 100);
    loading = false;
    if (res.ok && res.data) items = res.data;
    else error = res.error ?? 'Não foi possível carregar as propostas.';
  });
</script>

{#if loading}
  <p class="muted">Carregando propostas…</p>
{:else if error}
  <div class="card state" role="alert">
    <h2>Não foi possível carregar as propostas</h2>
    <p class="muted">{error}</p>
  </div>
{:else if items.length === 0}
  <div class="card state">
    <h2>Seja o primeiro a propor</h2>
    <p class="muted">Ainda não há propostas publicadas. Use o formulário abaixo para começar.</p>
  </div>
{:else}
  <ul class="list grid grid-2">
    {#each items as p (p.id)}
      <li class="card prop">
        <a class="prop-link" href={`/propostas/${p.id}`}>
          <h2 class="prop-title">{p.title}</h2>
          <p class="prop-body muted">{p.body}</p>
          <div class="prop-foot">
            <span class="support">
              <strong>{p.support_count.toLocaleString('pt-BR')}</strong>
              {p.support_count === 1 ? 'apoio' : 'apoios'}
            </span>
            <time datetime={p.created_at} class="muted">{formatDate(p.created_at)}</time>
          </div>
        </a>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .list { list-style: none; padding: 0; margin: 0 0 3rem; }
  .prop { padding: 0; }
  .prop-link { display: block; padding: 1.25rem; text-decoration: none; color: inherit; }
  .prop-link:hover { background: var(--surface-2, var(--c-bg)); }
  .prop-title { font-size: 1.15rem; margin: 0 0 0.4rem; }
  .prop-body {
    margin: 0 0 1rem;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .prop-foot { display: flex; justify-content: space-between; align-items: center; font-size: 0.9rem; }
  .support strong { color: var(--c-green-dark, #15803d); font-size: 1.05rem; }
  .card {
    background: var(--surface-1, var(--c-paper));
    border: 1px solid var(--border-subtle, var(--c-border));
    border-radius: 12px;
  }
  .state { text-align: center; padding: 3rem 1.5rem; margin-bottom: 2rem; }
</style>
