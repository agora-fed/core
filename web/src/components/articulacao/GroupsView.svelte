<script lang="ts">
  import { onMount } from 'svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Input from '../ui/Input.svelte';
  import Failure from './Failure.svelte';
  import { href, isFailure, loadReference, normalize, type Failure as F, type Group } from '../../lib/articulacao';

  let groups = $state<Group[] | null>(null);
  let failure = $state<F | null>(null);
  let q = $state('');

  const shown = $derived((groups ?? []).filter((g) => !q.trim() || normalize(g.title).includes(normalize(q.trim()))));
  const sectionsList = [
    { kind: 'caucus', title: 'Bancadas' },
    { kind: 'front', title: 'Frentes parlamentares' },
  ] as const;

  async function load() {
    failure = null;
    const ref = await loadReference();
    if (isFailure(ref)) failure = ref;
    else groups = ref.groups;
  }
  onMount(load);
</script>

<header class="art-head"><h1>Frentes e bancadas</h1></header>
{#if failure}
  <Failure {failure} retry={load} />
{:else if !groups}
  <Skeleton variant="text" lines={8} />
{:else}
  <div class="filter"><Input label="Buscar" type="search" bind:value={q} placeholder="Nome da frente ou bancada" /></div>
  {#each sectionsList as s (s.kind)}
    <section class="block">
      <h2 class="h2">{s.title}</h2>
      <div class="art-table-wrap">
        <table class="art-table">
          <thead><tr><th>Nome</th><th>Em exercício</th><th>Assinaturas na criação</th><th>Fonte</th></tr></thead>
          <tbody>
            {#each shown.filter((g) => g.kind === s.kind) as g (g.id)}
              <tr>
                <td><a href={href.group(g.id)}>{g.title}</a></td>
                <td>{g.in_office_count}</td>
                <td>{g.official_total ?? '—'}</td>
                <td>{g.source === 'tse' ? 'TSE (autodeclaração)' : 'Câmara'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  {/each}
{/if}

<style>
  .filter {
    max-width: 28rem;
  }
  .block {
    margin-top: var(--sp-6);
  }
  .h2 {
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
  }
</style>
