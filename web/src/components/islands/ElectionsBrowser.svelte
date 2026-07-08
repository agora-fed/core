<script lang="ts">
  // Elections comparator (Fase 4 do roadmap). Loads the caller-chosen election
  // then hydrates a candidate grid with Odoo-style filters: UF × cargo ×
  // partido × gênero. Filters persist in the URL query so a link is
  // shareable + a reload keeps the state.
  import { onMount } from 'svelte';
  import {
    listElections,
    listCandidacies,
    type ElectionDto,
    type CandidacyDto,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import Chip from '../ui/Chip.svelte';
  import Input from '../ui/Input.svelte';
  import Alert from '../ui/Alert.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import Icon from '../ui/Icon.svelte';

  const UFS = [
    'AC','AL','AM','AP','BA','CE','DF','ES','GO','MA','MG','MS','MT','PA','PB',
    'PE','PI','PR','RJ','RN','RO','RR','RS','SC','SE','SP','TO',
  ];
  const OFFICES = [
    { id: 'presidente', label: 'Presidente' },
    { id: 'governador', label: 'Governador' },
    { id: 'senador', label: 'Senador' },
    { id: 'deputado_federal', label: 'Deputado Federal' },
    { id: 'deputado_estadual', label: 'Deputado Estadual' },
    { id: 'prefeito', label: 'Prefeito' },
    { id: 'vereador', label: 'Vereador' },
  ];
  const GENDERS = [
    { id: 'mulher', label: 'Mulher' },
    { id: 'homem', label: 'Homem' },
    { id: 'nao-binarie', label: 'Não-binárie' },
    { id: 'prefiro-nao-dizer', label: 'Prefiro não dizer' },
  ];

  let elections = $state<ElectionDto[]>([]);
  let electionsLoading = $state(true);
  let electionsErr = $state<string | null>(null);
  let electionId = $state<string>('');

  let uf = $state<string>('');
  let office = $state<string>('');
  let party = $state<string>('');
  let gender = $state<string>('');
  let q = $state<string>('');

  let candidacies = $state<CandidacyDto[]>([]);
  let candidaciesLoading = $state(false);
  let candidaciesErr = $state<string | null>(null);

  function readQuery() {
    if (typeof window === 'undefined') return;
    const p = new URLSearchParams(window.location.search);
    electionId = p.get('election') ?? '';
    uf = p.get('uf') ?? '';
    office = p.get('office') ?? '';
    party = p.get('party') ?? '';
    gender = p.get('gender') ?? '';
    q = p.get('q') ?? '';
  }
  function writeQuery() {
    if (typeof window === 'undefined') return;
    const p = new URLSearchParams();
    if (electionId) p.set('election', electionId);
    if (uf) p.set('uf', uf);
    if (office) p.set('office', office);
    if (party) p.set('party', party);
    if (gender) p.set('gender', gender);
    if (q) p.set('q', q);
    const str = p.toString();
    history.replaceState(null, '', str ? `?${str}` : window.location.pathname);
  }

  async function loadElections() {
    electionsLoading = true;
    electionsErr = null;
    const res = await listElections();
    electionsLoading = false;
    if (res.success && res.data) {
      elections = res.data;
      if (!electionId && elections.length > 0) {
        // Default to the most recent (list is DESC by year).
        electionId = elections[0].id;
        writeQuery();
      }
      if (electionId) loadCandidacies();
    } else {
      electionsErr = res.error?.message ?? 'Falha ao carregar as eleições.';
    }
  }

  async function loadCandidacies() {
    if (!electionId) return;
    candidaciesLoading = true;
    candidaciesErr = null;
    writeQuery();
    const res = await listCandidacies(electionId, {
      uf,
      office,
      party,
      gender,
      q,
      limit: 500,
    });
    candidaciesLoading = false;
    if (res.success && res.data) {
      candidacies = res.data;
    } else {
      candidaciesErr = res.error?.message ?? 'Falha ao carregar candidatos.';
    }
  }

  onMount(() => {
    readQuery();
    loadElections();
    // Countdown tick — 60s é suficiente pra granularidade de dias/horas.
    tickTimer = setInterval(() => { now = Date.now(); }, 60_000);
    return () => {
      if (tickTimer) clearInterval(tickTimer);
    };
  });

  function pickElection(id: string) {
    electionId = id;
    writeQuery();
    loadCandidacies();
  }

  function clearFilters() {
    uf = '';
    office = '';
    party = '';
    gender = '';
    q = '';
    loadCandidacies();
  }

  let selected = $state<Set<string>>(new Set());
  function toggleSelected(id: string) {
    const next = new Set(selected);
    if (next.has(id)) next.delete(id);
    else if (next.size >= 3) return; // cap comparison at 3
    else next.add(id);
    selected = next;
  }
  let selectedList = $derived(
    candidacies.filter((c) => selected.has(c.id)),
  );

  // Party color helper — mirrors the palette used in the gastos dashboard.
  const partyColors: Record<string, string> = {
    PT: '#e84c3d',
    PL: '#1d4ed8',
    PV: '#22c55e',
    NOVO: '#f97316',
    PSDB: '#3b82f6',
    MDB: '#0ea5e9',
    PSB: '#f59e0b',
    UNIAO: '#7c3aed',
    PP: '#0891b2',
    REPUBLICANOS: '#059669',
    'PSOL': '#dc2626',
    PDT: '#eab308',
    PODE: '#8b5cf6',
    PCdoB: '#b91c1c',
    REDE: '#10b981',
    CIDADANIA: '#f472b6',
  };
  function colorFor(sigla: string): string {
    return partyColors[sigla] ?? '#6b7280';
  }

  function officeLabel(id: string): string {
    return OFFICES.find((o) => o.id === id)?.label ?? id;
  }

  // Calendário TSE 2026 — datas oficiais Resolução TSE 23.735/2024.
  // Cada entrada tem uma UTC ISO pra alimentar o countdown + comparar
  // "já passou" / "está aberto" / "próximo".
  interface CalMilestone {
    label: string;
    detail: string;
    when: string; // ISO date (00:00 BRT)
  }
  const CAL_2026: CalMilestone[] = [
    { label: 'Convenções partidárias', detail: 'Escolha dos candidatos e coligações', when: '2026-07-20T00:00:00-03:00' },
    { label: 'Registro de candidatura', detail: 'Último dia pra registrar candidatura no TSE', when: '2026-08-15T00:00:00-03:00' },
    { label: 'Início da campanha', detail: 'Propaganda eleitoral autorizada', when: '2026-08-16T00:00:00-03:00' },
    { label: 'Horário eleitoral gratuito', detail: 'Início do HEG em rádio e TV', when: '2026-09-04T00:00:00-03:00' },
    { label: '1º turno', detail: 'Presidente · Governador · Senador · Deputados Federal e Estadual', when: '2026-10-04T08:00:00-03:00' },
    { label: '2º turno', detail: 'Presidente e Governador (se necessário)', when: '2026-10-25T08:00:00-03:00' },
    { label: 'Prestação de contas final', detail: 'Prazo TSE pra prestação de contas', when: '2026-11-04T00:00:00-03:00' },
    { label: 'Posse dos eleitos', detail: 'Presidente, Governadores, Senadores, Deputados', when: '2027-01-01T00:00:00-03:00' },
  ];

  // Live-updating "agora" — tick a cada 60s. Suficiente pro countdown por
  // dias/horas (não é relógio de foguete).
  let now = $state<number>(Date.now());
  let tickTimer: ReturnType<typeof setInterval> | null = null;

  // Próximo milestone (o primeiro no futuro).
  let nextMilestone = $derived.by<CalMilestone | null>(() => {
    for (const m of CAL_2026) {
      if (new Date(m.when).getTime() > now) return m;
    }
    return null;
  });

  function fmtCountdown(target: string): string {
    const ms = new Date(target).getTime() - now;
    if (ms <= 0) return 'já ocorreu';
    const days = Math.floor(ms / 86_400_000);
    const hours = Math.floor((ms % 86_400_000) / 3_600_000);
    if (days >= 7) return `em ${days} dias`;
    if (days >= 1) return `em ${days}d ${hours}h`;
    const mins = Math.floor((ms % 3_600_000) / 60_000);
    return `em ${hours}h ${mins}m`;
  }

  function fmtCalDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString('pt-BR', {
        day: '2-digit', month: 'short', year: 'numeric',
      });
    } catch { return iso; }
  }

  function calState(iso: string): 'passed' | 'active' | 'future' {
    const t = new Date(iso).getTime();
    if (t < now - 86_400_000) return 'passed';
    if (t < now) return 'active';
    return 'future';
  }

  // Aggregate stats visible at the top of the grid.
  let stats = $derived.by(() => {
    const total = candidacies.length;
    const byGender: Record<string, number> = {};
    const byParty: Record<string, number> = {};
    for (const c of candidacies) {
      const g = c.candidate_gender ?? 'nao-informado';
      byGender[g] = (byGender[g] ?? 0) + 1;
      byParty[c.party_sigla] = (byParty[c.party_sigla] ?? 0) + 1;
    }
    const topParties = Object.entries(byParty)
      .sort((a, b) => b[1] - a[1])
      .slice(0, 6);
    return { total, byGender, topParties };
  });
</script>

<div class="wrap">
  <!-- Countdown + calendário TSE — sempre visível, dá ancoragem temporal ao usuário
       enquanto a base de candidatos não é povoada (registro só abre 15/08/2026). -->
  <section class="tse-panel">
    <div class="cd-row">
      <div class="cd-main">
        {#if nextMilestone}
          <span class="cd-eyebrow muted">Próximo passo</span>
          <strong class="cd-title">{nextMilestone.label}</strong>
          <span class="cd-when">
            {fmtCalDate(nextMilestone.when)} · <em>{fmtCountdown(nextMilestone.when)}</em>
          </span>
          <p class="cd-detail muted">{nextMilestone.detail}</p>
        {:else}
          <span class="cd-eyebrow muted">Calendário 2026</span>
          <strong class="cd-title">Ciclo eleitoral encerrado</strong>
        {/if}
      </div>
    </div>

    <ol class="cal">
      {#each CAL_2026 as m}
        {@const s = calState(m.when)}
        <li class:passed={s === 'passed'} class:active={s === 'active'}>
          <span class="dot" aria-hidden="true"></span>
          <div class="cal-body">
            <div class="cal-head">
              <strong>{m.label}</strong>
              <span class="cal-date muted">{fmtCalDate(m.when)}</span>
            </div>
            <span class="cal-detail muted">{m.detail}</span>
          </div>
        </li>
      {/each}
    </ol>
  </section>

  {#if electionsLoading}
    <div class="loading"><Spinner /></div>
  {:else if electionsErr}
    <Alert tone="danger">{electionsErr}</Alert>
  {:else if elections.length === 0}
    <Card padding="none">
      <EmptyState
        icon="ballot"
        title="Nenhuma eleição carregada ainda"
        description="O dataset do TSE ainda não foi importado. Volte assim que os candidatos oficiais forem publicados."
      />
    </Card>
  {:else}
    <section class="ele-picker">
      <span class="k muted">Eleição:</span>
      <div class="chips">
        {#each elections as e (e.id)}
          <button
            type="button"
            class="ele-chip"
            class:active={e.id === electionId}
            onclick={() => pickElection(e.id)}
          >
            {e.year} · {e.sphere}
            {#if e.round > 1}<span class="round">2º turno</span>{/if}
            <span class="count">{e.candidacy_count}</span>
          </button>
        {/each}
      </div>
    </section>

    <section class="filters">
      <div class="row-1">
        <label>
          <span>UF</span>
          <select bind:value={uf} onchange={loadCandidacies}>
            <option value="">Todas</option>
            {#each UFS as u}
              <option value={u}>{u}</option>
            {/each}
          </select>
        </label>
        <label>
          <span>Cargo</span>
          <select bind:value={office} onchange={loadCandidacies}>
            <option value="">Todos</option>
            {#each OFFICES as o}
              <option value={o.id}>{o.label}</option>
            {/each}
          </select>
        </label>
        <label>
          <span>Partido</span>
          <input
            type="text"
            placeholder="Sigla"
            bind:value={party}
            onkeydown={(e) => e.key === 'Enter' && loadCandidacies()}
          />
        </label>
        <label>
          <span>Gênero</span>
          <select bind:value={gender} onchange={loadCandidacies}>
            <option value="">Todos</option>
            {#each GENDERS as g}
              <option value={g.id}>{g.label}</option>
            {/each}
          </select>
        </label>
      </div>
      <div class="row-2">
        <div class="q-wrap">
          <Input
            id="ele-q"
            label=""
            placeholder="Buscar por nome do candidato…"
            bind:value={q}
            onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && loadCandidacies()}
          />
        </div>
        <Button variant="secondary" onclick={loadCandidacies}>
          <Icon name="search" size={16} /> Aplicar
        </Button>
        <Button variant="ghost" onclick={clearFilters}>Limpar</Button>
      </div>
    </section>

    {#if candidaciesLoading}
      <div class="loading"><Spinner /></div>
    {:else if candidaciesErr}
      <Alert tone="danger">{candidaciesErr}</Alert>
    {:else if candidacies.length === 0}
      <Card padding="none">
        <EmptyState
          icon="filter"
          title="Nenhum candidato com esses filtros"
          description="Afrouxa alguma condição ou espera o dataset ser importado."
        />
      </Card>
    {:else}
      <section class="stats">
        <Card>
          <div class="stats-row">
            <div class="stat">
              <span class="v">{new Intl.NumberFormat('pt-BR').format(stats.total)}</span>
              <span class="k">candidatos filtrados</span>
            </div>
            <div class="stat-chips">
              {#each Object.entries(stats.byGender) as [g, n]}
                <Chip>{GENDERS.find((x) => x.id === g)?.label ?? 'Não informado'}: {n}</Chip>
              {/each}
            </div>
          </div>
          <div class="top-parties">
            <span class="muted">Top partidos:</span>
            {#each stats.topParties as [sigla, n]}
              <span class="party-chip" style={`--c: ${colorFor(sigla)}`}>
                <span class="dot"></span>{sigla} <strong>{n}</strong>
              </span>
            {/each}
          </div>
        </Card>
      </section>

      {#if selectedList.length > 0}
        <section class="compare">
          <Card>
            <h3 class="sub"><Icon name="chart" size={18} /> Comparação lado-a-lado</h3>
            <div class="compare-grid">
              {#each selectedList as c (c.id)}
                <div class="compare-cell">
                  <div class="compare-head" style={`--c: ${colorFor(c.party_sigla)}`}>
                    <strong>{c.candidate_name}</strong>
                    <span>{officeLabel(c.office)} · {c.party_sigla} · nº {c.number}</span>
                  </div>
                  <dl class="compare-body">
                    <dt>UF</dt><dd>{c.sphere_uf ?? '—'}</dd>
                    <dt>Município</dt><dd>{c.sphere_municipio ?? '—'}</dd>
                    <dt>Gênero</dt><dd>{GENDERS.find((g) => g.id === c.candidate_gender)?.label ?? '—'}</dd>
                    <dt>Status</dt><dd>{c.status ?? '—'}</dd>
                  </dl>
                  <Button
                    variant="ghost"
                    size="sm"
                    onclick={() => toggleSelected(c.id)}
                  >
                    Tirar
                  </Button>
                </div>
              {/each}
            </div>
          </Card>
        </section>
      {/if}

      <section class="grid">
        {#each candidacies as c (c.id)}
          <button
            type="button"
            class="candidate"
            class:selected={selected.has(c.id)}
            onclick={() => toggleSelected(c.id)}
          >
            <span class="bar" style={`background: ${colorFor(c.party_sigla)}`}></span>
            <div class="body">
              <strong class="name">{c.candidate_name}</strong>
              <div class="meta">
                <Badge tone="neutral">{c.party_sigla}</Badge>
                <span class="muted">nº {c.number}</span>
              </div>
              <div class="office muted">
                {officeLabel(c.office)}
                {#if c.sphere_uf}· {c.sphere_uf}{/if}
                {#if c.sphere_municipio}· {c.sphere_municipio}{/if}
              </div>
              {#if c.candidate_gender}
                <span class="pill">{GENDERS.find((g) => g.id === c.candidate_gender)?.label}</span>
              {/if}
            </div>
            {#if selected.has(c.id)}
              <span class="check"><Icon name="check" size={14} /></span>
            {/if}
          </button>
        {/each}
      </section>
    {/if}
  {/if}
</div>

<style>
  .wrap {
    display: grid;
    gap: var(--sp-5);
  }

  /* --- painel TSE (countdown + calendário) --- */
  .tse-panel {
    padding: var(--sp-5);
    background: linear-gradient(
      180deg,
      var(--accent-soft) 0%,
      var(--surface-2) 100%
    );
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-lg);
    display: grid;
    gap: var(--sp-4);
  }
  .cd-row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .cd-main {
    display: flex;
    flex-direction: column;
    gap: 4px;
    flex: 1;
    min-width: 0;
  }
  .cd-eyebrow {
    font-size: var(--fs-xs);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .cd-title {
    font-size: var(--fs-2xl);
    color: var(--text-1);
    line-height: 1.15;
  }
  .cd-when {
    font-size: var(--fs-base);
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
  }
  .cd-when em {
    color: var(--accent);
    font-style: normal;
    font-weight: var(--fw-semibold);
  }
  .cd-detail {
    font-size: var(--fs-sm);
    margin: 0;
  }

  .cal {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
    counter-reset: cal;
  }
  .cal li {
    display: flex;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
  }
  .cal li.passed {
    opacity: 0.6;
  }
  .cal li.active {
    border-color: var(--accent);
    background: var(--surface-1);
    box-shadow: var(--shadow-sm);
  }
  .cal .dot {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--border-strong);
    margin-top: 6px;
    flex-shrink: 0;
  }
  .cal li.active .dot {
    background: var(--accent);
    box-shadow: 0 0 0 4px color-mix(in oklab, var(--accent) 25%, transparent);
  }
  .cal li.passed .dot {
    background: var(--positive, #22c55e);
  }
  .cal-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .cal-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .cal-head strong {
    color: var(--text-1);
    font-size: var(--fs-sm);
  }
  .cal-date {
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .cal-detail {
    font-size: var(--fs-xs);
    line-height: var(--lh-snug);
  }
  @media (max-width: 640px) {
    .cd-title {
      font-size: var(--fs-xl);
    }
  }
  .loading {
    display: flex;
    justify-content: center;
    padding: var(--sp-8);
  }
  .ele-picker {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .ele-picker .k {
    font-size: var(--fs-sm);
  }
  .chips {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .ele-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    background: var(--surface-1);
    color: var(--text-1);
    border-radius: var(--r-full);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .ele-chip:hover {
    background: var(--surface-2);
  }
  .ele-chip.active {
    background: var(--accent);
    color: var(--accent-contrast);
    border-color: var(--accent);
  }
  .ele-chip .round {
    background: rgba(0,0,0,.15);
    padding: 1px 6px;
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
  }
  .ele-chip .count {
    background: rgba(255,255,255,.35);
    padding: 1px 6px;
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .ele-chip:not(.active) .count {
    background: var(--surface-2);
  }

  .filters {
    display: flex;
    flex-direction: column;
    gap: var(--sp-3);
    padding: var(--sp-4);
    background: var(--surface-2);
    border-radius: var(--r-base);
  }
  .row-1 {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  }
  .row-1 label {
    display: flex;
    flex-direction: column;
    gap: 2px;
    font-size: var(--fs-xs);
    color: var(--text-3);
    font-weight: var(--fw-medium);
  }
  .row-1 select,
  .row-1 input {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    color: var(--text-1);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .row-2 {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-end;
    flex-wrap: wrap;
  }
  .q-wrap {
    flex: 1;
    min-width: 220px;
  }

  .stats-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: var(--sp-4);
    flex-wrap: wrap;
    margin-bottom: var(--sp-3);
  }
  .stat {
    display: flex;
    flex-direction: column;
  }
  .stat .v {
    font-size: var(--fs-3xl);
    font-weight: var(--fw-bold);
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
    line-height: 1;
  }
  .stat .k {
    font-size: var(--fs-xs);
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .stat-chips {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .top-parties {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
    padding-top: var(--sp-2);
    border-top: 1px solid var(--border-subtle);
  }
  .top-parties .muted {
    font-size: var(--fs-sm);
  }
  .party-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    padding: 2px var(--sp-2);
    background: var(--surface-2);
    border-radius: var(--r-full);
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    color: var(--text-1);
  }
  .party-chip .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--c);
  }
  .party-chip strong {
    font-variant-numeric: tabular-nums;
  }

  .compare {
    display: block;
  }
  .sub {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-lg);
    color: var(--text-1);
  }
  .compare-grid {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fit, minmax(220px, 1fr));
  }
  .compare-cell {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    background: var(--surface-1);
  }
  .compare-head {
    border-left: 4px solid var(--c);
    padding-left: var(--sp-2);
    display: flex;
    flex-direction: column;
  }
  .compare-head strong {
    font-size: var(--fs-base);
    color: var(--text-1);
  }
  .compare-head span {
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  .compare-body {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 4px var(--sp-3);
    font-size: var(--fs-sm);
    margin: 0;
  }
  .compare-body dt {
    color: var(--text-3);
    font-weight: var(--fw-medium);
  }
  .compare-body dd {
    margin: 0;
    color: var(--text-1);
  }

  .grid {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  }
  .candidate {
    position: relative;
    display: flex;
    gap: 0;
    padding: 0;
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    text-align: left;
    cursor: pointer;
    font: inherit;
    transition:
      border-color var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out);
  }
  .candidate:hover {
    border-color: var(--accent);
    box-shadow: var(--shadow-sm);
  }
  .candidate.selected {
    border-color: var(--accent);
    box-shadow: var(--shadow-focus);
  }
  .candidate .bar {
    width: 6px;
    border-radius: var(--r-base) 0 0 var(--r-base);
    flex-shrink: 0;
  }
  .candidate .body {
    flex: 1;
    padding: var(--sp-3) var(--sp-3);
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
  }
  .candidate .name {
    font-size: var(--fs-base);
    color: var(--text-1);
    line-height: 1.25;
  }
  .candidate .meta {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-xs);
  }
  .candidate .office {
    font-size: var(--fs-xs);
  }
  .candidate .pill {
    display: inline-flex;
    align-self: flex-start;
    padding: 1px 6px;
    background: var(--surface-2);
    border-radius: var(--r-sm);
    font-size: var(--fs-xs);
    color: var(--text-3);
    margin-top: 2px;
  }
  .candidate .check {
    position: absolute;
    top: var(--sp-2);
    right: var(--sp-2);
    background: var(--accent);
    color: var(--accent-contrast);
    width: 22px;
    height: 22px;
    border-radius: var(--r-full);
    display: flex;
    align-items: center;
    justify-content: center;
  }
</style>
