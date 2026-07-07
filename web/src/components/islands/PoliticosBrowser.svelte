<script lang="ts">
  // Diretório de parlamentares — CSR pra renderizar dados live (a lista cresce
  // sem rebuild). Filtra por partido, esfera e casa (Câmara/Senado), com busca
  // textual. Cada card tem foto, nome, partido/UF, e o placar público.
  //
  // 0.17.0: adotou ui/Chip, ui/Badge, ui/Avatar, ui/Icon, ui/Skeleton, ui/Input.
  import { onMount } from 'svelte';
  import {
    getAllMandates,
    getScorecards,
    DEFAULT_ORG_ID,
    type MandateDto,
  } from '../../lib/api';
  import type { ScorecardDto } from '../../lib/types';
  import { responseRate } from '../../lib/format';
  import Chip from '../ui/Chip.svelte';
  import Badge from '../ui/Badge.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import Input from '../ui/Input.svelte';

  let loading = $state(true);
  let mandates = $state<MandateDto[]>([]);
  let cards = $state<Map<string, ScorecardDto>>(new Map());
  let loadError = $state<string | null>(null);

  let search = $state('');
  let activeParty = $state<string | 'TODOS'>('TODOS');
  let activeHouse = $state<'todos' | 'camara' | 'senado'>('todos');
  let activeSphere = $state<'todos' | 'federal' | 'estadual' | 'municipal'>(
    'todos',
  );
  let sortBy = $state<'nome' | 'resposta' | 'silencio'>('nome');

  let parties = $derived(
    Array.from(
      new Set(mandates.map((m) => m.party).filter((p): p is string => !!p)),
    ).sort(),
  );
  let sphereCounts = $derived({
    federal: mandates.filter((m) => (m.sphere ?? 'federal') === 'federal').length,
    estadual: mandates.filter((m) => m.sphere === 'estadual').length,
    municipal: mandates.filter((m) => m.sphere === 'municipal').length,
  });

  let filtered = $derived(
    mandates.filter((m) => {
      if (activeParty !== 'TODOS' && m.party !== activeParty) return false;
      if (activeHouse !== 'todos' && m.house !== activeHouse) return false;
      if (activeSphere !== 'todos' && (m.sphere ?? 'federal') !== activeSphere)
        return false;
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
        const ra = totalA > 0 ? ca!.answered / totalA : -1;
        const rb = totalB > 0 ? cb!.answered / totalB : -1;
        if (rb !== ra) return rb - ra;
      } else if (sortBy === 'silencio') {
        const ia = totalA > 0 ? ca!.ignored / totalA : -1;
        const ib = totalB > 0 ? cb!.ignored / totalB : -1;
        if (ib !== ia) return ib - ia;
      }
      return a.display_name.localeCompare(b.display_name, 'pt-BR');
    }),
  );

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
      getAllMandates(DEFAULT_ORG_ID),
      getScorecards(DEFAULT_ORG_ID, 200),
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

  type BadgeTone = 'pending' | 'answered' | 'acted' | 'ignored';
  function badge(
    c: ScorecardDto | undefined,
  ): { tone: BadgeTone; label: string } {
    if (!c) return { tone: 'pending', label: 'Sem demandas' };
    const rate = responseRate(c.answered, c.ignored);
    if (rate === null) return { tone: 'pending', label: 'Sem registros' };
    if (rate >= 70) return { tone: 'answered', label: `${rate}% responde` };
    if (rate >= 40) return { tone: 'acted', label: `${rate}% responde` };
    return { tone: 'ignored', label: `${rate}% responde` };
  }
</script>

{#if loading}
  <div class="loading">
    <Input placeholder="Buscar…" disabled />
    <ul class="grid">
      {#each Array.from({ length: 6 }) as _, i (i)}
        <li class="sk-card">
          <Skeleton variant="circle" width="60px" />
          <div style="flex:1">
            <Skeleton width="70%" />
            <Skeleton width="45%" />
            <Skeleton width="30%" />
          </div>
        </li>
      {/each}
    </ul>
  </div>
{:else if loadError}
  <p class="hint hint-error" role="alert">{loadError}</p>
{:else}
  <section class="filters" aria-label="Filtros">
    <Input
      type="search"
      placeholder="Buscar por nome, partido ou estado…"
      autocomplete="off"
      bind:value={search}
      leading={searchIcon}
    />
    {#snippet searchIcon()}<Icon name="search" size={16} />{/snippet}

    <div class="chips" aria-label="Partido">
      <Chip
        selected={activeParty === 'TODOS'}
        onclick={() => (activeParty = 'TODOS')}
      >
        Todos
      </Chip>
      {#each parties as p (p)}
        <Chip
          selected={activeParty === p}
          onclick={() => (activeParty = p)}
        >
          {p}
        </Chip>
      {/each}
    </div>

    <div class="chips" aria-label="Esfera de governo">
      <span class="chip-label">Esfera:</span>
      <Chip
        selected={activeSphere === 'todos'}
        onclick={() => (activeSphere = 'todos')}
      >
        Todas
      </Chip>
      <button
        type="button"
        class="chip-disabled-wrap"
        onclick={() => sphereCounts.federal > 0 && (activeSphere = 'federal')}
        disabled={sphereCounts.federal === 0}
        title={sphereCounts.federal === 0
          ? 'Sem dados de esfera federal ainda'
          : ''}
      >
        <Chip
          selected={activeSphere === 'federal'}
          interactive={false}
        >
          Federal <span class="chip-count">{sphereCounts.federal}</span>
        </Chip>
      </button>
      <button
        type="button"
        class="chip-disabled-wrap"
        onclick={() =>
          sphereCounts.estadual > 0 && (activeSphere = 'estadual')}
        disabled={sphereCounts.estadual === 0}
        title={sphereCounts.estadual === 0
          ? 'Estadual: em breve (Assembleias legislativas + governadorias)'
          : ''}
      >
        <Chip
          selected={activeSphere === 'estadual'}
          interactive={false}
        >
          Estadual <span class="chip-count">{sphereCounts.estadual}</span>
        </Chip>
      </button>
      <button
        type="button"
        class="chip-disabled-wrap"
        onclick={() =>
          sphereCounts.municipal > 0 && (activeSphere = 'municipal')}
        disabled={sphereCounts.municipal === 0}
        title={sphereCounts.municipal === 0
          ? 'Municipal: em breve (Câmaras municipais + prefeituras)'
          : ''}
      >
        <Chip
          selected={activeSphere === 'municipal'}
          interactive={false}
        >
          Municipal <span class="chip-count">{sphereCounts.municipal}</span>
        </Chip>
      </button>
    </div>

    <div class="chips" aria-label="Casa">
      <Chip
        selected={activeHouse === 'todos'}
        onclick={() => (activeHouse = 'todos')}
      >
        Câmara + Senado
      </Chip>
      <Chip
        selected={activeHouse === 'camara'}
        onclick={() => (activeHouse = 'camara')}
      >
        Câmara
      </Chip>
      <Chip
        selected={activeHouse === 'senado'}
        onclick={() => (activeHouse = 'senado')}
      >
        Senado
      </Chip>
    </div>

    <div class="chips" aria-label="Ordenar por">
      <span class="chip-label">Ordenar:</span>
      <Chip selected={sortBy === 'nome'} onclick={() => (sortBy = 'nome')}>
        <Icon name="sort" size={12} />nome
      </Chip>
      <Chip
        selected={sortBy === 'resposta'}
        onclick={() => (sortBy = 'resposta')}
      >
        <Icon name="sla-answered" size={12} />quem mais responde
      </Chip>
      <Chip
        selected={sortBy === 'silencio'}
        onclick={() => (sortBy = 'silencio')}
      >
        <Icon name="sla-ignored" size={12} />quem mais ignora
      </Chip>
    </div>
    <p class="count muted">{sorted.length} mostrando</p>
  </section>

  {#if sorted.length === 0}
    <p class="muted center">Ninguém bate esse filtro.</p>
  {:else if groupedByParty}
    {#each Object.entries(groupedByParty) as [party, group] (party)}
      <section class="party-group">
        <h2>
          {party} <span class="muted">({group.length})</span>
        </h2>
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
    <li class="p-card">
      <a class="link" href={`/politicos/${m.id}`}>
        <Avatar src={m.avatar_url} name={m.display_name} size="lg" />
        <div class="meta">
          <strong class="name">
            {m.display_name}
            {#if m.has_verified_operator}
              <span
                class="verified"
                title="Mandato com operador verificado"
                aria-label="Vínculo verificado"
              >
                <Icon name="verified" size={14} />
              </span>
            {/if}
          </strong>
          <span class="muted office">
            {m.party}/{m.uf} ·
            {m.house === 'camara'
              ? 'Câmara'
              : m.house === 'senado'
                ? 'Senado'
                : ''}
          </span>
          <Badge tone={b.tone} size="sm">{b.label}</Badge>
          {#if c && c.answered + c.ignored > 0}
            <span class="stats-mini muted">
              <Icon name="sla-answered" size={12} /> {c.answered} ·
              <Icon name="sla-ignored" size={12} /> {c.ignored}
            </span>
          {/if}
        </div>
      </a>
      {#if m.public_email}
        <a
          class="email-link"
          href={`mailto:${m.public_email}`}
          title="Escrever para o gabinete"
          onclick={(e) => e.stopPropagation()}
        >
          <Icon name="message" size={12} /> {m.public_email}
        </a>
      {/if}
    </li>
  {/snippet}
{/if}

<style>
  .filters {
    display: grid;
    gap: var(--sp-3);
    margin-bottom: var(--sp-6);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--sp-2);
    align-items: center;
  }
  .chip-label {
    align-self: center;
    font-size: var(--fs-sm);
    color: var(--text-3);
    font-weight: var(--fw-medium);
    margin-right: var(--sp-1);
  }
  .chip-disabled-wrap {
    background: transparent;
    border: 0;
    padding: 0;
    cursor: pointer;
  }
  .chip-disabled-wrap:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .chip-count {
    display: inline-block;
    margin-left: var(--sp-1);
    font-size: var(--fs-xs);
    color: var(--text-3);
    font-variant-numeric: tabular-nums;
  }
  .count {
    margin: 0;
    font-size: var(--fs-sm);
  }
  .party-group {
    margin-bottom: var(--sp-8);
  }
  .party-group h2 {
    font-size: var(--fs-base);
    margin: 0 0 var(--sp-3);
    color: var(--text-1);
  }
  .grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
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
    gap: var(--sp-1);
    min-width: 0;
  }
  .name {
    font-size: var(--fs-md);
    color: var(--text-1);
    line-height: 1.2;
    display: flex;
    align-items: center;
    gap: var(--sp-1);
  }
  .verified {
    display: inline-flex;
    color: var(--success);
  }
  .office {
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .stats-mini {
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
    color: var(--text-3);
    display: inline-flex;
    align-items: center;
    gap: 2px;
  }
  .center {
    text-align: center;
    margin: var(--sp-12) 0;
  }
  .email-link {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    padding: 0 var(--sp-3) var(--sp-3) calc(60px + var(--sp-3) + var(--sp-3));
    font-size: var(--fs-xs);
    text-decoration: none;
    color: var(--text-3);
    word-break: break-all;
  }
  .email-link:hover {
    color: var(--accent);
  }
  .muted {
    color: var(--text-3);
  }
  .loading .sk-card {
    display: flex;
    gap: var(--sp-3);
    padding: var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    align-items: center;
    list-style: none;
  }
  .loading ul.grid {
    margin-top: var(--sp-4);
  }
</style>
