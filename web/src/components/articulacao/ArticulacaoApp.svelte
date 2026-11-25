<script lang="ts">
  // Hash router of the articulation area. The links of the first version keep working.
  import { onMount } from 'svelte';
  import { parseRoute, href, type View } from '../../lib/articulacao';
  import RoomsView from './RoomsView.svelte';
  import RoomView from './RoomView.svelte';
  import AuditView from './AuditView.svelte';
  import ProjectView from './ProjectView.svelte';
  import RollCallsView from './RollCallsView.svelte';
  import RollCallView from './RollCallView.svelte';
  import GroupsView from './GroupsView.svelte';
  import GroupView from './GroupView.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Card from '../ui/Card.svelte';

  let view = $state<View>({ name: 'rooms' });
  let ready = $state(false);

  const sync = () => {
    view = parseRoute(window.location.hash);
    window.scrollTo({ top: 0 });
  };
  onMount(() => {
    sync();
    ready = true;
    window.addEventListener('hashchange', sync);
    return () => window.removeEventListener('hashchange', sync);
  });

  const section = $derived(
    view.name === 'roll-calls' || view.name === 'roll-call'
      ? 'votacoes'
      : view.name === 'groups' || view.name === 'group'
        ? 'grupos'
        : 'salas',
  );
  const tabs = [
    { id: 'salas', href: href.rooms(), label: 'Salas' },
    { id: 'votacoes', href: href.rollCalls(), label: 'Votações nominais' },
    { id: 'grupos', href: href.groups(), label: 'Frentes e bancadas' },
  ];
</script>

<nav class="subnav" aria-label="Seções da sala de articulação">
  {#each tabs as t (t.id)}
    <a href={t.href} class="tab" class:active={section === t.id} aria-current={section === t.id ? 'page' : undefined}>
      {t.label}
    </a>
  {/each}
</nav>

{#if !ready}
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else if view.name === 'rooms'}
  <RoomsView />
{:else if view.name === 'room'}
  {#key view.id}<RoomView id={view.id} />{/key}
{:else if view.name === 'audit'}
  {#key view.id}<AuditView id={view.id} />{/key}
{:else if view.name === 'project'}
  {#key view.id}<ProjectView id={view.id} scenarioId={view.scenario} />{/key}
{:else if view.name === 'roll-calls'}
  <RollCallsView />
{:else if view.name === 'roll-call'}
  {#key view.id}<RollCallView id={view.id} />{/key}
{:else if view.name === 'groups'}
  <GroupsView />
{:else if view.name === 'group'}
  {#key view.id}<GroupView id={view.id} />{/key}
{:else}
  <Card padding="none"><EmptyState title="Página não encontrada" description="Volte para a lista de salas." /></Card>
{/if}

<style>
  .subnav {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
    margin-bottom: var(--sp-6);
    border-bottom: 1px solid var(--border-subtle);
  }
  .tab {
    padding: var(--sp-2) var(--sp-3);
    margin-bottom: -1px;
    text-decoration: none;
    color: var(--text-2);
    font-weight: var(--fw-medium);
    font-size: var(--fs-sm);
    white-space: nowrap;
    border-bottom: 2px solid transparent;
  }
  .tab:hover {
    color: var(--text-1);
  }
  .tab.active {
    color: var(--accent-strong);
    border-bottom-color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  /* Shared by the views of this area. */
  :global(.art-head) {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-5);
  }
  :global(.art-head h1) {
    margin: 0;
    font-size: var(--fs-2xl);
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }
  :global(.art-back) {
    font-size: var(--fs-sm);
    color: var(--text-3);
    text-decoration: none;
  }
  :global(.art-table-wrap) {
    overflow-x: auto;
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    background: var(--surface-1);
  }
  :global(.art-table) {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  :global(.art-table th),
  :global(.art-table td) {
    text-align: left;
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid var(--border-subtle);
    vertical-align: top;
  }
  :global(.art-table th) {
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.03em;
    color: var(--text-3);
    background: var(--surface-2);
  }
  :global(.art-table tr:last-child td) {
    border-bottom: 0;
  }
  :global(.art-check) {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-sm);
    cursor: pointer;
  }
</style>
