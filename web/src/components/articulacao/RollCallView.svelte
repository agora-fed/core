<script lang="ts">
  import { onMount } from 'svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Badge from '../ui/Badge.svelte';
  import Input from '../ui/Input.svelte';
  import Failure from './Failure.svelte';
  import {
    api,
    failureOf,
    fmtDate,
    href,
    normalize,
    orientationsText,
    type Failure as F,
    type RollCallDetail,
  } from '../../lib/articulacao';

  let { id }: { id: string } = $props();
  let rc = $state<RollCallDetail | null>(null);
  let failure = $state<F | null>(null);
  let q = $state('');

  const dissidents = $derived(new Set(rc?.dissidents ?? []));
  const votes = $derived(
    (rc?.votes ?? []).filter((v) => !q.trim() || normalize(`${v.name} ${v.party} ${v.uf} ${v.vote}`).includes(normalize(q.trim()))),
  );

  async function load() {
    failure = null;
    const res = await api.rollCall(id);
    if (res.success && res.data) rc = res.data;
    else failure = failureOf(res);
  }
  onMount(load);
</script>

<a class="art-back" href={href.rollCalls()}>← Votações</a>
{#if failure}
  <Failure {failure} retry={load} />
{:else if !rc}
  <Skeleton variant="text" lines={8} />
{:else}
  <header class="art-head"><h1>{rc.bill_label ?? rc.id}</h1></header>
  <p>{rc.description} <span class="muted">· {fmtDate(rc.voted_on)}</span></p>
  {#if rc.bill_summary}<p class="muted small">{rc.bill_summary}</p>{/if}
  <p class="counts">
    {#each Object.entries(rc.counts) as [k, n] (k)}<Badge size="sm">{k}: {n}</Badge>{/each}
    {#if rc.totals_match === true}<Badge tone="success" size="sm">confere com o total publicado</Badge>{/if}
  </p>
  {#if orientationsText(rc.orientations)}<p class="small">Orientações: {orientationsText(rc.orientations)}</p>{/if}
  <p class="small">{rc.dissidents.length} votos contra a orientação do partido ou bloco.</p>
  <div class="filter"><Input label="Filtrar votos" type="search" bind:value={q} placeholder="Nome, partido, UF ou voto" /></div>
  <div class="art-table-wrap">
    <table class="art-table">
      <thead><tr><th>Deputado</th><th>Partido/UF</th><th>Voto</th><th></th></tr></thead>
      <tbody>
        {#each votes as v (v.camara_id)}
          <tr>
            <td>{v.name}</td>
            <td>{v.party}/{v.uf}</td>
            <td>{v.vote}</td>
            <td>{#if dissidents.has(v.camara_id)}<Badge tone="warning" size="sm">contra a orientação</Badge>{/if}</td>
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
  .counts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
  }
  .filter {
    max-width: 28rem;
  }
</style>
