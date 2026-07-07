<script lang="ts">
  // Dashboard for proposals — DB-only aggregation. Structure mirrors the
  // gastos dashboard for consistency: total tiles + bar chart + filters.
  import { onMount } from 'svelte';
  import {
    getPropostasSummary,
    getAllMandates,
    DEFAULT_ORG_ID,
    type ReportFilters,
    type PropostasReport,
    type MandateDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Chip from '../ui/Chip.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import BarChart from '../social/BarChart.svelte';

  type GroupBy = NonNullable<ReportFilters['group_by']>;
  type House = NonNullable<ReportFilters['house']>;
  type Sphere = NonNullable<ReportFilters['sphere']>;
  type Status = NonNullable<ReportFilters['status']>;

  let filters = $state<ReportFilters>({ group_by: 'partido' });
  let report = $state<PropostasReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let mandates = $state<MandateDto[]>([]);

  const groupByOptions: { value: GroupBy; label: string }[] = [
    { value: 'partido', label: 'Partido' },
    { value: 'politico', label: 'Político' },
    { value: 'casa', label: 'Cargo (casa)' },
    { value: 'esfera', label: 'Esfera' },
    { value: 'uf', label: 'Estado' },
    { value: 'office', label: 'Cargo completo' },
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
    { value: '', label: 'Todas' },
    { value: 'draft', label: 'Rascunho' },
    { value: 'published', label: 'Publicada' },
    { value: 'clustered', label: 'Consenso' },
  ];

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
    const res = await getPropostasSummary(filters);
    loading = false;
    if (res.ok && res.data) {
      report = res.data;
    } else {
      error = res.error ?? 'Falha ao carregar o painel.';
    }
  }

  function setFilter<K extends keyof ReportFilters>(k: K, v: ReportFilters[K]) {
    filters = { ...filters, [k]: v };
    void load();
  }

  const groupLabel = $derived(
    groupByOptions.find((o) => o.value === filters.group_by)?.label ?? 'Partido',
  );

  onMount(async () => {
    const mr = await getAllMandates(DEFAULT_ORG_ID);
    if (mr.ok && mr.data) mandates = mr.data;
    await load();
  });
</script>

<header class="head">
  <div class="ic"><Icon name="ballot" size={24} /></div>
  <div>
    <h1>Propostas por mandato</h1>
    <p class="muted">
      Todas as propostas dirigidas a cada mandato, agrupadas conforme filtros
      de partido, casa, esfera e status.
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
        >
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
        >
          {o.label}
        </Chip>
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
        >
          {o.label}
        </Chip>
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
        >
          {o.label}
        </Chip>
      {/each}
    </div>
  </div>
  <div class="ctrl">
    <span class="ctrl-label">Estado</span>
    <div class="chips scroll">
      <Chip
        selected={!filters.uf}
        onclick={() => setFilter('uf', '')}
      >Todos</Chip>
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
  <section class="totals">
    <Card>
      <div class="tile">
        <span class="tile-label">Total de propostas</span>
        <strong class="tile-value">{report.total.toLocaleString('pt-BR')}</strong>
        <span class="tile-hint">{report.groups.length} categorias</span>
      </div>
    </Card>
    <Card>
      <div class="tile">
        <span class="tile-label">Publicadas</span>
        <strong class="tile-value">{published.toLocaleString('pt-BR')}</strong>
        <span class="tile-hint">passaram pela moderação</span>
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
    <Card>
      <BarChart
        rows={report.groups.slice(0, 30).map((g) => ({
          label: g.label,
          value: g.count,
          hint: `${g.published} publicada${g.published === 1 ? '' : 's'}`,
        }))}
        format={(v) => `${v.toLocaleString('pt-BR')} propostas`}
        empty="Sem propostas para esses filtros."
      />
    </Card>
  </section>

  {#if report.groups.length > 30}
    <p class="muted center">
      Mostrando 30 de {report.groups.length}. Refine os filtros para ver o resto.
    </p>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-6);
  }
  .head h1 {
    margin: 0;
    font-size: var(--fs-3xl);
    color: var(--text-1);
  }
  .head p {
    margin: 2px 0 0;
    font-size: var(--fs-sm);
    max-width: 46rem;
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
  .chart {
    margin-top: var(--sp-4);
  }
  .chart h2 {
    font-size: var(--fs-xl);
    margin: 0 0 var(--sp-3);
    color: var(--text-1);
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

  @media (max-width: 640px) {
    .ctrl {
      grid-template-columns: 1fr;
      gap: var(--sp-1);
    }
  }
</style>
