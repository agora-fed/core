<script lang="ts">
  // Detalhe de um partido. Duas camadas:
  //  1. DIRETÓRIOS REAIS (0.37.0, tabela party_directory): grupos territoriais
  //     curados — ex. "Diretório Municipal do PT — Porto Alegre". Admin de
  //     plataforma/partido cria via formulário; cada diretório expande para os
  //     membros DERIVADOS (mandatos do partido naquele território).
  //  2. REPRESENTANTES por esfera: a visão automática de todos os mandatos da
  //     sigla (federal/estadual), mantida como panorama.
  import { onMount } from 'svelte';
  import {
    getAllMandates,
    getParty,
    getDirectoryMembers,
    createPartyDirectory,
    deletePartyDirectory,
    DEFAULT_ORG_ID,
    type MandateDto,
    type PartyDirectoryDto,
    type DirectoryMemberDto,
    type CreateDirectoryFields,
  } from '../../lib/api';
  import { partyColor } from '../../lib/parties';

  let { sigla }: { sigla: string } = $props();

  let loading = $state(true);
  let members = $state<MandateDto[]>([]);
  let loadError = $state<string | null>(null);

  // Diretórios reais + membros por diretório (lazy).
  let directories = $state<PartyDirectoryDto[]>([]);
  let dirMembers = $state<Record<string, DirectoryMemberDto[]>>({});
  let expanded = $state<Record<string, boolean>>({});
  let isAdmin = $state(false);

  // Formulário de criação.
  let showForm = $state(false);
  let fEsfera = $state<'federal' | 'estadual' | 'municipal'>('municipal');
  let fUf = $state('');
  let fMunicipio = $state('');
  let fName = $state('');
  let submitting = $state(false);
  let formError = $state<string | null>(null);

  const UFS = [
    'AC','AL','AM','AP','BA','CE','DF','ES','GO','MA','MG','MS','MT','PA',
    'PB','PE','PI','PR','RJ','RN','RO','RR','RS','SC','SE','SP','TO',
  ];

  const SPHERES: Array<{ key: 'federal' | 'estadual' | 'municipal'; label: string; hint: string }> = [
    { key: 'federal', label: 'Representantes federais', hint: 'Câmara dos Deputados + Senado' },
    { key: 'estadual', label: 'Representantes estaduais', hint: 'Assembleias legislativas + governos' },
    { key: 'municipal', label: 'Representantes municipais', hint: 'Câmaras municipais + prefeituras' },
  ];

  let bySphere = $derived.by(() => {
    const groups: Record<string, MandateDto[]> = { federal: [], estadual: [], municipal: [] };
    for (const m of members) groups[m.sphere ?? 'federal'].push(m);
    for (const k of Object.keys(groups)) {
      groups[k].sort((a, b) => a.display_name.localeCompare(b.display_name, 'pt-BR'));
    }
    return groups;
  });

  let accent = $derived(partyColor(sigla));

  let formValid = $derived(
    fName.trim().length > 0 &&
      (fEsfera === 'federal' || fUf !== '') &&
      (fEsfera !== 'municipal' || fMunicipio.trim().length > 0),
  );

  function dirLabel(d: PartyDirectoryDto): string {
    const scope =
      d.esfera === 'municipal'
        ? `${d.municipio}/${d.uf}`
        : d.esfera === 'estadual'
          ? d.uf
          : 'Nacional';
    return `${d.name} · ${scope}`;
  }

  async function loadDirectories() {
    const res = await getParty(sigla);
    if (res.ok && res.data) directories = res.data.directories ?? [];
  }

  async function toggleMembers(dirId: string) {
    expanded[dirId] = !expanded[dirId];
    if (expanded[dirId] && !dirMembers[dirId]) {
      const res = await getDirectoryMembers(sigla, dirId);
      dirMembers[dirId] = res.ok && res.data ? res.data : [];
    }
  }

  async function submitCreate(event: SubmitEvent) {
    event.preventDefault();
    if (!formValid || submitting) return;
    submitting = true;
    formError = null;
    const fields: CreateDirectoryFields = {
      esfera: fEsfera,
      name: fName.trim(),
      ...(fEsfera !== 'federal' ? { uf: fUf } : {}),
      ...(fEsfera === 'municipal' ? { municipio: fMunicipio.trim() } : {}),
    };
    const res = await createPartyDirectory(sigla, fields);
    submitting = false;
    if (res.ok) {
      fName = '';
      fMunicipio = '';
      showForm = false;
      await loadDirectories();
    } else {
      formError = res.error ?? 'Não foi possível criar o diretório.';
    }
  }

  async function removeDirectory(d: PartyDirectoryDto) {
    if (!window.confirm(`Remover "${d.name}"?`)) return;
    const res = await deletePartyDirectory(sigla, d.id);
    if (res.ok) {
      await loadDirectories();
    } else {
      window.alert(res.error ?? 'Não foi possível remover.');
    }
  }

  onMount(async () => {
    try {
      isAdmin = localStorage.getItem('dsoc_is_admin') === '1';
    } catch {
      isAdmin = false;
    }
    await loadDirectories();
    // Federal + estadual only. Municipal party affiliations mirror the same
    // national siglas (~68k rows), so listing them all here would drown the UI.
    const [fed, est] = await Promise.all([
      getAllMandates(DEFAULT_ORG_ID, 5000, 'federal'),
      getAllMandates(DEFAULT_ORG_ID, 5000, 'estadual'),
    ]);
    loading = false;
    if ((fed.ok && fed.data) || (est.ok && est.data)) {
      const merged = [
        ...(fed.ok && fed.data ? fed.data : []),
        ...(est.ok && est.data ? est.data : []),
      ];
      members = merged.filter((m) => m.party === sigla);
    } else {
      loadError = fed.error ?? est.error ?? 'Não foi possível carregar o partido.';
    }
  });

  function houseLabel(m: MandateDto): string {
    if (m.house === 'camara') return 'Câmara';
    if (m.house === 'senado') return 'Senado';
    return m.office;
  }
</script>

<header class="head" style={`--party-accent:${accent}`}>
  <span class="crest" aria-hidden="true">{sigla.slice(0, 4)}</span>
  <div>
    <h1>{sigla}</h1>
    {#if !loading}
      <p class="muted">{members.length} representante{members.length === 1 ? '' : 's'} na plataforma</p>
    {/if}
  </div>
</header>

<!-- Diretórios reais (grupos territoriais curados) -->
<section class="directories-block">
  <div class="dir-head">
    <h2>Diretórios</h2>
    {#if isAdmin}
      <button type="button" class="btn-sm" onclick={() => (showForm = !showForm)}>
        {showForm ? 'Cancelar' : '+ Novo diretório'}
      </button>
    {/if}
  </div>

  {#if isAdmin && showForm}
    <form class="dir-form card" onsubmit={submitCreate}>
      <div class="row">
        <label>
          <span>Esfera</span>
          <select bind:value={fEsfera}>
            <option value="municipal">Municipal</option>
            <option value="estadual">Estadual</option>
            <option value="federal">Federal (nacional)</option>
          </select>
        </label>
        {#if fEsfera !== 'federal'}
          <label>
            <span>UF</span>
            <select bind:value={fUf}>
              <option value="" disabled>UF…</option>
              {#each UFS as uf (uf)}
                <option value={uf}>{uf}</option>
              {/each}
            </select>
          </label>
        {/if}
        {#if fEsfera === 'municipal'}
          <label class="grow">
            <span>Município</span>
            <input type="text" bind:value={fMunicipio} placeholder="Ex.: Porto Alegre" />
          </label>
        {/if}
      </div>
      <label>
        <span>Nome do diretório</span>
        <input
          type="text"
          bind:value={fName}
          placeholder="Ex.: Diretório Municipal do {sigla} — Porto Alegre"
        />
      </label>
      {#if formError}
        <p class="hint hint-error" role="alert">{formError}</p>
      {/if}
      <button type="submit" class="btn-primary" disabled={!formValid || submitting}>
        {submitting ? 'Criando…' : 'Criar diretório'}
      </button>
    </form>
  {/if}

  {#if directories.length === 0}
    <p class="muted empty">
      Nenhum diretório cadastrado ainda.{isAdmin ? ' Crie o primeiro acima.' : ''}
    </p>
  {:else}
    <ul class="dir-list">
      {#each directories as d (d.id)}
        <li class="card dir-item">
          <div class="dir-row">
            <button type="button" class="dir-toggle" onclick={() => toggleMembers(d.id)}>
              <span class="chev" class:open={expanded[d.id]}>▸</span>
              <span class="dir-name">{dirLabel(d)}</span>
            </button>
            {#if isAdmin}
              <button
                type="button"
                class="btn-remove"
                title="Remover diretório"
                onclick={() => removeDirectory(d)}
              >✕</button>
            {/if}
          </div>
          {#if expanded[d.id]}
            <div class="dir-members">
              {#if !dirMembers[d.id]}
                <p class="muted">Carregando…</p>
              {:else if dirMembers[d.id].length === 0}
                <p class="muted">Nenhum mandato do {sigla} neste território ainda.</p>
              {:else}
                <ul class="grid">
                  {#each dirMembers[d.id] as m (m.mandate_id)}
                    <li class="card member">
                      <a class="link" href={`/politicos/${m.mandate_id}`}>
                        {#if m.avatar_url}
                          <img class="avatar" src={m.avatar_url} alt="" loading="lazy" />
                        {:else}
                          <span class="avatar avatar-placeholder">👤</span>
                        {/if}
                        <div class="meta">
                          <strong class="name">{m.display_name}</strong>
                          <span class="muted office">{m.office}</span>
                        </div>
                      </a>
                    </li>
                  {/each}
                </ul>
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}
</section>

<!-- Panorama automático por esfera -->
{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else if members.length === 0}
  <div class="card center">
    <p>Nenhum representante deste partido carregado ainda.</p>
    <p class="muted"><a href="/partidos">Voltar aos partidos</a></p>
  </div>
{:else}
  {#each SPHERES as s (s.key)}
    {@const group = bySphere[s.key]}
    {#if group.length > 0}
      <section class="directory">
        <div class="dir-head">
          <h2>{s.label}</h2>
          <span class="muted count">{group.length}</span>
        </div>
        <p class="dir-hint muted">{s.hint}</p>
        <ul class="grid">
          {#each group as m (m.id)}
            <li class="card">
              <a class="link" href={`/politicos/${m.id}`}>
                {#if m.avatar_url}
                  <img class="avatar" src={m.avatar_url} alt="" loading="lazy" />
                {:else}
                  <span class="avatar avatar-placeholder">👤</span>
                {/if}
                <div class="meta">
                  <strong class="name">
                    {m.display_name}
                    {#if m.has_verified_operator}
                      <span
                        class="badge-verified badge-verified-small"
                        title="Mandato com operador verificado"
                        aria-label="Vínculo verificado"
                      >✓</span>
                    {/if}
                  </strong>
                  <span class="muted office">{houseLabel(m)}{m.uf ? ` · ${m.uf}` : ''}</span>
                </div>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {/each}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: 1rem;
    padding-bottom: 1.5rem;
    margin-bottom: 1.5rem;
    border-bottom: 1px solid var(--border-subtle, var(--c-border));
  }
  .crest {
    width: 64px;
    height: 64px;
    border-radius: 12px;
    background: var(--party-accent, var(--c-bg));
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    flex-shrink: 0;
    letter-spacing: -0.02em;
  }
  .head h1 { margin: 0 0 0.2rem; font-size: 1.6rem; }
  .head p { margin: 0; }

  .directories-block { margin-bottom: 2.5rem; }
  .directory { margin-bottom: 2rem; }
  .dir-head { display: flex; align-items: center; gap: 0.75rem; }
  .dir-head h2 { margin: 0; font-size: 1.15rem; color: var(--text-1, var(--c-navy)); }
  .count { font-variant-numeric: tabular-nums; margin-left: auto; }
  .dir-hint { margin: 0.15rem 0 0.8rem; font-size: 0.85rem; }
  .empty { margin: 0.75rem 0 0; }

  .btn-sm {
    margin-left: auto;
    padding: 0.35rem 0.8rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, var(--c-border));
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font-weight: 600;
    font-size: 0.85rem;
    cursor: pointer;
  }
  .btn-sm:hover { background: var(--surface-2, #f4f4f7); }

  .dir-form { padding: 1rem; margin: 0.75rem 0 1rem; display: grid; gap: 0.75rem; }
  .dir-form .row { display: flex; gap: 0.75rem; flex-wrap: wrap; }
  .dir-form label { display: grid; gap: 0.25rem; font-size: 0.85rem; font-weight: 600; }
  .dir-form label.grow { flex: 1; min-width: 12rem; }
  .dir-form input, .dir-form select {
    padding: 0.5rem 0.6rem;
    border-radius: 8px;
    border: 1px solid var(--border-subtle, var(--c-border));
    background: var(--surface-1, #fff);
    color: var(--text-1, inherit);
    font: inherit;
    min-width: 0;
  }
  .btn-primary {
    justify-self: start;
    padding: 0.55rem 1.1rem;
    border-radius: 8px;
    border: none;
    background: var(--accent, #15803d);
    color: var(--accent-contrast, #fff);
    font-weight: 700;
    cursor: pointer;
  }
  .btn-primary:disabled { opacity: 0.5; cursor: not-allowed; }

  .dir-list { list-style: none; padding: 0; margin: 0.75rem 0 0; display: grid; gap: 0.6rem; }
  .dir-item { padding: 0; }
  .dir-row { display: flex; align-items: center; }
  .dir-toggle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.85rem 1rem;
    background: none;
    border: none;
    cursor: pointer;
    text-align: left;
    color: var(--text-1, inherit);
    font-size: 1rem;
    font-weight: 600;
  }
  .chev { transition: transform 120ms ease; display: inline-block; color: var(--text-3, #888); }
  .chev.open { transform: rotate(90deg); }
  .btn-remove {
    padding: 0.5rem 0.8rem;
    background: none;
    border: none;
    color: var(--text-3, #999);
    cursor: pointer;
    font-size: 0.9rem;
  }
  .btn-remove:hover { color: #dc2626; }
  .dir-members { padding: 0 1rem 1rem; }

  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.7rem;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
  }
  .card {
    background: var(--surface-1, var(--c-paper));
    border: 1px solid var(--border-subtle, var(--c-border));
    border-radius: 12px;
    overflow: hidden;
  }
  .member { transition: transform 100ms ease, box-shadow 100ms ease; }
  .member:hover { transform: translateY(-2px); box-shadow: 0 6px 16px rgba(0,0,0,0.07); }
  .link { display: flex; gap: 0.8rem; padding: 0.85rem; text-decoration: none; color: inherit; align-items: center; }
  .avatar { width: 56px; height: 56px; border-radius: 50%; object-fit: cover; background: var(--c-bg); flex-shrink: 0; }
  .avatar-placeholder { display: inline-flex; align-items: center; justify-content: center; font-size: 1.4rem; }
  .meta { display: grid; gap: 0.15rem; min-width: 0; }
  .name { font-size: 1rem; line-height: 1.2; }
  .office { font-size: 0.85rem; }
  .center { text-align: center; padding: 2.5rem 1.5rem; }
  .hint-error { color: #dc2626; }
  .badge-verified {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--c-green-soft, #d4edda);
    color: var(--c-green-dark, #1e6b3a);
    font-weight: 700;
    border-radius: 999px;
    line-height: 1;
    padding: 0.15rem 0.45rem;
    margin-left: 0.35rem;
    vertical-align: middle;
  }
  .badge-verified-small { font-size: 0.7rem; padding: 0.1rem 0.35rem; }
</style>
