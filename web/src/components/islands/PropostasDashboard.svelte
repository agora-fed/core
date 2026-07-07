<script lang="ts">
  // Propostas dashboard — DB-only aggregation, same polish surface as
  // GastoParlamentarDashboard: URL-persistent filters, chip sort, colored
  // bars, %-do-total, drill-down modal por click.
  import { onMount } from 'svelte';
  import {
    getPropostasSummary,
    getAllMandates,
    DEFAULT_ORG_ID,
    type ReportFilters,
    type PropostasReport,
    type PropostasDetailRow,
    type MandateDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Chip from '../ui/Chip.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import Modal from '../ui/Modal.svelte';

  type GroupBy = NonNullable<ReportFilters['group_by']>;
  type House = NonNullable<ReportFilters['house']>;
  type Sphere = NonNullable<ReportFilters['sphere']>;
  type Status = NonNullable<ReportFilters['status']>;
  type SortBy = 'value' | 'count' | 'name';

  const groupByOptions: { value: GroupBy; label: string }[] = [
    { value: 'partido', label: 'Partido' },
    { value: 'politico', label: 'Político' },
    { value: 'casa', label: 'Casa' },
    { value: 'esfera', label: 'Esfera' },
    { value: 'uf', label: 'Estado' },
    { value: 'office', label: 'Cargo' },
  ];
  const houseOptions: { value: House; label: string }[] = [
    { value: '', label: 'Todas' },
    { value: 'camara', label: 'Câmara' },
    { value: 'senado', label: 'Senado' },
  ];
  const sphereOptions: { value: Sphere; label: string }[] = [
    { value: '', label: 'Todas' },
    { value: 'federal', label: 'Federal' },
    { value: 'estadual', label: 'Estadual' },
    { value: 'municipal', label: 'Municipal' },
  ];
  const statusOptions: { value: Status; label: string }[] = [
    { value: '', label: 'Todos' },
    { value: 'draft', label: 'Rascunho' },
    { value: 'published', label: 'Publicada' },
    { value: 'clustered', label: 'Consenso' },
  ];
  const sortOptions: { value: SortBy; label: string }[] = [
    { value: 'value', label: 'Nº de propostas' },
    { value: 'count', label: 'Publicadas' },
    { value: 'name', label: 'Nome' },
  ];

  // Same ideological palette as the gastos dashboard so bars stay
  // consistent between screens.
  const partyColors: Record<string, string> = {
    PT: '#e84c3d',
    PSOL: '#c81f26',
    PCdoB: '#c81f26',
    PSB: '#e84c3d',
    PV: '#22c55e',
    REDE: '#22c55e',
    PDT: '#e15b3d',
    MDB: '#f4c20d',
    SOLIDARIEDADE: '#f4c20d',
    PSDB: '#3c76c6',
    UNIÃO: '#1d4ed8',
    PP: '#1d4ed8',
    PL: '#1d4ed8',
    REPUBLICANOS: '#1d4ed8',
    PODEMOS: '#3c76c6',
    CIDADANIA: '#3c76c6',
    NOVO: '#f97316',
    AVANTE: '#1d4ed8',
    PSD: '#1d4ed8',
    PRD: '#3c76c6',
  };
  function colorFor(label: string, key: string): string {
    return partyColors[label] || partyColors[key] || 'var(--accent)';
  }

  let filters = $state<ReportFilters>({ group_by: 'partido' });
  let sortBy = $state<SortBy>('value');
  let report = $state<PropostasReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let mandates = $state<MandateDto[]>([]);
  let ready = $state(false);

  // Drill-down state.
  let drillOpen = $state(false);
  let drillLabel = $state('');
  let drillRows = $state<PropostasDetailRow[]>([]);

  function loadFromUrl() {
    const p = new URLSearchParams(window.location.search);
    filters = {
      group_by: (p.get('group_by') as GroupBy) || 'partido',
      status: (p.get('status') as Status) || undefined,
      uf: p.get('uf') || undefined,
      house: (p.get('house') as House) || undefined,
      party: p.get('party') || undefined,
      sphere: (p.get('sphere') as Sphere) || undefined,
    };
    const sortParam = p.get('sort') as SortBy | null;
    if (sortParam && ['value', 'count', 'name'].includes(sortParam)) {
      sortBy = sortParam;
    }
  }
  function persistUrl() {
    const url = new URL(window.location.href);
    const set = (k: string, v: string | undefined) => {
      if (v) url.searchParams.set(k, v);
      else url.searchParams.delete(k);
    };
    set('group_by', filters.group_by);
    set('status', filters.status);
    set('uf', filters.uf);
    set('house', filters.house);
    set('party', filters.party);
    set('sphere', filters.sphere);
    set('sort', sortBy);
    history.replaceState({}, '', url);
  }

  const ufList = $derived.by(() => {
    const set = new Set<string>();
    for (const m of mandates) if (m.uf) set.add(m.uf);
    return Array.from(set).sort();
  });
  const partyList = $derived.by(() => {
    const set = new Set<string>();
    for (const m of mandates) if (m.party) set.add(m.party);
    return Array.from(set).sort();
  });

  async function load() {
    loading = true;
    error = null;
    persistUrl();
    const res = await getPropostasSummary(filters);
    loading = false;
    if (res.ok && res.data) report = res.data;
    else error = res.error ?? 'Falha ao carregar o painel.';
  }

  function setFilter<K extends keyof ReportFilters>(k: K, v: ReportFilters[K]) {
    filters = { ...filters, [k]: v };
    void load();
  }
  function setSort(v: SortBy) {
    sortBy = v;
    persistUrl();
  }

  const groupLabel = $derived(
    groupByOptions.find((o) => o.value === filters.group_by)?.label ?? 'Partido',
  );

  const sortedGroups = $derived.by(() => {
    if (!report) return [];
    const arr = [...report.groups];
    if (sortBy === 'value') arr.sort((a, b) => b.count - a.count);
    else if (sortBy === 'count') arr.sort((a, b) => b.published - a.published);
    else arr.sort((a, b) => a.label.localeCompare(b.label, 'pt-BR'));
    return arr;
  });

  function openDrill(label: string, key: string) {
    if (filters.group_by === 'politico') {
      window.location.href = `/politicos/${key}`;
      return;
    }
    if (!report?.detail) return;
    drillLabel = label;
    const gb = filters.group_by ?? 'partido';
    drillRows = report.detail.filter((d) => {
      if (gb === 'partido') return (d.party ?? 'SEM PARTIDO') === label;
      if (gb === 'casa') {
        if (label === 'Câmara') return d.house === 'camara';
        if (label === 'Senado') return d.house === 'senado';
        return !d.house;
      }
      if (gb === 'uf') return d.uf === label;
      return true;
    });
    drillRows.sort((a, b) => b.count - a.count);
    drillOpen = true;
  }

  onMount(async () => {
    loadFromUrl();
    ready = true;
    const mr = await getAllMandates(DEFAULT_ORG_ID);
    if (mr.ok && mr.data) mandates = mr.data;
    await load();
  });
</script>

<header class="head">
  <div class="ic"><Icon name="ballot" size={24} /></div>
  <div class="head-body">
    <h1>Propostas por mandato</h1>
    <p class="muted">
      Propostas dirigidas aos mandatos, agregadas por partido, político,
      casa, esfera, estado ou cargo — com filtro por status.
    </p>
  </div>
</header>

<section class="controls">
  <div class="ctrl">
    <span class="ctrl-label">Agrupar por</span>
    <div class="chips">
      {#each groupByOptions as o (o.value)}
        <Chip
          selected={filters.group_by === o.value}
          onclick={() => setFilter('group_by', o.value)}
        >{o.label}</Chip>
      {/each}
    </div>
  </div>
  <div class="ctrl">
    <span class="ctrl-label">Ordenar por</span>
    <div class="chips">
      {#each sortOptions as o (o.value)}
        <Chip selected={sortBy === o.value} onclick={() => setSort(o.value)}>
          {o.label}
        </Chip>
      {/each}
    </div>
  </div>
  <div class="ctrl">
    <span class="ctrl-label">Status</span>
    <div class="chips">
      {#each statusOptions as o (o.value + '')}
        <Chip
          selected={(filters.status ?? '') === o.value}
          onclick={() => setFilter('status', o.value)}
        >{o.label}</Chip>
      {/each}
    </div>
  </div>
  <div class="ctrl">
    <span class="ctrl-label">Casa</span>
    <div class="chips">
      {#each houseOptions as o (o.value + '')}
        <Chip
          selected={(filters.house ?? '') === o.value}
          onclick={() => setFilter('house', o.value)}
        >{o.label}</Chip>
      {/each}
    </div>
  </div>
  <div class="ctrl">
    <span class="ctrl-label">Esfera</span>
    <div class="chips">
      {#each sphereOptions as o (o.value + '')}
        <Chip
          selected={(filters.sphere ?? '') === o.value}
          onclick={() => setFilter('sphere', o.value)}
        >{o.label}</Chip>
      {/each}
    </div>
  </div>
  <div class="ctrl">
    <span class="ctrl-label">Estado</span>
    <div class="chips scroll">
      <Chip selected={!filters.uf} onclick={() => setFilter('uf', '')}>Todos</Chip>
      {#each ufList as u (u)}
        <Chip
          selected={filters.uf === u}
          onclick={() => setFilter('uf', u)}
        >{u}</Chip>
      {/each}
    </div>
  </div>
  {#if partyList.length > 0}
    <div class="ctrl">
      <span class="ctrl-label">Partido</span>
      <div class="chips scroll">
        <Chip selected={!filters.party} onclick={() => setFilter('party', '')}>Todos</Chip>
        {#each partyList as p (p)}
          <Chip
            selected={filters.party === p}
            onclick={() => setFilter('party', p)}
          >{p}</Chip>
        {/each}
      </div>
    </div>
  {/if}
</section>

{#if loading && !report}
  <div class="skeletons">
    <Card><Skeleton lines={2} /></Card>
    <Card><Skeleton lines={6} /></Card>
  </div>
{:else if error}
  <ErrorState message={error} retry={load} />
{:else if report}
  {@const published = report.groups.reduce((a, g) => a + g.published, 0)}
  {@const clustered = report.groups.reduce((a, g) => a + g.clustered, 0)}
  {@const maxCount = sortedGroups[0]?.count || 1}
  <section class="totals">
    <Card>
      <div class="tile">
        <span class="tile-label">Total de propostas</span>
        <strong class="tile-value">{report.total.toLocaleString('pt-BR')}</strong>
        <span class="tile-hint">
          {report.groups.length} {report.groups.length === 1 ? 'categoria' : 'categorias'}
        </span>
      </div>
    </Card>
    <Card>
      <div class="tile">
        <span class="tile-label">Publicadas</span>
        <strong class="tile-value">{published.toLocaleString('pt-BR')}</strong>
        <span class="tile-hint">
          {report.total > 0 ? Math.round((published / report.total) * 100) : 0}% do total
        </span>
      </div>
    </Card>
    <Card>
      <div class="tile">
        <span class="tile-label">Em consenso</span>
        <strong class="tile-value">{clustered.toLocaleString('pt-BR')}</strong>
        <span class="tile-hint">agrupadas em cluster semântico</span>
      </div>
    </Card>
  </section>

  <section class="chart">
    <h2>Por {groupLabel.toLowerCase()}</h2>
    {#if report.total === 0}
      <Card padding="none">
        <EmptyState
          icon="ballot"
          title="Sem propostas para esses filtros"
          description="Solte os filtros para ver a distribuição."
        />
      </Card>
    {:else}
      <Card>
        <ol class="rows">
          {#each sortedGroups.slice(0, 30) as g (g.key)}
            {@const pct = report.total > 0
              ? Math.round((g.count / report.total) * 1000) / 10
              : 0}
            {@const barPct = Math.max(2, Math.round((g.count / maxCount) * 100))}
            <li>
              <button
                type="button"
                class="row"
                onclick={() => openDrill(g.label, g.key)}
                title={`Ver mandatos de ${g.label}`}
              >
                <span class="row-label" title={g.label}>{g.label}</span>
                <span class="row-bar">
                  <span
                    class="row-fill"
                    style={`--pct:${barPct}%; --bar-color:${colorFor(g.label, g.key)}`}
                  ></span>
                </span>
                <span class="row-value">
                  <strong>
                    {g.count.toLocaleString('pt-BR')} {g.count === 1 ? 'proposta' : 'propostas'}
                  </strong>
                  <span class="row-hint">
                    {pct}% · {g.published} publicada{g.published === 1 ? '' : 's'}
                  </span>
                </span>
              </button>
            </li>
          {/each}
        </ol>
      </Card>
    {/if}
  </section>

  {#if sortedGroups.length > 30}
    <p class="muted center">
      Mostrando 30 de {sortedGroups.length}. Refine os filtros para ver o resto.
    </p>
  {/if}
{/if}

<Modal bind:open={drillOpen} title={`Mandatos: ${drillLabel}`} size="lg">
  {#if drillRows.length > 0}
    <table class="detail-table">
      <thead>
        <tr>
          <th>Mandato</th>
          <th>Partido / UF</th>
          <th>Casa</th>
          <th class="right">Nº</th>
          <th class="right">Publicadas</th>
        </tr>
      </thead>
      <tbody>
        {#each drillRows.slice(0, 100) as r (r.mandate_id)}
          <tr>
            <td>
              <a href={`/politicos/${r.mandate_id}`}>{r.display_name}</a>
            </td>
            <td>
              <span class="party-tag">{r.party ?? 'SEM'}</span>
              <span class="muted">/ {r.uf ?? '—'}</span>
            </td>
            <td>
              {r.house === 'camara' ? 'Câmara' : r.house === 'senado' ? 'Senado' : '—'}
            </td>
            <td class="right"><strong>{r.count}</strong></td>
            <td class="right muted">{r.published}</td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if drillRows.length > 100}
      <p class="muted">Mostrando 100 de {drillRows.length}.</p>
    {/if}
  {:else}
    <p class="muted">Sem detalhamento para esse recorte.</p>
  {/if}
</Modal>

<style>
  .head {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    margin-bottom: var(--sp-5);
  }
  .head-body {
    min-width: 0;
    flex: 1;
  }
  .head h1 {
    margin: 0;
    font-size: var(--fs-3xl);
    color: var(--text-1);
  }
  .head p {
    margin: 2px 0 0;
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .ic {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    background: var(--accent-soft);
    color: var(--accent);
    border-radius: var(--r-base);
    flex-shrink: 0;
  }
  .controls {
    display: grid;
    gap: var(--sp-3);
    margin-bottom: var(--sp-5);
  }
  .ctrl {
    display: grid;
    grid-template-columns: 130px 1fr;
    gap: var(--sp-3);
    align-items: center;
  }
  .ctrl-label {
    font-size: var(--fs-sm);
    color: var(--text-3);
    font-weight: var(--fw-semibold);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-1);
  }
  .chips.scroll {
    max-height: 5.6em;
    overflow-y: auto;
    padding-right: var(--sp-1);
  }
  .totals {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: 1fr;
    margin-bottom: var(--sp-5);
  }
  @media (min-width: 720px) {
    .totals {
      grid-template-columns: repeat(3, 1fr);
    }
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
  }
  .tile-label {
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--text-3);
    font-weight: var(--fw-semibold);
  }
  .tile-value {
    font-size: var(--fs-3xl);
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
    line-height: 1.1;
  }
  .tile-hint {
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .chart h2 {
    font-size: var(--fs-xl);
    margin: 0 0 var(--sp-3);
    color: var(--text-1);
  }
  .rows {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .row {
    display: grid;
    grid-template-columns: minmax(120px, 22%) 1fr minmax(180px, auto);
    align-items: center;
    gap: var(--sp-3);
    background: transparent;
    border: 0;
    padding: 4px 0;
    cursor: pointer;
    font: inherit;
    text-align: left;
    color: inherit;
    width: 100%;
    border-radius: var(--r-sm);
    transition: background var(--dur-fast) var(--ease-out);
  }
  .row:hover {
    background: var(--surface-2);
  }
  .row-label {
    font-size: var(--fs-sm);
    color: var(--text-1);
    font-weight: var(--fw-medium);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding-left: var(--sp-2);
  }
  .row-bar {
    height: 24px;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    overflow: hidden;
    position: relative;
  }
  .row-fill {
    height: 100%;
    width: var(--pct, 0%);
    background: var(--bar-color, var(--accent));
    border-radius: var(--r-sm);
    display: block;
    transition: width var(--dur-base) var(--ease-out);
  }
  .row-value {
    font-variant-numeric: tabular-nums;
    text-align: right;
    color: var(--text-1);
    font-size: var(--fs-sm);
    padding-right: var(--sp-2);
  }
  .row-value strong {
    font-weight: var(--fw-semibold);
    display: block;
  }
  .row-value .row-hint {
    display: block;
    color: var(--text-3);
    font-size: var(--fs-xs);
    font-weight: var(--fw-medium);
  }
  .skeletons {
    display: grid;
    gap: var(--sp-3);
  }
  .center {
    text-align: center;
  }
  .muted {
    color: var(--text-3);
  }
  .detail-table {
    width: 100%;
    border-collapse: collapse;
    font-size: var(--fs-sm);
  }
  .detail-table th,
  .detail-table td {
    padding: var(--sp-2) var(--sp-3);
    border-bottom: 1px solid var(--border-subtle);
    text-align: left;
  }
  .detail-table th {
    color: var(--text-3);
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .detail-table .right {
    text-align: right;
  }
  .detail-table a {
    color: var(--accent);
    text-decoration: none;
  }
  .detail-table a:hover {
    text-decoration: underline;
  }
  .party-tag {
    background: var(--surface-2);
    padding: 1px 6px;
    border-radius: var(--r-full);
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
  }
  @media (max-width: 640px) {
    .ctrl {
      grid-template-columns: 1fr;
      gap: var(--sp-1);
    }
    .row {
      grid-template-columns: 90px 1fr 130px;
    }
  }
</style>
