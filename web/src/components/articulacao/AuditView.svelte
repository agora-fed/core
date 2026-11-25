<script lang="ts">
  import { onMount } from 'svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Card from '../ui/Card.svelte';
  import Failure from './Failure.svelte';
  import {
    api,
    auditDetail,
    auditLabel,
    failureOf,
    fmtTime,
    href,
    isFailure,
    loadReference,
    type AuditRow,
    type Failure as F,
  } from '../../lib/articulacao';

  let { id }: { id: string } = $props();
  let rows = $state<AuditRow[] | null>(null);
  let names = $state(new Map<number, string>());
  let failure = $state<F | null>(null);

  async function load() {
    failure = null;
    const [res, ref] = await Promise.all([api.audit(id), loadReference()]);
    if (!isFailure(ref)) names = new Map(ref.deputies.map((d) => [d.camara_id, d.name]));
    if (res.success && res.data) rows = res.data;
    else failure = failureOf(res);
  }
  onMount(load);
</script>

<a class="art-back" href={href.room(id)}>← Sala</a>
<header class="art-head"><h1>Histórico de mudanças</h1></header>

{#if failure}
  <Failure {failure} retry={load} />
{:else if !rows}
  <Skeleton variant="text" lines={6} />
{:else if rows.length === 0}
  <Card padding="none"><EmptyState title="Nada registrado ainda" /></Card>
{:else}
  <div class="art-table-wrap">
    <table class="art-table">
      <thead><tr><th>Quando</th><th>Quem</th><th>O quê</th><th>Detalhe</th></tr></thead>
      <tbody>
        {#each rows as r (r.id)}
          <tr>
            <td class="nowrap">{fmtTime(r.at)}</td>
            <td>{r.actor_name ?? 'ex-membro'}</td>
            <td>{auditLabel(r.action)}</td>
            <td class="muted">{auditDetail(r, names)}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .nowrap {
    white-space: nowrap;
  }
</style>
