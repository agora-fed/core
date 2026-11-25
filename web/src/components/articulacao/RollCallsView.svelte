<script lang="ts">
  import { onMount } from 'svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Badge from '../ui/Badge.svelte';
  import Card from '../ui/Card.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Failure from './Failure.svelte';
  import { api, failureOf, fmtDate, fmtTime, href, type Failure as F, type RollCall, type SyncRun } from '../../lib/articulacao';

  let rows = $state<RollCall[] | null>(null);
  let runs = $state<SyncRun[]>([]);
  let failure = $state<F | null>(null);

  const JOB: Record<string, string> = { deputies: 'deputados', groups: 'frentes e lideranças', roll_calls: 'votações' };

  async function load() {
    failure = null;
    const [res, st] = await Promise.all([api.rollCalls(), api.syncStatus()]);
    if (st.success && st.data) runs = st.data;
    if (res.success && res.data) rows = res.data;
    else failure = failureOf(res);
  }
  onMount(load);
</script>

<header class="art-head"><h1>Votações nominais do Plenário</h1></header>
{#if runs.length}
  <p class="muted small">
    Dados da Câmara dos Deputados e do TSE. Última atualização:
    {runs.map((r) => `${JOB[r.job] ?? r.job} ${fmtTime(r.finished_at ?? r.started_at)}`).join(' · ')}
  </p>
{/if}

{#if failure}
  <Failure {failure} retry={load} />
{:else if !rows}
  <Skeleton variant="text" lines={8} />
{:else if rows.length === 0}
  <Card padding="none"><EmptyState icon="ballot" title="Nenhuma votação sincronizada ainda" /></Card>
{:else}
  <div class="art-table-wrap">
    <table class="art-table">
      <thead><tr><th>Data</th><th>Proposição</th><th>Descrição</th><th>Votos</th></tr></thead>
      <tbody>
        {#each rows as r (r.id)}
          <tr>
            <td class="nowrap">{fmtDate(r.voted_on)}</td>
            <td class="nowrap">{r.bill_label ?? '—'}</td>
            <td><a href={href.rollCall(r.id)}>{r.description || r.id}</a></td>
            <td class="nowrap">
              {r.vote_count}
              {#if r.totals_match === false}<Badge tone="warning" size="sm">≠ total publicado</Badge>{/if}
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .small {
    font-size: var(--fs-sm);
  }
  .nowrap {
    white-space: nowrap;
  }
</style>
