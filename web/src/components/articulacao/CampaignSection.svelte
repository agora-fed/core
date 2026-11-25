<script lang="ts">
  // Campaign finance of one deputy (TSE 2022, diogenes ADR-0009): money by source and the
  // largest donors. Declared facts with their source; the page never labels a person.
  import { onMount } from 'svelte';
  import Badge from '../ui/Badge.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import {
    api,
    brl,
    donorKindText,
    provenanceText,
    sourceShares,
    type Campaign,
  } from '../../lib/articulacao';

  let { camaraId }: { camaraId: number } = $props();

  let data = $state<Campaign | null>(null);
  let error = $state('');

  onMount(async () => {
    const res = await api.campaign(camaraId);
    if (res.success && res.data) data = res.data;
    else error = res.error?.message ?? 'Não foi possível carregar.';
  });
</script>

<section class="campaign" aria-label="Financiamento de campanha">
  <h3 class="h3">Financiamento de campanha</h3>
  {#if error}
    <p class="muted small">{error}</p>
  {:else if !data}
    <Skeleton variant="block" height="5rem" />
  {:else if !data.imported_at}
    <p class="muted small">{provenanceText(data)}</p>
  {:else if data.total_cents === 0}
    <p class="muted small">Sem receitas declaradas ao TSE para a eleição de {data.year}.</p>
  {:else}
    <p class="total"><b>{brl(data.total_cents)}</b> <span class="muted small">declarados na eleição de {data.year}</span></p>
    <ul class="shares">
      {#each sourceShares(data.totals) as s (s.source)}
        <li>
          <span class="label">{s.label}</span>
          <span class="bar" aria-hidden="true"><span style:width={`${s.pct}%`}></span></span>
          <span class="value">{brl(s.cents)} <span class="muted">({s.pct.toLocaleString('pt-BR')}%)</span></span>
        </li>
      {/each}
    </ul>
    {#if data.donors.length}
      <h4 class="h4">Maiores doadores</h4>
      <ol class="donors">
        {#each data.donors as d (`${d.key}-${d.via_party}`)}
          <li>
            <span class="name">{d.name}</span>
            <span class="value">{brl(d.cents)}</span>
            <span class="meta">
              <Badge size="sm" tone={d.via_party ? 'warning' : 'neutral'}>{donorKindText(d)}</Badge>
              {#if d.activity}<span class="muted small">{d.activity}</span>{/if}
            </span>
          </li>
        {/each}
      </ol>
    {/if}
    <p class="muted small source">
      {provenanceText(data)} · <a href={data.source_url} target="_blank" rel="noopener noreferrer">dados abertos ↗</a>.
      Doação declarada é um fato público; sozinha, não prova influência sobre o voto.
    </p>
  {/if}
</section>

<style>
  .campaign {
    margin: var(--sp-4) 0;
  }
  .h3 {
    font-size: var(--fs-base);
    margin: 0 0 var(--sp-2);
  }
  .h4 {
    font-size: var(--fs-sm);
    margin: var(--sp-3) 0 var(--sp-2);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .total {
    margin: 0 0 var(--sp-2);
  }
  .shares,
  .donors {
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-1);
  }
  .shares {
    list-style: none;
  }
  .shares li {
    display: grid;
    grid-template-columns: minmax(8rem, 1fr) minmax(4rem, 1fr) auto;
    gap: var(--sp-2);
    align-items: center;
    font-size: var(--fs-sm);
  }
  .bar {
    height: 0.5rem;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--accent);
  }
  .value {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .donors {
    padding-left: var(--sp-5);
  }
  .donors li {
    font-size: var(--fs-sm);
  }
  .donors .name {
    font-weight: var(--fw-semibold);
    overflow-wrap: anywhere;
  }
  .donors .value {
    margin-left: var(--sp-2);
  }
  .meta {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
    align-items: center;
    margin-top: 2px;
  }
  .source {
    margin-top: var(--sp-3);
  }
  @media (max-width: 480px) {
    .shares li {
      grid-template-columns: 1fr auto;
    }
    .bar {
      display: none;
    }
  }
</style>
