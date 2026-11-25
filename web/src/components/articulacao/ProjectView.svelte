<script lang="ts">
  // One project: scenarios, the count, filters, the board, the selection bar, live updates.
  import { onMount } from 'svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Select from '../ui/Select.svelte';
  import Badge from '../ui/Badge.svelte';
  import Card from '../ui/Card.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Failure from './Failure.svelte';
  import Scoreboard from './Scoreboard.svelte';
  import Board from './Board.svelte';
  import DeputyModal from './DeputyModal.svelte';
  import ScenarioModal from './ScenarioModal.svelte';
  import SettingsModal from './SettingsModal.svelte';
  import PasteModal from './PasteModal.svelte';
  import ColumnModal from './ColumnModal.svelte';
  import DonorsPanel from './DonorsPanel.svelte';
  import { toast } from '../../lib/toasts';
  import {
    EMPTY_FILTER,
    api,
    eventsUrl,
    failureOf,
    filterDeputies,
    href,
    isFailure,
    loadReference,
    moveBody,
    movedPositions,
    positionsMap,
    selectIds,
    undecidedId,
    type Column,
    type Failure as F,
    type Filter,
    type GroupBy,
    type Project,
    type Reference,
    type Scenario,
  } from '../../lib/articulacao';

  let { id, scenarioId }: { id: string; scenarioId: string | null } = $props();

  let project = $state<Project | null>(null);
  let scenario = $state<Scenario | null>(null);
  let ref = $state<Reference | null>(null);
  let failure = $state<F | null>(null);
  let filter = $state<Filter>({ ...EMPTY_FILTER });
  let groupBy = $state<GroupBy>('none');
  let selected = $state(new Set<number>());
  let lastClicked = $state<number | null>(null);
  let target = $state('');
  let deputyId = $state<number | null>(null);
  let modal = $state<'scenario' | 'settings' | 'paste' | 'column' | null>(null);
  let editing = $state<Column | null>(null);

  const canEdit = $derived(project !== null && project.role !== 'viewer');
  const priorities = $derived(new Set(project?.priorities ?? []));
  const dissidents = $derived(new Set(scenario?.dissidents ?? []));
  const visible = $derived(ref && scenario ? filterDeputies(ref.deputies, filter, priorities, dissidents) : []);
  const und = $derived(scenario ? undecidedId(scenario.columns) : '');
  const deputy = $derived(deputyId !== null ? ref?.byId.get(deputyId) ?? null : null);

  const parties = $derived([...new Set(ref?.deputies.map((d) => d.party) ?? [])].sort());
  const ufs = $derived([...new Set(ref?.deputies.map((d) => d.uf) ?? [])].sort());
  const groupOptions = $derived([
    { value: '', label: 'Todas as frentes e bancadas' },
    ...(ref?.groups ?? [])
      .filter((g) => g.in_office_count > 0)
      .map((g) => ({
        value: g.id,
        label: `${g.title} (${g.in_office_count})`,
        group: g.kind === 'caucus' ? 'Bancadas' : 'Frentes',
      })),
  ]);

  async function reload(sid: string | null = scenario?.id ?? scenarioId, quiet = false) {
    const res = await api.project(id);
    if (!res.success || !res.data) {
      if (quiet) toast.error(res.error?.message ?? 'Não foi possível atualizar.');
      else failure = failureOf(res);
      return;
    }
    project = res.data;
    const pick = sid && res.data.scenarios.some((s) => s.id === sid) ? sid : res.data.scenarios[0]?.id;
    if (!pick) {
      scenario = null;
      return;
    }
    const sc = await api.scenario(pick);
    if (sc.success && sc.data) scenario = sc.data;
    else if (!quiet) failure = failureOf(sc);
  }

  onMount(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    let source: EventSource | null = null;
    (async () => {
      const r = await loadReference();
      if (isFailure(r)) {
        failure = r;
        return;
      }
      ref = r;
      await reload(scenarioId);
      if (failure) return;
      source = new EventSource(eventsUrl(id), { withCredentials: true });
      const refresh = () => {
        clearTimeout(timer);
        timer = setTimeout(() => reload(scenario?.id ?? null, true), 300);
      };
      source.addEventListener('change', refresh);
      source.addEventListener('reload', refresh);
    })();
    return () => {
      clearTimeout(timer);
      source?.close();
    };
  });

  function choose(sid: string) {
    window.location.hash = href.project(id, sid);
  }

  async function move(ids: number[], columnId: string) {
    if (!scenario || !ids.length) return;
    const sc = scenario;
    scenario = { ...sc, positions: movedPositions(sc.positions, ids, columnId, und) };
    selected = new Set();
    const res = await api.move(sc.id, moveBody(ids, columnId, und));
    if (!res.success) toast.error(res.error?.message ?? 'Não foi possível mover.');
    await reload(sc.id, true);
  }

  async function setPriority(ids: number[], value: boolean) {
    const res = await api.setPriorities(id, ids, value);
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível alterar os prioritários.');
      return;
    }
    selected = new Set();
    await reload(scenario?.id ?? null, true);
  }

  async function addNote(body: string, isPrivate: boolean) {
    if (deputyId === null) return false;
    const res = await api.addNote(id, deputyId, body, isPrivate);
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível salvar a anotação.');
      return false;
    }
    await reload(scenario?.id ?? null, true);
    return true;
  }

  async function deleteNote(noteId: string) {
    if (!confirm('Excluir esta anotação?')) return;
    const res = await api.deleteNote(noteId);
    if (!res.success) toast.error(res.error?.message ?? 'Não foi possível excluir.');
    await reload(scenario?.id ?? null, true);
  }

  async function renameScenario() {
    if (!scenario) return;
    const name = prompt('Novo nome do cenário:', scenario.name)?.trim();
    if (!name) return;
    const res = await api.renameScenario(scenario.id, name);
    if (!res.success) toast.error(res.error?.message ?? 'Não foi possível renomear.');
    await reload(scenario.id, true);
  }

  async function deleteScenario() {
    if (!scenario || !confirm(`Excluir o cenário "${scenario.name}"?`)) return;
    const res = await api.deleteScenario(scenario.id);
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível excluir.');
      return;
    }
    window.location.hash = href.project(id);
    await reload(null, true);
  }

  const closeModal = () => {
    modal = null;
    editing = null;
  };
</script>

{#if failure}
  <a class="art-back" href={href.rooms()}>← Salas</a>
  <Failure {failure} retry={() => location.reload()} />
{:else if !project || !ref}
  <Skeleton variant="block" height="10rem" />
{:else}
  <a class="art-back" href={href.room(project.room_id)}>← Sala</a>
  <header class="art-head">
    <h1>{project.name}</h1>
    {#if canEdit}
      <Button variant="secondary" size="sm" onclick={() => (modal = 'paste')}>Colar lista</Button>
      <Button variant="ghost" size="sm" onclick={() => (modal = 'settings')}>Configurar</Button>
    {/if}
  </header>

  <div class="scenarios" role="tablist" aria-label="Cenários">
    {#each project.scenarios as s (s.id)}
      <button type="button" role="tab" class="scn" aria-selected={scenario?.id === s.id} onclick={() => choose(s.id)}>
        {s.name} <Badge size="sm" tone={scenario?.id === s.id ? 'accent' : 'neutral'}>{s.yes} sim</Badge>
      </button>
    {/each}
    {#if canEdit}<Button variant="ghost" size="sm" onclick={() => (modal = 'scenario')}>+ Cenário</Button>{/if}
  </div>

  {#if !scenario}
    <Card padding="none">
      <EmptyState
        icon="ballot"
        title="Nenhum cenário ainda"
        description={canEdit ? 'Crie o primeiro cenário: em branco, ou a partir de uma votação real.' : 'Este projeto ainda não tem cenários.'}
      >
        {#snippet action()}
          {#if canEdit}<Button variant="primary" onclick={() => (modal = 'scenario')}>Criar cenário</Button>{/if}
        {/snippet}
      </EmptyState>
    </Card>
  {:else}
    <Scoreboard {scenario} priorities={project.priorities} />

    {#if canEdit}
      <div class="tools">
        {#if scenario.source_roll_call}
          <span class="muted small">Origem: <a href={href.rollCall(scenario.source_roll_call)}>votação {scenario.source_roll_call}</a></span>
        {/if}
        <span class="spacer"></span>
        <Button variant="ghost" size="sm" onclick={renameScenario}>Renomear cenário</Button>
        <Button variant="ghost" size="sm" onclick={() => ((editing = null), (modal = 'column'))}>+ Coluna</Button>
        <Button variant="ghost" size="sm" onclick={deleteScenario}>Excluir cenário</Button>
      </div>
    {/if}

    <div class="filters">
      <div class="f-search"><Input label="Buscar" type="search" bind:value={filter.q} placeholder="Nome, partido ou UF" /></div>
      <Select label="Partido" compact bind:value={filter.party} options={[{ value: '', label: 'Todos' }, ...parties.map((p) => ({ value: p, label: p }))]} />
      <Select label="UF" compact bind:value={filter.uf} options={[{ value: '', label: 'Todas' }, ...ufs.map((u) => ({ value: u, label: u }))]} />
      <div class="f-group"><Select label="Frente ou bancada" compact bind:value={filter.group} options={groupOptions} /></div>
      <Select
        label="Agrupar"
        compact
        bind:value={groupBy}
        options={[
          { value: 'none', label: 'Sem agrupar' },
          { value: 'party', label: 'Por partido' },
          { value: 'uf', label: 'Por UF' },
        ]}
      />
      <div class="f-checks">
        <label class="art-check"><input type="checkbox" bind:checked={filter.onlyPriorities} /> Só prioritários</label>
        {#if scenario.dissidents.length}
          <label class="art-check"><input type="checkbox" bind:checked={filter.onlyDissidents} /> Só dissidentes</label>
        {/if}
      </div>
    </div>

    <Board
      columns={scenario.columns}
      positions={scenario.positions}
      columnCounts={scenario.column_counts}
      deputies={visible}
      {priorities}
      {dissidents}
      {groupBy}
      {selected}
      {canEdit}
      onselect={(dep, order, range) => {
        selected = selectIds(selected, order, dep, lastClicked, range);
        lastClicked = dep;
      }}
      onopen={(dep) => (deputyId = dep)}
      onmove={move}
      onselectall={(ids) => (selected = new Set([...selected, ...ids]))}
      oneditcolumn={(col) => ((editing = col), (modal = 'column'))}
    />

    <DonorsPanel scenarioId={scenario.id} columns={scenario.columns} />

    {#if canEdit && selected.size}
      <div class="actionbar" role="region" aria-label="Ações da seleção">
        <b>{selected.size} selecionado(s)</b>
        <Select label="Mover para" hideLabel compact bind:value={target} options={[{ value: '', label: 'Mover para…' }, ...scenario.columns.map((c) => ({ value: c.id, label: c.name }))]} />
        <Button variant="primary" size="sm" disabled={!target} onclick={() => move([...selected], target)}>Mover</Button>
        <Button variant="secondary" size="sm" onclick={() => setPriority([...selected], true)}>★ Prioritários</Button>
        <Button variant="ghost" size="sm" onclick={() => setPriority([...selected], false)}>Tirar ★</Button>
        <Button variant="ghost" size="sm" onclick={() => (selected = new Set())}>Limpar</Button>
      </div>
    {/if}
  {/if}

  {#if deputy && scenario}
    <DeputyModal
      open={true}
      {deputy}
      groups={deputy.groups.map((g) => ref?.groupById.get(g)).filter((g) => g !== undefined)}
      columns={scenario.columns}
      columnId={positionsMap(scenario.positions).get(deputy.camara_id) ?? und}
      isPriority={priorities.has(deputy.camara_id)}
      notes={project.notes.filter((n) => n.camara_id === deputy.camara_id)}
      {canEdit}
      onclose={() => (deputyId = null)}
      onmove={(col) => move([deputy.camara_id], col)}
      onpriority={(v) => setPriority([deputy.camara_id], v)}
      onaddnote={addNote}
      ondeletenote={deleteNote}
    />
  {/if}
  {#if modal === 'scenario'}
    <ScenarioModal open={true} {project} current={scenario} onclose={closeModal} oncreated={(sid) => (closeModal(), choose(sid), reload(sid, true))} />
  {:else if modal === 'settings'}
    <SettingsModal
      open={true}
      {project}
      onclose={closeModal}
      onsaved={() => (closeModal(), reload(scenario?.id ?? null, true))}
      ondeleted={() => (window.location.hash = href.room(project?.room_id ?? ''))}
    />
  {:else if modal === 'paste'}
    <PasteModal
      open={true}
      projectId={id}
      onclose={closeModal}
      onpriorities={(ids) => (closeModal(), setPriority(ids, true), toast.success('Prioritários marcados.'))}
      onselect={(ids) => (closeModal(), (selected = new Set(ids)))}
    />
  {:else if modal === 'column'}
    <ColumnModal open={true} projectId={id} column={editing} onclose={closeModal} ondone={() => (closeModal(), reload(scenario?.id ?? null, true))} />
  {/if}
{/if}

<style>
  .small {
    font-size: var(--fs-sm);
  }
  .scenarios {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
    margin-bottom: var(--sp-4);
  }
  .scn {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    font: inherit;
    font-size: var(--fs-sm);
    color: var(--text-1);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-full);
    cursor: pointer;
  }
  .scn[aria-selected='true'] {
    border-color: var(--accent);
    background: var(--accent-soft);
    font-weight: var(--fw-semibold);
  }
  .tools {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
    margin-bottom: var(--sp-3);
  }
  .spacer {
    flex: 1;
  }
  .filters {
    display: flex;
    flex-wrap: wrap;
    align-items: flex-end;
    gap: var(--sp-3);
    margin-bottom: var(--sp-4);
  }
  .filters :global(.field) {
    margin-bottom: 0;
  }
  .f-search {
    flex: 1 1 14rem;
    min-width: 0;
  }
  .f-group {
    flex: 1 1 14rem;
    min-width: 0;
    max-width: 22rem;
  }
  .f-checks {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-3);
    padding-bottom: var(--sp-2);
  }
  .actionbar {
    position: sticky;
    bottom: var(--sp-3);
    z-index: 5;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--sp-2);
    margin-top: var(--sp-4);
    padding: var(--sp-3) var(--sp-4);
    background: var(--surface-1);
    border: 1px solid var(--accent);
    border-radius: var(--r-base);
    box-shadow: var(--shadow-lg);
  }
</style>
