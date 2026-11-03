<script lang="ts">
  // Lista de consultas carregada NO CLIENTE — reflete o banco em tempo real
  // (SSG mostraria consultas congeladas no build). getConsultas usa apiGet cru →
  // shape { ok, data, error }.
  import { onMount } from 'svelte';
  import { getConsultas } from '../../lib/api';
  import { formatDate } from '../../lib/format';
  import type { ConsultaSummary } from '../../lib/types';

  interface Props {
    kind?: 'consulta' | 'plebiscito';
  }
  let { kind }: Props = $props();

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<ConsultaSummary[]>([]);

  onMount(async () => {
    const res = await getConsultas(kind);
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
    <h2>{kind === 'plebiscito' ? 'Nenhum plebiscito publicado' : 'Nenhuma consulta publicada'}</h2>
    <p class="muted">{kind === 'plebiscito' ? 'Novos plebiscitos aparecerão aqui assim que abertos.' : 'Novas consultas públicas aparecerão aqui assim que abertas.'}</p>
  </div>
{:else}
  <ul class="list grid grid-2">
    {#each items as c (c.id)}
      <li class="card item" class:closed={c.status === 'closed'}>
        <a class="item-link" href={`/consulta/?id=${c.id}`}>
          <div class="row">
            <span>
              <span class="badge {c.status}">{c.status === 'open' ? (c.kind === 'consulta' ? 'Aberta' : 'Aberto') : (c.kind === 'consulta' ? 'Encerrada' : 'Encerrado')}</span>
              {#if c.kind === 'plebiscito'}<span class="badge sso">Membros</span>{/if}
            </span>
            <span class="muted small">{c.question_count} {c.question_count === 1 ? (c.kind === 'consulta' ? 'opção' : 'pergunta') : (c.kind === 'consulta' ? 'opções' : 'perguntas')}</span>
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
  .badge.kind { background: var(--c-purple-soft, #f3e8ff); color: var(--c-purple-dark, #7c3aed); margin-left: 0.25rem; }
  .badge.sso { background: var(--c-blue-soft, #dbeafe); color: var(--c-blue-dark, #1d4ed8); margin-left: 0.25rem; }
  .card {
    background: var(--surface-1, var(--c-paper));
    border: 1px solid var(--border-subtle, var(--c-border));
    border-radius: 12px;
  }
  .state { text-align: center; padding: 3rem 1.5rem; }
</style>
