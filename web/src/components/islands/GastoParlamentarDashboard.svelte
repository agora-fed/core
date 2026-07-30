<script lang="ts">
  // Dashboard-style view of parlamentar spending, polished for the political
  // dashboards work (0.19.0-polish). Combines Câmara CEAP + Senado CEAPS
  // (backend picks the last-closed fiscal year, N-1, so numbers stay
  // stable). Three panels: tiles with pure numbers, colored bar chart with
  // %-of-total, drill-down modal on click, refresh button + cache stamp.
  // Filters + group_by persist in the URL (?group_by=&uf=…) so a link can
  // share the exact view.
  import { onMount } from 'svelte';
  import {
    getGastoParlamentar,
    getAllMandates,
    DEFAULT_ORG_ID,
    type ReportFilters,
    type GastoReport,
    type GastoDetailRow,
    type MandateDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Chip from '../ui/Chip.svelte';
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import Modal from '../ui/Modal.svelte';
  import BarChart from '../social/BarChart.svelte';

  type GroupBy = NonNullable<ReportFilters['group_by']>;
  type House = NonNullable<ReportFilters['house']>;
  type SortBy = 'value' | 'count' | 'name';

  // Sem "Esfera" — agrupar por esfera nesta base federal-only produziria
  // um único grupo (Federal). Ver `sphereOptions` acima.
  const groupByOptions: { value: GroupBy; label: string }[] = [
    { value: 'partido', label: 'Partido' },
    { value: 'politico', label: 'Político' },
    { value: 'casa', label: 'Casa (Câmara/Senado)' },
    { value: 'uf', label: 'Estado' },
    { value: 'office', label: 'Cargo' },
  ];
  const houseOptions: { value: House; label: string }[] = [
    { value: '', label: 'Todas' },
    { value: 'camara', label: 'Câmara' },
    { value: 'senado', label: 'Senado' },
  ];
  // Federal-only: CEAP (Câmara) + CEAPS (Senado) são as únicas cotas
  // parlamentares com API pública unificada. Assembleias legislativas
  // e câmaras municipais NÃO têm dado equivalente — cada uma tem regra
  // própria de disclosure. `filters.sphere` fica fixo em 'federal' e o
  // chip foi retirado da UI (não induzir a leitura errada de gráfico
  // com R$ 0).
  const sortOptions: { value: SortBy; label: string }[] = [
    { value: 'value', label: 'Valor' },
    { value: 'count', label: 'Nº de mandatos' },
    { value: 'name', label: 'Nome' },
  ];

  // Ideological color spectrum — hand-picked for the ~22 seated federal
  // parties (2026). Anything not listed falls back to var(--accent).
  const partyColors: Record<string, string> = {
    // Left
    PT: '#e84c3d',
    PSOL: '#c81f26',
    PCdoB: '#c81f26',
    PSB: '#e84c3d',
    PV: '#22c55e',
    REDE: '#22c55e',
    PDT: '#e15b3d',
    // Center-left / center
    MDB: '#f4c20d',
    SOLIDARIEDADE: '#f4c20d',
    // Center-right / right
    PSDB: '#3c76c6',
    UNIÃO: '#1d4ed8',
    PP: '#1d4ed8',
    PL: '#1d4ed8',
    REPUBLICANOS: '#1d4ed8',
    PODEMOS: '#3c76c6',
    CIDADANIA: '#3c76c6',
    NOVO: '#f97316',
    AVANTE: '#1d4ed8',
    'PSD': '#1d4ed8',
    PRD: '#3c76c6',
  };
  function colorFor(label: string, key: string): string {
    const p = partyColors[label] || partyColors[key];
    return p ?? 'var(--accent)';
  }

  // Sempre federal — ver comentário em `sphereOptions` acima.
  let filters = $state<ReportFilters>({ group_by: 'partido', sphere: 'federal' });
  let sortBy = $state<SortBy>('value');
  let report = $state<GastoReport | null>(null);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let mandates = $state<MandateDto[]>([]);
  let refreshing = $state(false);
  let ready = $state(false);

  // Drill-down modal state.
  let drillOpen = $state(false);
  let drillLabel = $state('');
  let drillRows = $state<GastoDetailRow[]>([]);

  function loadFromUrl() {
    const p = new URLSearchParams(window.location.search);
    filters = {
      group_by: (p.get('group_by') as GroupBy) || 'partido',
      uf: p.get('uf') || undefined,
      house: (p.get('house') as House) || undefined,
      party: p.get('party') || undefined,
      // Sempre federal — sphere estadual/municipal não tem fonte de dado.
      // Ignora explicitamente parâmetro na URL vindo de link antigo.
      sphere: 'federal',
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

  async function load(opts: { refresh?: boolean } = {}) {
    loading = true;
    error = null;
    persistUrl();
    const res = await getGastoParlamentar(filters, opts);
    loading = false;
    if (res.ok && res.data) {
      report = res.data;
    } else {
      error = res.error ?? 'Falha ao carregar o painel.';
    }
  }

  async function refresh() {
    if (refreshing) return;
    refreshing = true;
    await load({ refresh: true });
    refreshing = false;
    if (!error) toast.success('Dados atualizados da Câmara + Senado.');
  }

  function setFilter<K extends keyof ReportFilters>(k: K, v: ReportFilters[K]) {
    filters = { ...filters, [k]: v };
    void load();
  }
  function setSort(v: SortBy) {
    sortBy = v;
    persistUrl();
  }

  function fmtBrl(cents: number): string {
    return (cents / 100).toLocaleString('pt-BR', {
      style: 'currency',
      currency: 'BRL',
      maximumFractionDigits: 0,
    });
  }
  function fmtRelative(iso: string): string {
    const then = new Date(iso).getTime();
    const now = Date.now();
    const secs = Math.floor((now - then) / 1000);
    if (secs < 60) return 'agora mesmo';
    if (secs < 3600) return `há ${Math.floor(secs / 60)} min`;
    if (secs < 86400) return `há ${Math.floor(secs / 3600)} h`;
    return `há ${Math.floor(secs / 86400)} d`;
  }

  const groupLabel = $derived(
    groupByOptions.find((o) => o.value === filters.group_by)?.label ?? 'Partido',
  );

  const sortedGroups = $derived.by(() => {
    if (!report) return [];
    const arr = [...report.groups];
    if (sortBy === 'value') arr.sort((a, b) => b.amount_cents - a.amount_cents);
    else if (sortBy === 'count')
      arr.sort((a, b) => b.mandate_count - a.mandate_count);
    else arr.sort((a, b) => a.label.localeCompare(b.label, 'pt-BR'));
    return arr;
  });

  function openDrill(label: string, key: string) {
    if (!report?.detail || filters.group_by !== 'partido' && filters.group_by !== 'uf' && filters.group_by !== 'casa' && filters.group_by !== 'esfera' && filters.group_by !== 'office') {
      // For politico group_by each bar is already one mandate — clicking is
      // less useful. Skip modal.
      window.location.href = `/politicos/?id=${key}`;
      return;
    }
    if (!report.detail) return;
    drillLabel = label;
    // Filter detail rows by group. Which field to compare depends on group_by.
    const gb = filters.group_by ?? 'partido';
    drillRows = report.detail.filter((d) => {
      if (gb === 'partido') return (d.party ?? 'SEM PARTIDO') === label;
      if (gb === 'casa') {
        if (label === 'Câmara') return d.house === 'camara';
        if (label === 'Senado') return d.house === 'senado';
        return !d.house;
      }
      if (gb === 'uf' || gb === 'esfera' || gb === 'office') {
        return d.uf === label;
      }
      return true;
    });
    drillRows.sort((a, b) => b.amount_cents - a.amount_cents);
    drillOpen = true;
  }

  onMount(async () => {
    loadFromUrl();
    ready = true;
    // Restrict to federal — CEAP (Câmara) + CEAPS (Senado) só existem para
    // o Congresso Federal.
    const mr = await getAllMandates(DEFAULT_ORG_ID, 5000, 'federal');
    if (mr.ok && mr.data) mandates = mr.data;
    await load();
  });
</script>

<header class="head">
  <div class="ic"><Icon name="chart" size={24} /></div>
  <div class="head-body">
    <h1>Gasto parlamentar</h1>
    <p class="muted">
      <strong>Federal apenas.</strong> Câmara (CEAP) + Senado (CEAPS), ano
      fiscal fechado. Assembleias Legislativas e Câmaras Municipais não
      possuem cota parlamentar com API pública unificada — cada casa
      publica em formato próprio, quando publica. Fontes:
      <a href="https://dadosabertos.camara.leg.br" target="_blank" rel="noreferrer noopener">
        dadosabertos.camara.leg.br</a> ·
      <a
        href="https://www12.senado.leg.br/transparencia/dados-abertos-transparencia/dados-abertos-ceaps"
        target="_blank"
        rel="noreferrer noopener"
      >transparência do Senado</a>.
    </p>
  </div>
</header>

{#if ready && report}
  <div class="meta-bar">
    <div class="meta-info">
      <Icon name="calendar" size={14} />
      <span>Ano de referência: <strong>{report.year}</strong></span>
      <span class="dot">·</span>
      <Icon name="info" size={14} />
      <span>Atualizado {fmtRelative(report.cached_at)}</span>
    </div>
    <Button
      variant="ghost"
      size="sm"
      onclick={refresh}
      loading={refreshing}
      disabled={refreshing || loading}
    >
      <Icon name="arrow-right" size={14} />
      Atualizar dados
    </Button>
  </div>
{/if}

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
  <!-- Chip de esfera removido — este painel é federal-only (CEAP+CEAPS). -->
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
  <p class="hint">
    <Icon name="info" size={14} /> Primeira carga pode demorar 15-45 s
    (~594 chamadas concorrentes + CSV do Senado). Depois fica em cache 6 h.
  </p>
{:else if error}
  <ErrorState message={error} retry={() => load()} />
{:else if report}
  <section class="totals">
    <Card>
      <div class="tile">
        <span class="tile-label">Total gasto</span>
        <strong class="tile-value">{fmtBrl(report.total_cents)}</strong>
        <span class="tile-hint">
          {report.mandate_count} mandatos · ano {report.year}
        </span>
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
        <span class="tile-hint">
          {sortBy === 'value' ? 'ordenados por valor' : sortBy === 'count' ? 'ordenados por nº' : 'ordenados por nome'}
        </span>
      </div>
    </Card>
  </section>

  <section class="chart">
    <h2>Por {groupLabel.toLowerCase()}</h2>
    {#if report.total_cents === 0}
      <Card padding="none">
        <EmptyState
          icon="chart"
          title="Sem gastos registrados para esses filtros"
          description={filters.house === 'senado'
            ? 'Confira se o ano de referência já teve CEAPS publicada.'
            : 'Ajuste os filtros ou clique em Atualizar dados.'}
        />
      </Card>
    {:else}
      <Card>
        <ol class="rows">
          {#each sortedGroups.slice(0, 30) as g (g.key)}
            {@const pct = report.total_cents > 0
              ? Math.round((g.amount_cents / report.total_cents) * 1000) / 10
              : 0}
            {@const barPct = Math.max(2, Math.round((g.amount_cents / (sortedGroups[0]?.amount_cents || 1)) * 100))}
            <li>
              <button
                type="button"
                class="row"
                onclick={() => openDrill(g.label, g.key)}
                title={`Ver detalhes de ${g.label}`}
              >
                <span class="row-label" title={g.label}>{g.label}</span>
                <span
                  class="row-bar"
                  role="img"
                  aria-label={`${g.label}: ${fmtBrl(g.amount_cents)}`}
                >
                  <span
                    class="row-fill"
                    style={`--pct:${barPct}%; --bar-color:${colorFor(g.label, g.key)}`}
                  ></span>
                </span>
                <span class="row-value">
                  <strong>{fmtBrl(g.amount_cents)}</strong>
                  <span class="row-hint">
                    {pct}% · {g.mandate_count} {g.mandate_count === 1 ? 'mandato' : 'mand.'}
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

  {#if filters.house === 'senado'}
    <p class="disclaimer">
      <Icon name="info" size={14} />
      Senado publica só a CEAPS (equivalente à CEAP da Câmara). Auxílio-moradia (R$ 5.500/mês)
      e imóveis funcionais são divulgados em outra página e ainda não estão somados aqui.
    </p>
  {/if}
{/if}

<Modal bind:open={drillOpen} title={`Detalhamento: ${drillLabel}`} size="lg">
  {#if drillRows.length > 0}
    <table class="detail-table">
      <thead>
        <tr>
          <th>Mandato</th>
          <th>Partido / UF</th>
          <th>Casa</th>
          <th class="right">Gasto</th>
        </tr>
      </thead>
      <tbody>
        {#each drillRows.slice(0, 100) as r (r.mandate_id)}
          <tr>
            <td>
              <a href={`/politicos/?id=${r.mandate_id}`}>{r.display_name}</a>
            </td>
            <td>
              <span class="party-tag">{r.party ?? 'SEM'}</span>
              <span class="muted">/ {r.uf ?? '—'}</span>
            </td>
            <td>
              {r.house === 'camara' ? 'Câmara' : r.house === 'senado' ? 'Senado' : '—'}
            </td>
            <td class="right"><strong>{fmtBrl(r.amount_cents)}</strong></td>
          </tr>
        {/each}
      </tbody>
    </table>
    {#if drillRows.length > 100}
      <p class="muted">Mostrando 100 de {drillRows.length}.</p>
    {/if}
  {:else}
    <p class="muted">Sem detalhamento disponível.</p>
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
  .meta-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    margin-bottom: var(--sp-5);
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .meta-info {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .meta-info .dot {
    color: var(--text-3);
    margin: 0 4px;
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
  .hint {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--text-3);
    font-size: var(--fs-sm);
    margin: var(--sp-3) 0 0;
  }
  .disclaimer {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    background: var(--warning-soft);
    color: var(--warning);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    font-size: var(--sp-sm);
    margin-top: var(--sp-4);
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
      grid-template-columns: 90px 1fr 110px;
    }
  }
</style>
