<script lang="ts">
  // Who paid for the campaigns of the deputies in each column of a scenario, and the donors
  // that several deputies share (TSE 2022, diogenes ADR-0009). It loads when opened.
  import Skeleton from '../ui/Skeleton.svelte';
  import Badge from '../ui/Badge.svelte';
  import {
    api,
    brl,
    byColumnText,
    provenanceText,
    sourceLabel,
    type Column,
    type ScenarioMoney,
  } from '../../lib/articulacao';

  let { scenarioId, columns }: { scenarioId: string; columns: Column[] } = $props();

  let data = $state<ScenarioMoney | null>(null);
  let error = $state('');
  let loadedFor = $state('');

  async function load() {
    if (loadedFor === scenarioId) return;
    loadedFor = scenarioId;
    data = null;
    error = '';
    const res = await api.scenarioMoney(scenarioId);
    if (res.success && res.data) data = res.data;
    else error = res.error?.message ?? 'Não foi possível carregar.';
  }

  // A board move changes the columns: closing the panel forgets the data, so the next
  // opening loads the current board.
  let open = $state(false);
  $effect(() => {
    if (open && scenarioId) void load();
    else if (!open) loadedFor = '';
  });

  const nameOf = (id: string) => columns.find((c) => c.id === id)?.name ?? '—';
  const money = $derived(
    (data?.columns ?? [])
      .filter((c) => c.deputies > 0)
      .map((c) => {
        const total = Object.values(c.cents_by_source).reduce((n, v) => n + (v ?? 0), 0);
        const top = Object.entries(c.cents_by_source).sort((a, b) => (b[1] ?? 0) - (a[1] ?? 0))[0];
        return { ...c, name: nameOf(c.column_id), total, top };
      }),
  );
</script>

<details class="panel" bind:open>
  <summary>Quem financiou as campanhas deste cenário <span class="muted">(TSE)</span></summary>
  {#if error}
    <p class="muted small">{error}</p>
  {:else if !data}
    <Skeleton variant="block" height="6rem" />
  {:else if !data.imported_at}
    <p class="muted small">{provenanceText(data)}</p>
  {:else}
    <div class="table-wrap">
      <table>
        <caption class="sr-only">Receitas de campanha por coluna</caption>
        <thead>
          <tr><th scope="col">Coluna</th><th scope="col">Deputados</th><th scope="col">Total declarado</th><th scope="col">Maior origem</th></tr>
        </thead>
        <tbody>
          {#each money as c (c.column_id)}
            <tr>
              <th scope="row">{c.name}</th>
              <td>{c.deputies}{c.with_accounts < c.deputies ? ` (${c.with_accounts} com contas)` : ''}</td>
              <td class="num">{brl(c.total)}</td>
              <td>{c.top ? `${sourceLabel(c.top[0])} · ${brl(c.top[1] ?? 0)}` : '—'}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>

    <h3 class="h3">Doadores em comum</h3>
    {#if data.donors.length === 0}
      <p class="muted small">Nenhum doador financiou dois ou mais destes deputados.</p>
    {:else}
      <p class="muted small">Pessoas e campanhas que financiaram dois ou mais deputados, e em que coluna cada um deles está neste cenário.</p>
      <ol class="donors">
        {#each data.donors as d (d.key)}
          <li>
            <span class="name">{d.name}</span>
            {#if d.kind === 'candidate'}<Badge size="sm">campanha</Badge>{/if}
            <span class="muted small">— {d.deputies} deputados · {brl(d.cents)}</span>
            <div class="cols">{byColumnText(d.by_column, columns)}</div>
            {#if d.activity}<div class="muted small">{d.activity}</div>{/if}
          </li>
        {/each}
      </ol>
    {/if}
    <p class="muted small source">
      {provenanceText(data)} · <a href={data.source_url} target="_blank" rel="noopener noreferrer">dados abertos ↗</a>.
      Inclui quem doou a um partido que repassou o dinheiro ao deputado. Doação declarada é um fato público; sozinha, não
      prova influência sobre o voto.
    </p>
  {/if}
</details>

<style>
  .panel {
    margin-top: var(--sp-4);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-md);
    padding: var(--sp-3) var(--sp-4);
  }
  summary {
    cursor: pointer;
    font-weight: var(--fw-semibold);
  }
  .small {
    font-size: var(--fs-sm);
  }
  .h3 {
    font-size: var(--fs-base);
    margin: var(--sp-4) 0 var(--sp-1);
  }
  .table-wrap {
    overflow-x: auto;
    margin-top: var(--sp-3);
  }
  table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  th,
  td {
    text-align: left;
    padding: var(--sp-1) var(--sp-2);
    border-bottom: 1px solid var(--border-subtle);
    vertical-align: top;
  }
  .num {
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .donors {
    margin: var(--sp-2) 0 0;
    padding-left: var(--sp-5);
    display: grid;
    gap: var(--sp-2);
  }
  .donors li {
    font-size: var(--fs-sm);
  }
  .name {
    font-weight: var(--fw-semibold);
    overflow-wrap: anywhere;
  }
  .cols {
    font-variant-numeric: tabular-nums;
  }
  .source {
    margin-top: var(--sp-3);
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip: rect(0, 0, 0, 0);
    white-space: nowrap;
  }
</style>
