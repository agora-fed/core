<script lang="ts">
  import { onMount } from 'svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Badge from '../ui/Badge.svelte';
  import Failure from './Failure.svelte';
  import { api, camaraUrl, failureOf, href, safeUrl, type Failure as F, type GroupDetail } from '../../lib/articulacao';

  let { id }: { id: string } = $props();
  let g = $state<GroupDetail | null>(null);
  let failure = $state<F | null>(null);

  async function load() {
    failure = null;
    const res = await api.group(id);
    if (res.success && res.data) g = res.data;
    else failure = failureOf(res);
  }
  onMount(load);
</script>

<a class="art-back" href={href.groups()}>← Frentes e bancadas</a>
{#if failure}
  <Failure {failure} retry={load} />
{:else if !g}
  <Skeleton variant="text" lines={8} />
{:else}
  <header class="art-head"><h1>{g.title}</h1></header>
  <p class="facts">
    <Badge size="sm">{g.in_office_count} em exercício</Badge>
    <Badge size="sm">{g.member_count} registrados</Badge>
    {#if g.official_total !== null}<Badge size="sm">{g.official_total} assinaturas na criação</Badge>{/if}
    {#if g.details?.signatures_net}<Badge size="sm">saldo de adesões: {g.details.signatures_net}</Badge>{/if}
    <Badge size="sm" tone="info">{g.source === 'tse' ? 'Fonte: autodeclaração ao TSE (2022)' : 'Fonte: Câmara dos Deputados'}</Badge>
  </p>
  {#if safeUrl(g.details?.document_url)}
    <p><a href={safeUrl(g.details?.document_url)} target="_blank" rel="noopener noreferrer">Documento oficial ↗</a></p>
  {/if}
  <div class="art-table-wrap">
    <table class="art-table">
      <thead><tr><th>Deputado</th><th>Papel</th><th>Situação</th></tr></thead>
      <tbody>
        {#each g.members as m, i (`${m.camara_id}-${i}`)}
          <tr>
            <td><a href={camaraUrl(m.camara_id)} target="_blank" rel="noopener noreferrer">{m.name ?? `#${m.camara_id}`}</a></td>
            <td>{m.role ?? ''}</td>
            <td>{m.in_office ? 'em exercício' : 'fora de exercício'}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}

<style>
  .facts {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
  }
</style>
