<script lang="ts">
  // Partidos. Fase 2 (fatia A): o catálogo de partidos é DERIVADO dos mandatos
  // (mandate.party/uf/sphere/house) — sem tabela própria ainda. Cada partido vira um card com a
  // contagem por esfera (federal/estadual/municipal), prenunciando os "diretórios" (subgrupos) que
  // a fatia B vai formalizar. Clicar leva a /partidos/{sigla}. CSR pra refletir novos seeds
  // sem rebuild.
  import { onMount } from 'svelte';
  import { getParties, type PartyDto } from '../../lib/api';
  import { partySlug, partyColor } from '../../lib/parties';

  interface PartyAgg {
    sigla: string;
    total: number;
    federal: number;
    estadual: number;
    municipal: number;
  }

  let loading = $state(true);
  let raw = $state<PartyDto[]>([]);
  let loadError = $state<string | null>(null);
  let search = $state('');
  let activeSphere = $state<'todos' | 'federal' | 'estadual' | 'municipal'>('todos');

  let parties = $derived.by<PartyAgg[]>(() => {
    // Contagens já vêm do backend (/api/v1/parties) — nada de baixar 69k mandatos.
    let list: PartyAgg[] = raw.map((p) => ({
      sigla: p.sigla,
      total: p.mandate_count,
      federal: p.federal_count,
      estadual: p.estadual_count,
      municipal: p.municipal_count,
    }));
    if (activeSphere !== 'todos') {
      list = list
        .map((p) => ({ ...p, total: p[activeSphere] }))
        .filter((p) => p.total > 0);
    }
    if (search.trim()) {
      const q = search.toLowerCase();
      list = list.filter((p) => p.sigla.toLowerCase().includes(q));
    }
    return list.sort((a, b) => b.total - a.total || a.sigla.localeCompare(b.sigla, 'pt-BR'));
  });

  let totalPoliticos = $derived(parties.reduce((n, p) => n + p.total, 0));

  onMount(async () => {
    const res = await getParties();
    loading = false;
    if (res.ok && res.data) {
      raw = res.data;
    } else {
      loadError = res.error ?? 'Não foi possível carregar os partidos.';
    }
  });
</script>

{#if loading}
  <p class="muted">Carregando partidos…</p>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else}
  <section class="filters" aria-label="Filtros">
    <input
      class="input search"
      type="search"
      bind:value={search}
      placeholder="Buscar partido (sigla)…"
      autocomplete="off"
    />
    <div class="chips" aria-label="Esfera de governo">
      <span class="chip-label muted">Esfera:</span>
      {#each [['todos', 'Todas'], ['federal', 'Federal'], ['estadual', 'Estadual'], ['municipal', 'Municipal']] as [key, label] (key)}
        <button
          type="button"
          class={`chip small ${activeSphere === key ? 'active' : ''}`}
          onclick={() => (activeSphere = key as typeof activeSphere)}
        >{label}</button>
      {/each}
    </div>
    <p class="count muted">{parties.length} partidos · {totalPoliticos} representantes</p>
  </section>

  {#if parties.length === 0}
    <p class="muted center">Nenhum partido bate esse filtro.</p>
  {:else}
    <ul class="grid">
      {#each parties as p (p.sigla)}
        <li class="card" style={`--party-accent:${partyColor(p.sigla)}`}>
          <a class="link" href={`/partidos/${partySlug(p.sigla)}`}>
            <span class="crest" aria-hidden="true">{p.sigla.slice(0, 3)}</span>
            <div class="meta">
              <strong class="name">{p.sigla}</strong>
              <span class="muted total">{p.total} representante{p.total === 1 ? '' : 's'}</span>
              <span class="spheres muted">
                {#if p.federal}<span class="pill">Federal {p.federal}</span>{/if}
                {#if p.estadual}<span class="pill">Estadual {p.estadual}</span>{/if}
                {#if p.municipal}<span class="pill">Municipal {p.municipal}</span>{/if}
              </span>
            </div>
          </a>
        </li>
      {/each}
    </ul>
  {/if}

  <p class="foot muted">
    Diretórios estaduais e municipais (subgrupos) e a administração por pessoas físicas chegam na
    próxima etapa. Hoje cada partido é derivado dos mandatos reais já carregados.
  </p>
{/if}

<style>
  .filters {
    display: grid;
    gap: 0.7rem;
    margin-bottom: 1.5rem;
  }
  .search { width: 100%; }
  .chips { display: flex; flex-wrap: wrap; gap: 0.35rem; align-items: center; }
  .chip {
    padding: 0.35rem 0.7rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 999px;
    font-size: 0.85rem;
    cursor: pointer;
    color: var(--c-text);
    font-family: inherit;
  }
  .chip:hover { background: var(--c-bg); }
  .chip.active {
    background: var(--c-green-soft);
    border-color: var(--c-green-dark);
    color: var(--c-green-dark);
    font-weight: 600;
  }
  .chip-label { font-size: 0.85rem; margin-right: 0.3rem; }
  .count { margin: 0; font-size: 0.85rem; }
  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.7rem;
    grid-template-columns: repeat(auto-fill, minmax(230px, 1fr));
  }
  .card {
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    overflow: hidden;
    border-left: 4px solid var(--party-accent, var(--c-border));
    transition: transform 100ms ease, box-shadow 100ms ease;
  }
  .card:hover { transform: translateY(-2px); box-shadow: 0 6px 16px rgba(0,0,0,0.07); }
  .link {
    display: flex;
    gap: 0.8rem;
    padding: 0.85rem;
    text-decoration: none;
    color: inherit;
    align-items: center;
  }
  .crest {
    width: 52px;
    height: 52px;
    border-radius: 10px;
    background: var(--party-accent, var(--c-bg));
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 0.9rem;
    flex-shrink: 0;
    letter-spacing: -0.02em;
  }
  .meta { display: grid; gap: 0.2rem; min-width: 0; }
  .name { font-size: 1.1rem; }
  .total { font-size: 0.85rem; }
  .spheres { display: flex; flex-wrap: wrap; gap: 0.25rem; margin-top: 0.15rem; }
  .pill {
    font-size: 0.72rem;
    background: var(--c-bg);
    border: 1px solid var(--c-border);
    border-radius: 999px;
    padding: 0.1rem 0.45rem;
    font-variant-numeric: tabular-nums;
  }
  .center { text-align: center; margin: 3rem 0; }
  .foot {
    margin-top: 2rem;
    font-size: 0.85rem;
    padding-top: 1rem;
    border-top: 1px solid var(--c-border);
  }
</style>
