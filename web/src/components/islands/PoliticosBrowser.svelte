<script lang="ts">
  // Politicos browser — 0.23.0-municipais rewrite.
  //
  // The previous version pulled every mandate (~1653 rows / 17 requests). With
  // 68k+ municipals coming in the seed, that path is dead. Now the browser:
  //   1. Refuses to query until the user picks a sphere.
  //   2. Requires UF when sphere ≠ federal.
  //   3. Requires municipio (from a dropdown of ~5.5k options) when sphere =
  //      municipal.
  //   4. Sends one filtered request to `/api/v1/politicos/browse` and paginates
  //      server-side.
  //
  // Filters mirror to the URL query so a page is shareable and a reload
  // restores state.
  import { onMount } from 'svelte';
  import {
    browsePoliticos,
    listMunicipios,
    type PoliticoRow,
    type MunicipioRow,
  } from '../../lib/api';
  import Card from '../ui/Card.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Badge from '../ui/Badge.svelte';
  import Icon from '../ui/Icon.svelte';
  import Input from '../ui/Input.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Alert from '../ui/Alert.svelte';
  import Spinner from '../ui/Spinner.svelte';
  import Button from '../ui/Button.svelte';

  const UFS = [
    'AC','AL','AM','AP','BA','CE','DF','ES','GO','MA','MG','MS','MT','PA','PB',
    'PE','PI','PR','RJ','RN','RO','RR','RS','SC','SE','SP','TO',
  ];

  type Sphere = 'federal' | 'estadual' | 'municipal';

  // Default sphere = 'federal' — the smallest bucket (594 rows) so the page
  // always shows *something* on first paint. The user can switch to estadual /
  // municipal from the dropdown.
  let sphere = $state<Sphere | ''>('federal');
  let uf = $state('');
  let municipio = $state('');
  let q = $state('');

  let municipios = $state<MunicipioRow[]>([]);
  let munLoading = $state(false);
  let munErr = $state<string | null>(null);

  let items = $state<PoliticoRow[]>([]);
  let total = $state(0);
  let offset = $state(0);
  const PAGE = 100;
  let loading = $state(false);
  let listErr = $state<string | null>(null);

  // Restore filters from URL — running before the first mount call so an
  // authored deep link (e.g. shared bookmark) executes the query immediately.
  function readQuery() {
    if (typeof window === 'undefined') return;
    const p = new URLSearchParams(window.location.search);
    const s = p.get('sphere');
    if (s === 'federal' || s === 'estadual' || s === 'municipal') sphere = s;
    uf = (p.get('uf') ?? '').toUpperCase();
    municipio = p.get('municipio') ?? '';
    q = p.get('q') ?? '';
  }
  function writeQuery() {
    if (typeof window === 'undefined') return;
    const p = new URLSearchParams();
    if (sphere) p.set('sphere', sphere);
    if (uf) p.set('uf', uf);
    if (municipio) p.set('municipio', municipio);
    if (q) p.set('q', q);
    const str = p.toString();
    history.replaceState(null, '', str ? `?${str}` : window.location.pathname);
  }

  // Rule: user must satisfy the required combination before we fire a query.
  let ready = $derived.by<boolean>(() => {
    if (!sphere) return false;
    if (sphere === 'federal') return true;
    if (sphere === 'estadual') return uf.length === 2;
    if (sphere === 'municipal') return uf.length === 2 && municipio.length > 0;
    return false;
  });

  let needsMunicipio = $derived(sphere === 'municipal');
  let needsUf = $derived(sphere !== '' && sphere !== 'federal');

  // Human-readable gate hint — shown as the empty state before `ready`.
  let gateHint = $derived.by<string>(() => {
    if (!sphere) return 'Escolha uma esfera para começar.';
    if (needsUf && !uf) return 'Escolha um estado.';
    if (needsMunicipio && !municipio) return 'Escolha um município do estado.';
    return '';
  });

  async function loadMunicipios() {
    if (!uf) {
      municipios = [];
      return;
    }
    munLoading = true;
    munErr = null;
    const res = await listMunicipios(uf);
    munLoading = false;
    if (res.success && res.data) {
      municipios = res.data;
    } else {
      munErr = res.error?.message ?? 'Falha ao carregar municípios.';
    }
  }

  async function loadItems(reset = true) {
    if (!ready || !sphere) return;
    loading = true;
    listErr = null;
    if (reset) {
      offset = 0;
      items = [];
    }
    const res = await browsePoliticos({
      sphere,
      uf: needsUf ? uf : undefined,
      municipio: needsMunicipio ? municipio : undefined,
      q: q || undefined,
      limit: PAGE,
      offset,
    });
    loading = false;
    if (res.success && res.data) {
      total = res.data.total;
      items = reset
        ? res.data.items
        : [...items, ...res.data.items];
    } else {
      listErr = res.error?.message ?? 'Falha ao carregar políticos.';
    }
  }

  function apply() {
    writeQuery();
    loadItems(true);
  }

  // When the sphere / uf changes, cascade-reset the dependent fields + reload
  // the dropdown choices so the UI doesn't hold stale values.
  function onSphereChange() {
    // Federal has no UF/municipio; clear both.
    if (sphere !== 'municipal') municipio = '';
    if (sphere === 'federal') uf = '';
    writeQuery();
    if (ready) loadItems(true);
    else items = [];
  }
  function onUfChange() {
    municipio = '';
    municipios = [];
    writeQuery();
    if (needsMunicipio && uf) loadMunicipios();
    if (ready) loadItems(true);
    else items = [];
  }
  function onMunicipioChange() {
    writeQuery();
    if (ready) loadItems(true);
  }

  onMount(() => {
    readQuery();
    if (needsMunicipio && uf) loadMunicipios();
    if (ready) loadItems(true);
  });

  function sphereLabel(s: Sphere): string {
    return s === 'federal' ? 'Federal' : s === 'estadual' ? 'Estadual' : 'Municipal';
  }
</script>

<section class="wrap">
  <div class="filters">
    <label>
      <span class="k">Esfera</span>
      <select bind:value={sphere} onchange={onSphereChange}>
        <option value="">Escolher esfera…</option>
        <option value="federal">Federal (Câmara + Senado)</option>
        <option value="estadual">Estadual (Assembleias)</option>
        <option value="municipal">Municipal (Prefeituras + Câmaras)</option>
      </select>
    </label>

    {#if needsUf}
      <label>
        <span class="k">Estado</span>
        <select bind:value={uf} onchange={onUfChange}>
          <option value="">Escolher UF…</option>
          {#each UFS as u}
            <option value={u}>{u}</option>
          {/each}
        </select>
      </label>
    {/if}

    {#if needsMunicipio}
      <label class="mun">
        <span class="k">Município</span>
        <select
          bind:value={municipio}
          onchange={onMunicipioChange}
          disabled={!uf || munLoading}
        >
          <option value="">
            {munLoading
              ? 'Carregando…'
              : uf
                ? `Escolher município (${municipios.length})…`
                : 'Escolha um estado primeiro'}
          </option>
          {#each municipios as m}
            <option value={m.nome}>{m.nome} ({m.count})</option>
          {/each}
        </select>
      </label>
    {/if}

    <div class="q-wrap">
      <Input
        id="pol-q"
        label=""
        placeholder="Buscar por nome…"
        bind:value={q}
        onkeydown={(e: KeyboardEvent) => e.key === 'Enter' && apply()}
        leading={searchIcon}
      />
    </div>
    <Button
      variant="primary"
      onclick={apply}
      disabled={!ready}
      loading={loading && offset === 0}
    >
      Buscar
    </Button>
  </div>

  {#snippet searchIcon()}<Icon name="search" size={16} />{/snippet}

  {#if munErr}
    <Alert tone="danger">{munErr}</Alert>
  {/if}

  {#if !ready}
    <Card padding="none">
      <EmptyState
        icon="filter"
        title="Configure o filtro pra começar"
        description={gateHint || 'Escolha esfera → estado → município (se aplicável).'}
      />
    </Card>
  {:else if listErr}
    <Alert tone="danger">{listErr}</Alert>
  {:else if loading && items.length === 0}
    <div class="loading"><Spinner /></div>
  {:else if items.length === 0}
    <Card padding="none">
      <EmptyState
        icon="users"
        title="Nenhum político encontrado"
        description="Ajuste os filtros ou aguarde a próxima carga de dados."
      />
    </Card>
  {:else}
    <div class="results-head">
      <span class="total">
        <strong>{new Intl.NumberFormat('pt-BR').format(total)}</strong> político(s)
        {#if sphere}
          — <em>{sphereLabel(sphere)}</em>
        {/if}
        {#if uf}· <em>{uf}</em>{/if}
        {#if municipio}· <em>{municipio}</em>{/if}
      </span>
    </div>

    <ul class="grid">
      {#each items as m (m.id)}
        <li class="p-card">
          <a class="link" href={`/politicos/?id=${m.id}`}>
            <Avatar src={m.avatar_url} name={m.display_name} size="lg" />
            <div class="meta">
              <strong class="name">
                {m.display_name}
                {#if m.has_verified_operator}
                  <span class="verified" aria-label="Vínculo verificado">
                    <Icon name="verified" size={14} />
                  </span>
                {/if}
              </strong>
              <span class="muted office">{m.office}</span>
              {#if m.party}
                <Badge tone="neutral" size="sm">{m.party}</Badge>
              {/if}
            </div>
          </a>
        </li>
      {/each}
    </ul>

    {#if items.length < total}
      <div class="more">
        <Button
          variant="ghost"
          onclick={() => {
            offset += PAGE;
            loadItems(false);
          }}
          loading={loading}
        >
          Carregar mais ({items.length}/{new Intl.NumberFormat('pt-BR').format(total)})
        </Button>
      </div>
    {/if}
  {/if}
</section>

<style>
  .wrap {
    display: block;
  }
  .filters {
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    padding: var(--sp-4);
    background: var(--surface-2);
    border-radius: var(--r-base);
    margin-bottom: var(--sp-5);
    align-items: end;
  }
  .filters label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: var(--fs-xs);
    color: var(--text-3);
    font-weight: var(--fw-medium);
  }
  .filters .k {
    color: var(--text-3);
    text-transform: uppercase;
    letter-spacing: 0.03em;
  }
  .filters select {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    color: var(--text-1);
    padding: var(--sp-2) var(--sp-3);
    border-radius: var(--r-sm);
    font: inherit;
    font-size: var(--fs-sm);
    cursor: pointer;
  }
  .filters select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .filters .mun {
    grid-column: span 2;
  }
  .q-wrap {
    grid-column: span 2;
    min-width: 220px;
  }
  .loading {
    display: flex;
    justify-content: center;
    padding: var(--sp-8);
  }
  .results-head {
    margin-bottom: var(--sp-3);
  }
  .total {
    color: var(--text-3);
    font-size: var(--fs-sm);
  }
  .total strong {
    color: var(--text-1);
    font-variant-numeric: tabular-nums;
  }
  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
  }
  .p-card {
    padding: 0;
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    overflow: hidden;
    transition:
      transform var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .p-card:hover {
    transform: translateY(-2px);
    box-shadow: var(--shadow-lg);
    border-color: var(--border-strong);
  }
  .link {
    display: flex;
    gap: var(--sp-3);
    padding: var(--sp-3);
    text-decoration: none;
    color: inherit;
    align-items: center;
  }
  .meta {
    display: grid;
    gap: 4px;
    min-width: 0;
  }
  .name {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    color: var(--text-1);
    font-size: var(--fs-base);
  }
  .verified {
    color: var(--accent);
    display: inline-flex;
  }
  .office {
    font-size: var(--fs-xs);
  }
  .more {
    display: flex;
    justify-content: center;
    padding: var(--sp-4) 0;
  }
</style>
