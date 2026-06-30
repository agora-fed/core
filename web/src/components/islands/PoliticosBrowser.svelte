<script lang="ts">
  // Diretório de parlamentares — CSR pra rendar dados live (a lista cresce sem rebuild). Filtra
  // por partido e por casa (Câmara/Senado), com busca textual. Cada card tem foto, nome,
  // partido/UF, e o pequeno badge de placar quando há scorecard registrado.
  import { onMount } from 'svelte';
  import {
    getMandates,
    getScorecards,
    DEFAULT_ORG_ID,
    type MandateDto,
  } from '../../lib/api';
  import type { ScorecardDto } from '../../lib/types';
  import { responseRate } from '../../lib/format';

  let loading = $state(true);
  let mandates = $state<MandateDto[]>([]);
  let cards = $state<Map<string, ScorecardDto>>(new Map());
  let loadError = $state<string | null>(null);

  let search = $state('');
  let activeParty = $state<string | 'TODOS'>('TODOS');
  let activeHouse = $state<'todos' | 'camara' | 'senado'>('todos');
  // Ordenação: nome (alfabético), resposta (% maior), silêncio (% ignorada maior).
  let sortBy = $state<'nome' | 'resposta' | 'silencio'>('nome');

  // Reactive views derived from filters.
  let parties = $derived(
    Array.from(
      new Set(mandates.map((m) => m.party).filter((p): p is string => !!p)),
    ).sort(),
  );
  let filtered = $derived(
    mandates.filter((m) => {
      if (activeParty !== 'TODOS' && m.party !== activeParty) return false;
      if (activeHouse !== 'todos' && m.house !== activeHouse) return false;
      if (search.trim()) {
        const q = search.toLowerCase();
        const hay = `${m.display_name} ${m.party ?? ''} ${m.uf ?? ''} ${m.office}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    }),
  );

  let sorted = $derived(
    [...filtered].sort((a, b) => {
      const ca = cards.get(a.id);
      const cb = cards.get(b.id);
      const totalA = (ca?.answered ?? 0) + (ca?.ignored ?? 0);
      const totalB = (cb?.answered ?? 0) + (cb?.ignored ?? 0);
      if (sortBy === 'resposta') {
        // Quem responde mais — quem não tem nada vai pro fim.
        const ra = totalA > 0 ? (ca!.answered / totalA) : -1;
        const rb = totalB > 0 ? (cb!.answered / totalB) : -1;
        if (rb !== ra) return rb - ra;
      } else if (sortBy === 'silencio') {
        // Quem ignora mais — destaque negativo. Sem dados, vai pro fim.
        const ia = totalA > 0 ? (ca!.ignored / totalA) : -1;
        const ib = totalB > 0 ? (cb!.ignored / totalB) : -1;
        if (ib !== ia) return ib - ia;
      }
      // Default / fallback secundário: nome.
      return a.display_name.localeCompare(b.display_name, 'pt-BR');
    }),
  );

  // Agrupar só faz sentido na ordenação por nome — nas ordenações por placar, lista contínua.
  let groupedByParty = $derived(
    sortBy === 'nome'
      ? sorted.reduce<Record<string, MandateDto[]>>((acc, m) => {
          const k = m.party ?? '—';
          acc[k] ??= [];
          acc[k].push(m);
          return acc;
        }, {})
      : null,
  );

  onMount(async () => {
    const [mr, sr] = await Promise.all([
      getMandates(DEFAULT_ORG_ID, 500),
      getScorecards(DEFAULT_ORG_ID, 500),
    ]);
    loading = false;
    if (mr.ok && mr.data) {
      mandates = mr.data;
    } else {
      loadError = mr.error ?? 'Não foi possível carregar a lista.';
      return;
    }
    if (sr.ok && sr.data) {
      cards = new Map(sr.data.map((c) => [c.mandate_id, c]));
    }
  });

  function badge(c: ScorecardDto | undefined) {
    if (!c) return { cls: 'badge-pending', label: 'Sem demandas' };
    const rate = responseRate(c.answered, c.ignored);
    if (rate === null) return { cls: 'badge-pending', label: 'Sem registros' };
    if (rate >= 70) return { cls: 'badge-answered', label: `${rate}% responde` };
    if (rate >= 40) return { cls: 'badge-acted', label: `${rate}% responde` };
    return { cls: 'badge-ignored', label: `${rate}% responde` };
  }
</script>

{#if loading}
  <p class="muted">Carregando políticos…</p>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else}
  <section class="filters" aria-label="Filtros">
    <input
      class="input search"
      type="search"
      bind:value={search}
      placeholder="Buscar por nome, partido ou estado…"
      autocomplete="off"
    />
    <div class="chips" aria-label="Partido">
      <button
        type="button"
        class={`chip ${activeParty === 'TODOS' ? 'active' : ''}`}
        onclick={() => (activeParty = 'TODOS')}
      >
        Todos
      </button>
      {#each parties as p (p)}
        <button
          type="button"
          class={`chip ${activeParty === p ? 'active' : ''}`}
          onclick={() => (activeParty = p)}
        >
          {p}
        </button>
      {/each}
    </div>
    <div class="chips" aria-label="Casa">
      <button
        type="button"
        class={`chip small ${activeHouse === 'todos' ? 'active' : ''}`}
        onclick={() => (activeHouse = 'todos')}
      >Câmara + Senado</button>
      <button
        type="button"
        class={`chip small ${activeHouse === 'camara' ? 'active' : ''}`}
        onclick={() => (activeHouse = 'camara')}
      >Câmara</button>
      <button
        type="button"
        class={`chip small ${activeHouse === 'senado' ? 'active' : ''}`}
        onclick={() => (activeHouse = 'senado')}
      >Senado</button>
    </div>
    <div class="chips" aria-label="Ordenar por">
      <span class="chip-label muted">Ordenar:</span>
      <button type="button" class={`chip small ${sortBy === 'nome' ? 'active' : ''}`}
        onclick={() => (sortBy = 'nome')}>nome</button>
      <button type="button" class={`chip small ${sortBy === 'resposta' ? 'active' : ''}`}
        onclick={() => (sortBy = 'resposta')}>quem mais responde</button>
      <button type="button" class={`chip small ${sortBy === 'silencio' ? 'active' : ''}`}
        onclick={() => (sortBy = 'silencio')}>quem mais ignora</button>
    </div>
    <p class="count muted">{sorted.length} mostrando</p>
  </section>

  {#if sorted.length === 0}
    <p class="muted center">Ninguém bate esse filtro.</p>
  {:else if groupedByParty}
    {#each Object.entries(groupedByParty) as [party, group] (party)}
      <section class="party-group">
        <h2>{party} <span class="muted">({group.length})</span></h2>
        <ul class="grid">
          {#each group as m (m.id)}
            {@render politicoCard(m)}
          {/each}
        </ul>
      </section>
    {/each}
  {:else}
    <ul class="grid">
      {#each sorted as m (m.id)}
        {@render politicoCard(m)}
      {/each}
    </ul>
  {/if}

{#snippet politicoCard(m: MandateDto)}
  {@const c = cards.get(m.id)}
  {@const b = badge(c)}
  <li class="card">
    <a class="link" href={`/politicos/${m.id}`}>
      {#if m.avatar_url}
        <img class="avatar" src={m.avatar_url} alt="" loading="lazy" />
      {:else}
        <span class="avatar avatar-placeholder">👤</span>
      {/if}
      <div class="meta">
        <strong class="name">{m.display_name}</strong>
        <span class="muted office">
          {m.party}/{m.uf} ·
          {m.house === 'camara' ? 'Câmara' : m.house === 'senado' ? 'Senado' : ''}
        </span>
        <span class={`badge ${b.cls}`}>{b.label}</span>
        {#if c && (c.answered + c.ignored) > 0}
          <span class="stats-mini muted">
            ✓ {c.answered} · ✗ {c.ignored}
          </span>
        {/if}
      </div>
    </a>
  </li>
{/snippet}
{/if}

<style>
  .filters {
    display: grid;
    gap: 0.7rem;
    margin-bottom: 1.5rem;
  }
  .search {
    width: 100%;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
  }
  .chip {
    padding: 0.45rem 0.8rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 999px;
    font-size: 0.92rem;
    cursor: pointer;
    color: var(--c-text);
    font-family: inherit;
  }
  .chip:hover {
    background: var(--c-bg);
  }
  .chip.active {
    background: var(--c-green-soft);
    border-color: var(--c-green-dark);
    color: var(--c-green-dark);
    font-weight: 600;
  }
  .chip.small {
    font-size: 0.85rem;
    padding: 0.35rem 0.7rem;
  }
  .count {
    margin: 0;
    font-size: 0.85rem;
  }
  .party-group {
    margin-bottom: 2rem;
  }
  .party-group h2 {
    font-size: 1rem;
    margin: 0 0 0.7rem;
    color: var(--c-navy);
  }
  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.7rem;
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  }
  .card {
    padding: 0;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    overflow: hidden;
    transition: transform 100ms ease, box-shadow 100ms ease;
  }
  .card:hover {
    transform: translateY(-2px);
    box-shadow: 0 6px 16px rgba(0, 0, 0, 0.07);
  }
  .link {
    display: flex;
    gap: 0.8rem;
    padding: 0.85rem;
    text-decoration: none;
    color: inherit;
    align-items: center;
  }
  .avatar {
    width: 60px;
    height: 60px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    flex-shrink: 0;
  }
  .avatar-placeholder {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 1.4rem;
  }
  .meta {
    display: grid;
    gap: 0.15rem;
    min-width: 0;
  }
  .name {
    font-size: 1rem;
    line-height: 1.2;
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .office {
    font-size: 0.85rem;
  }
  .badge {
    margin-top: 0.2rem;
    align-self: start;
  }
  .stats-mini {
    font-size: 0.78rem;
    font-variant-numeric: tabular-nums;
  }
  .chip-label {
    align-self: center;
    font-size: 0.85rem;
    margin-right: 0.3rem;
  }
  .center {
    text-align: center;
    margin: 3rem 0;
  }
</style>
