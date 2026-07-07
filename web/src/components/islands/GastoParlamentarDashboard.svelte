<script lang="ts">
  // Dashboard-style view of parlamentar spending. Fetches the aggregated
  // report and renders three panels: total tile, bar chart by group, table.
  // Filters (UF, casa, esfera, partido) and "Agrupar por" are chips.
  //
  // First render on a cold server cache can take ~10 s (594 concurrent
  // Câmara API calls). Skeleton stays up until the response returns.
  import { onMount } from 'svelte';
  import {
    getGastoParlamentar,
    getAllMandates,
    DEFAULT_ORG_ID,
    type ReportFilters,
    type GastoReport,
    type MandateDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Chip from '../ui/Chip.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import BarChart from '../social/BarChart.svelte';

  type GroupBy = NonNullable<ReportFilters['group_by']>;
  type House = NonNullable<ReportFilters['house']>;
  type Sphere = NonNullable<ReportFilters['sphere']>;

  let filters = $state<ReportFilters>({ group_by: 'partido' });
  let report = $state<GastoReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let mandates = $state<MandateDto[]>([]);

  const groupByOptions: { value: GroupBy; label: string }[] = [
    { value: 'partido', label: 'Partido' },
    { value: 'politico', label: 'Político' },
    { value: 'casa', label: 'Cargo (casa)' },
    { value: 'esfera', label: 'Esfera' },
    { value: 'uf', label: 'Estado (UF)' },
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
    const res = await getGastoParlamentar(filters);
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

  function fmtBrl(cents: number): string {
    return (cents / 100).toLocaleString('pt-BR', {
      style: 'currency',
      currency: 'BRL',
      maximumFractionDigits: 0,
    });
  }

  onMount(async () => {
    const mr = await getAllMandates(DEFAULT_ORG_ID);
    if (mr.ok && mr.data) mandates = mr.data;
    await load();
  });

  const groupLabel = $derived(
    groupByOptions.find((o) => o.value === filters.group_by)?.label ?? 'Partido',
  );
</script>

<header class="head">
  <div class="ic"><Icon name="chart" size={24} /></div>
  <div>
    <h1>Gasto parlamentar</h1>
    <p class="muted">
      Cota parlamentar da Câmara (última ano fechado). Fonte:
      <a href="https://dadosabertos.camara.leg.br" target="_blank" rel="noreferrer noopener">
        dadosabertos.camara.leg.br
      </a>. Senado não expõe cota comparável.
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
  <p class="hint">
    <Icon name="info" size={14} /> Primeira carga pode demorar 10-30 s enquanto
    a Câmara responde às ~500 chamadas concorrentes. Depois fica em cache.
  </p>
{:else if error}
  <ErrorState message={error} retry={load} />
{:else if report}
  <section class="totals">
    <Card>
      <div class="tile">
        <span class="tile-label">Total de gastos</span>
        <strong class="tile-value">{fmtBrl(report.total_cents)}</strong>
        <span class="tile-hint">{report.mandate_count} mandatos</span>
      </div>
    </Card>
    <Card>
      <div class="tile">
        <span class="tile-label">Média por mandato</span>
        <strong class="tile-value">
          {fmtBrl(
            report.mandate_count > 0
              ? Math.round(report.total_cents / report.mandate_count)
              : 0,
          )}
        </strong>
        <span class="tile-hint">agrupado por {groupLabel.toLowerCase()}</span>
      </div>
    </Card>
    <Card>
      <div class="tile">
        <span class="tile-label">Grupos</span>
        <strong class="tile-value">{report.groups.length}</strong>
        <span class="tile-hint">categorias distintas</span>
      </div>
    </Card>
  </section>

  <section class="chart">
    <h2>Por {groupLabel.toLowerCase()}</h2>
    <Card>
      <BarChart
        rows={report.groups.slice(0, 30).map((g) => ({
          label: g.label,
          value: g.amount_cents,
          hint: `${g.mandate_count} ${g.mandate_count === 1 ? 'mandato' : 'mandatos'}`,
        }))}
        format={fmtBrl}
        empty="Sem gastos registrados para esses filtros."
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
  .head a {
    color: var(--accent);
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
  .hint {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--text-3);
    font-size: var(--fs-sm);
    margin: var(--sp-3) 0 0;
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
