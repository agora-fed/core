<script lang="ts">
  // Cartão de estatísticas ao vivo na landing. Puxa GET /stats/public
  // (não credenciado, cacheável) e mostra 4-5 números grandes com labels
  // curtos. Sem PII. Ancora a tese: a plataforma é medida pela quantidade
  // de accountability, não pelo engajamento.
  import { onMount } from 'svelte';
  import { getPublicStats, type PublicStats } from '../../lib/api';

  let stats = $state<PublicStats | null>(null);
  let error = $state(false);

  onMount(async () => {
    const r = await getPublicStats();
    if (r.ok && r.data) stats = r.data;
    else error = true;
  });

  function fmt(n: number): string {
    return new Intl.NumberFormat('pt-BR').format(n);
  }
</script>

{#if stats && !error}
  <div class="stats" role="region" aria-label="Estatísticas ao vivo">
    <div class="stat">
      <strong>{fmt(stats.mandates_indexed)}</strong>
      <span>mandatos catalogados</span>
    </div>
    <div class="stat">
      <strong>{fmt(stats.citizens_active)}</strong>
      <span>cidadã(o)s verificadas(os)</span>
    </div>
    <div class="stat">
      <strong>{fmt(stats.proposals_published)}</strong>
      <span>propostas públicas</span>
    </div>
    {#if stats.responses_public + stats.silences_public > 0}
      <div class="stat" data-tone="ok">
        <strong>{fmt(stats.responses_public)}</strong>
        <span>respostas dentro do prazo</span>
      </div>
      <div class="stat" data-tone="bad">
        <strong>{fmt(stats.silences_public)}</strong>
        <span>silêncios públicos registrados</span>
      </div>
    {/if}
  </div>
{/if}

<style>
  .stats {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 0.75rem;
    margin: 0 auto;
    max-width: 64rem;
  }
  .stat {
    padding: 1rem 1.25rem;
    background: var(--surface-1, white);
    border: 1px solid var(--border-subtle, #e5e7eb);
    border-radius: 12px;
    text-align: center;
  }
  .stat strong {
    display: block;
    font-size: 2rem;
    line-height: 1.1;
    font-variant-numeric: tabular-nums;
    color: var(--text-1, #111);
  }
  .stat span {
    display: block;
    font-size: 0.85rem;
    color: var(--text-2, #666);
    margin-top: 4px;
  }
  .stat[data-tone='ok'] {
    background: var(--c-green-soft, #e6f7ed);
    border-color: #b7e4c7;
  }
  .stat[data-tone='ok'] strong {
    color: #115c2d;
  }
  .stat[data-tone='bad'] {
    background: #fff1f0;
    border-color: #ffa39e;
  }
  .stat[data-tone='bad'] strong {
    color: #a8071a;
  }
</style>
