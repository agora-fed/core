<script lang="ts">
  // Public page "Quem financia": the 2022 campaign of each deputy in office (TSE) and the
  // companies of the donors (Receita Federal). Open data, no sign-in (diogenes ADR-0009,
  // ADR-0010). `?deputado=<id>` opens the card, so a link can share it.
  import { onMount } from 'svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Button from '../ui/Button.svelte';
  import Input from '../ui/Input.svelte';
  import Select from '../ui/Select.svelte';
  import Modal from '../ui/Modal.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import CampaignSection from '../articulacao/CampaignSection.svelte';
  import {
    api,
    brl,
    companySourceText,
    normalize,
    safeUrl,
    type CampaignList,
    type CampaignRow,
  } from '../../lib/articulacao';

  type SortBy = 'total' | 'people' | 'companies';
  const PAGE = 60;

  let data = $state<CampaignList | null>(null);
  let error = $state('');
  let q = $state('');
  let uf = $state('');
  let party = $state('');
  let sortBy = $state<SortBy>('total');
  let shown = $state(PAGE);
  let openId = $state<number | null>(null);
  let open = $state(false);

  const selected = $derived(data?.deputies.find((d) => d.camara_id === openId) ?? null);

  const options = (values: (string | null)[], all: string) => [
    { value: '', label: all },
    ...[...new Set(values.filter((v): v is string => !!v))].sort().map((v) => ({ value: v, label: v })),
  ];
  const ufOptions = $derived(options(data?.deputies.map((d) => d.uf) ?? [], 'Todos os estados'));
  const partyOptions = $derived(options(data?.deputies.map((d) => d.party) ?? [], 'Todos os partidos'));
  const sortOptions = [
    { value: 'total', label: 'Total declarado' },
    { value: 'people', label: 'Doações de pessoas' },
    { value: 'companies', label: 'Doadores ligados a empresas' },
  ];

  const sortKey: Record<SortBy, (d: CampaignRow) => number> = {
    total: (d) => d.total_cents,
    people: (d) => d.people_cents,
    companies: (d) => d.donors_with_companies,
  };

  const rows = $derived.by(() => {
    const needle = normalize(q.trim());
    const key = sortKey[sortBy];
    return (data?.deputies ?? [])
      .filter(
        (d) =>
          (!needle || normalize(`${d.name} ${d.party ?? ''} ${d.uf ?? ''}`).includes(needle)) &&
          (!uf || d.uf === uf) &&
          (!party || d.party === party),
      )
      .sort((a, b) => key(b) - key(a) || a.name.localeCompare(b.name, 'pt-BR'));
  });

  function setUrl(id: number | null) {
    try {
      const url = new URL(window.location.href);
      if (id === null) url.searchParams.delete('deputado');
      else url.searchParams.set('deputado', String(id));
      history.replaceState(history.state, '', url);
    } catch {
      /* the card still opens */
    }
  }

  function show(id: number) {
    openId = id;
    open = true;
    setUrl(id);
  }

  function close() {
    open = false;
    setUrl(null);
  }

  onMount(async () => {
    const res = await api.campaigns();
    if (res.success && res.data) data = res.data;
    else error = res.error?.message ?? 'Não foi possível carregar.';
    const wanted = Number(new URLSearchParams(window.location.search).get('deputado'));
    if (Number.isInteger(wanted) && wanted > 0) show(wanted);
  });
</script>

<header class="intro">
  <h1>Quem financia</h1>
  <p class="lead">
    De onde veio o dinheiro da campanha de 2022 de cada deputado federal em exercício, quem doou e em que empresas os
    doadores aparecem como sócios ou administradores. Todos os dados são públicos: vêm do TSE e da Receita Federal.
  </p>
</header>

{#if error}
  <ErrorState message={error} retry={() => location.reload()} />
{:else if !data}
  <Skeleton variant="block" height="20rem" />
{:else}
  <div class="filters">
    <Input type="search" label="Buscar deputado" bind:value={q} placeholder="Nome, partido ou UF" oninput={() => (shown = PAGE)} />
    <Select label="Estado" bind:value={uf} options={ufOptions} onchange={() => (shown = PAGE)} />
    <Select label="Partido" bind:value={party} options={partyOptions} onchange={() => (shown = PAGE)} />
    <Select label="Ordenar por" bind:value={sortBy} options={sortOptions} />
  </div>

  {#if rows.length === 0}
    <EmptyState title="Nenhum deputado encontrado" description="Mude a busca ou os filtros." />
  {:else}
    <p class="muted small count">{rows.length} deputados</p>
    <ul class="list">
      {#each rows.slice(0, shown) as d (d.camara_id)}
        <li>
          <button type="button" class="row" onclick={() => show(d.camara_id)} aria-label={`Ver o financiamento de ${d.name}`}>
            <Avatar src={safeUrl(d.photo_url)} name={d.name} size="sm" />
            <span class="who">
              <span class="name">{d.name}</span>
              <span class="muted small">{[d.party, d.uf].filter(Boolean).join(' · ')}</span>
            </span>
            <span class="figures">
              <span class="total">{brl(d.total_cents)}</span>
              <span class="muted small">
                pessoas físicas {brl(d.people_cents)}{d.donors_with_companies
                  ? ` · ${d.donors_with_companies} ${d.donors_with_companies === 1 ? 'doador ligado' : 'doadores ligados'} a empresas`
                  : ''}{d.own_companies
                  ? ` · consta em ${d.own_companies} ${d.own_companies === 1 ? 'empresa' : 'empresas'}`
                  : ''}
              </span>
            </span>
          </button>
        </li>
      {/each}
    </ul>
    {#if rows.length > shown}
      <div class="more"><Button variant="secondary" onclick={() => (shown += PAGE)}>Mostrar mais</Button></div>
    {/if}
  {/if}

  <p class="muted small source">
    Campanha: TSE, prestação de contas {data.year} ·
    <a href={data.source_url} target="_blank" rel="noopener noreferrer">dados abertos ↗</a>. {companySourceText(data.companies)}.
    Doação declarada é um fato público; sozinha, não prova influência sobre o voto.
  </p>
{/if}

{#if openId !== null}
  <Modal bind:open title={selected?.name ?? 'Financiamento de campanha'} size="lg" onclose={close}>
    {#key openId}
      <CampaignSection camaraId={openId} />
    {/key}
  </Modal>
{/if}

<style>
  .intro h1 {
    margin: 0 0 var(--sp-2);
  }
  .lead {
    color: var(--text-2);
    max-width: 48rem;
    margin: 0 0 var(--sp-5);
  }
  .filters {
    display: grid;
    grid-template-columns: 2fr 1fr 1fr 1fr;
    gap: var(--sp-3);
    align-items: end;
    margin-bottom: var(--sp-4);
  }
  @media (max-width: 720px) {
    .filters {
      grid-template-columns: 1fr 1fr;
    }
    .filters > :global(:first-child) {
      grid-column: 1 / -1;
    }
  }
  .small {
    font-size: var(--fs-sm);
  }
  .count {
    margin: 0 0 var(--sp-2);
  }
  .list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-1);
  }
  .row {
    width: 100%;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: var(--sp-3);
    align-items: center;
    text-align: left;
    padding: var(--sp-2) var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-md);
    color: inherit;
    font: inherit;
    cursor: pointer;
  }
  .row:hover {
    border-color: var(--accent);
  }
  .row:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .who,
  .figures {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .figures {
    text-align: right;
  }
  .name {
    font-weight: var(--fw-semibold);
    overflow-wrap: anywhere;
  }
  .total {
    font-weight: var(--fw-semibold);
    font-variant-numeric: tabular-nums;
  }
  @media (max-width: 560px) {
    .row {
      grid-template-columns: auto minmax(0, 1fr);
    }
    .figures {
      grid-column: 2;
      text-align: left;
    }
  }
  .more {
    display: flex;
    justify-content: center;
    margin-top: var(--sp-4);
  }
  .source {
    margin-top: var(--sp-5);
  }
</style>
