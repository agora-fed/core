<script lang="ts">
  import { onMount } from 'svelte';
  import {
    addPhase,
    advancePhase,
    createProcess,
    getProcess,
    listPhases,
    listProcesses,
    nextPhasePosition,
    sortPhases,
    statusLabel,
    type PhaseDto,
    type ProcessDto,
  } from '../../lib/processes';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let logged = $state(false);
  let processes = $state<ProcessDto[]>([]);
  let current = $state<ProcessDto | null>(null);
  let phases = $state<PhaseDto[]>([]);
  let busy = $state(false);
  let newName = $state('');

  async function loadDetail(id: string) {
    const [process, items] = await Promise.all([getProcess(id), listPhases(id)]);
    if (process.ok && process.data) {
      current = process.data;
      phases = sortPhases(items.data ?? []);
    } else {
      loadError = process.error ?? 'Processo não encontrado.';
    }
  }

  onMount(async () => {
    try {
      logged = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
    const id = new URLSearchParams(window.location.search).get('id');
    if (id) {
      await loadDetail(id);
    } else {
      const res = await listProcesses();
      if (res.ok && res.data) processes = res.data;
      else loadError = res.error ?? 'Não foi possível carregar os processos.';
    }
    loading = false;
  });

  async function create(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    const res = await createProcess(newName);
    busy = false;
    if (res.success && res.data) window.location.href = `/processos/?id=${res.data.id}`;
    else toast.error(res.error?.message ?? 'Não foi possível criar o processo.');
  }

  async function addNewPhase(event: SubmitEvent) {
    event.preventDefault();
    if (!current) return;
    busy = true;
    const res = await addPhase(current.id, newName, nextPhasePosition(phases));
    busy = false;
    if (res.success) {
      newName = '';
      await loadDetail(current.id);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível adicionar a etapa.');
    }
  }

  async function advance() {
    if (!current) return;
    busy = true;
    const res = await advancePhase(current.id);
    busy = false;
    if (res.success) await loadDetail(current.id);
    else toast.error(res.error?.message ?? 'Não foi possível avançar a etapa.');
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <Card><p role="alert">{loadError}</p></Card>
{:else if current}
  <p><a href="/processos/">← Todos os processos</a></p>
  <h1>{current.title}</h1>
  <p class="muted">{statusLabel(current.status)}</p>

  <h2>Etapas</h2>
  {#if phases.length === 0}
    <p class="muted">Nenhuma etapa cadastrada.</p>
  {:else}
    <ol class="phases">
      {#each phases as p (p.id)}
        <li aria-current={p.active ? 'step' : undefined} class:active={p.active}>
          {p.name}{p.active ? ' (etapa atual)' : ''}
        </li>
      {/each}
    </ol>
  {/if}

  {#if logged}
    <Button disabled={busy || phases.length === 0} onclick={advance}>Avançar etapa</Button>
    <form onsubmit={addNewPhase}>
      <label>Nova etapa <input bind:value={newName} required /></label>
      <Button type="submit" variant="ghost" disabled={busy}>Adicionar etapa</Button>
    </form>
  {/if}
{:else}
  <h1>Processos participativos</h1>
  {#if processes.length === 0}
    <p class="muted">Nenhum processo por enquanto.</p>
  {:else}
    <ul class="list">
      {#each processes as p (p.id)}
        <li>
          <a href={`/processos/?id=${p.id}`}>{p.title}</a>
          <span class="muted">{statusLabel(p.status)}</span>
        </li>
      {/each}
    </ul>
  {/if}
  {#if logged}
    <Card>
      <h2>Criar processo</h2>
      <form onsubmit={create}>
        <label>Título <input bind:value={newName} required /></label>
        <Button type="submit" disabled={busy}>Criar</Button>
      </form>
    </Card>
  {/if}
{/if}

<style>
  .list {
    list-style: none;
    margin: 0 0 var(--sp-4);
    padding: 0;
  }
  .list li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--border-subtle);
  }
  .phases .active {
    font-weight: var(--fw-medium);
  }
  form {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--sp-3);
    margin-top: var(--sp-3);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    width: 100%;
  }
</style>
