<script lang="ts">
  // Lista de consultas carregada NO CLIENTE — reflete o banco em tempo real
  // (SSG mostraria consultas congeladas no build). getConsultas usa apiGet cru →
  // shape { ok, data, error }.
  import { onMount } from 'svelte';
  import { getConsultas } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import type { ConsultaSummary } from '../../lib/types';

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<ConsultaSummary[]>([]);

  onMount(async () => {
    const res = await getConsultas();
    loading = false;
    if (res.ok && res.data) items = res.data;
    else error = res.error ?? 'Não foi possível carregar as consultas.';
  });
</script>

{#if loading}
  <p class="muted">Carregando consultas…</p>
{:else if error}
  <div class="card state" role="alert">
    <h2>Não foi possível carregar as consultas</h2>
    <p class="muted">{error}</p>
  </div>
{:else if items.length === 0}
  <div class="card state">
    <h2>Nenhuma consulta publicada</h2>
    <p class="muted">Novas consultas públicas aparecerão aqui assim que abertas.</p>
  </div>
{:else}
  <ul class="list grid grid-2">
    {#each items as c (c.id)}
      <li class="card item" class:closed={c.status === 'closed'}>
        <a class="item-link" href={`/consulta/?id=${c.id}`}>
          <div class="row">
            <span class="badge {c.status}">{c.status === 'open' ? 'Aberta' : 'Encerrada'}</span>
            <span class="muted small">{c.question_count} {c.question_count === 1 ? 'pergunta' : 'perguntas'}</span>
          </div>
          <h2 class="item-title">{c.title}</h2>
          <p class="muted small">
            {c.status === 'open' ? 'Encerra em' : 'Encerrou em'} {formatDate(c.closes_at)}
          </p>
        </a>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .list { list-style: none; padding: 0; margin: 0; }
  .item { padding: 0; }
  .item:hover { transform: translateY(-2px); box-shadow: 0 6px 16px rgba(0,0,0,0.07); transition: transform 100ms ease, box-shadow 100ms ease; }
  .item.closed { opacity: 0.72; }
  .item-link { display: block; padding: 1.25rem; text-decoration: none; color: inherit; }
  .row { display: flex; justify-content: space-between; align-items: center; margin-bottom: 0.5rem; }
  .item-title { font-size: 1.15rem; margin: 0 0 0.35rem; }
  .small { font-size: 0.85rem; }
  .badge { font-size: 0.72rem; font-weight: 700; text-transform: uppercase; letter-spacing: 0.03em; padding: 0.15rem 0.5rem; border-radius: 999px; }
  .badge.open { background: var(--c-green-soft, #dcfce7); color: var(--c-green-dark, #15803d); }
  .badge.closed { background: var(--c-bg, #f1f5f9); color: var(--muted, #64748b); }
  .card {
    background: var(--surface-1, var(--c-paper));
    border: 1px solid var(--border-subtle, var(--c-border));
    border-radius: 12px;
  }
  .state { text-align: center; padding: 3rem 1.5rem; }
</style>
