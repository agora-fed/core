<script lang="ts">
  import { onMount } from 'svelte';
  import {
    addProject,
    allocatedCents,
    canPlaceOrder,
    createBudget,
    formatCents,
    getBudget,
    listBudgets,
    listProjects,
    placeOrder,
    reaisToCents,
    toggleProject,
    type BudgetDto,
    type ProjectDto,
  } from '../../lib/budgets';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';

  let loading = $state(true);
  let loadError = $state<string | null>(null);
  let logged = $state(false);
  let budgets = $state<BudgetDto[]>([]);
  let current = $state<BudgetDto | null>(null);
  let projects = $state<ProjectDto[]>([]);
  let selected = $state<string[]>([]);
  let ordered = $state(false);
  let busy = $state(false);

  let newTitle = $state('');
  let newAmount = $state('');

  const allocated = $derived(allocatedCents(projects, selected));

  async function loadDetail(id: string) {
    const [budget, items] = await Promise.all([getBudget(id), listProjects(id)]);
    if (budget.ok && budget.data) {
      current = budget.data;
      projects = items.data ?? [];
    } else {
      loadError = budget.error ?? 'Orçamento não encontrado.';
    }
  }

  onMount(async () => {
    try {
      logged = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
    const id = new URLSearchParams(window.location.search).get('id');
    if (id) {
      await loadDetail(id);
    } else {
      const res = await listBudgets();
      if (res.ok && res.data) budgets = res.data;
      else loadError = res.error ?? 'Não foi possível carregar os orçamentos.';
    }
    loading = false;
  });

  async function create(event: SubmitEvent) {
    event.preventDefault();
    const cents = reaisToCents(newAmount);
    if (!cents) {
      toast.error('Informe um valor válido, como 1.500,00.');
      return;
    }
    busy = true;
    const res = current
      ? await addProject(current.id, newTitle, cents)
      : await createBudget(newTitle, cents);
    busy = false;
    if (!res.success) {
      toast.error(res.error?.message ?? 'Não foi possível salvar.');
    } else if (current) {
      newTitle = '';
      newAmount = '';
      await loadDetail(current.id);
    } else if (res.data) {
      window.location.href = `/orcamento/?id=${(res.data as BudgetDto).id}`;
    }
  }

  async function order() {
    if (!current) return;
    busy = true;
    const res = await placeOrder(current.id, selected);
    busy = false;
    if (res.success) ordered = true;
    else toast.error(res.error?.message ?? 'Não foi possível registrar sua escolha.');
  }
</script>

{#snippet organizerForm(heading: string, titleLabel: string, amountLabel: string)}
  <Card>
    <h2>{heading}</h2>
    <p class="muted">Exige conta com verificação de organizador.</p>
    <form onsubmit={create}>
      <label>{titleLabel} <input bind:value={newTitle} required /></label>
      <label>{amountLabel} <input bind:value={newAmount} inputmode="decimal" required /></label>
      <Button type="submit" variant="ghost" disabled={busy}>Salvar</Button>
    </form>
  </Card>
{/snippet}

{#if loading}
  <p class="muted">Carregando…</p>
{:else if loadError}
  <Card><p role="alert">{loadError}</p></Card>
{:else if current}
  <p><a href="/orcamento/">← Todos os orçamentos</a></p>
  <h1>{current.title}</h1>
  <p class="muted">Teto: {formatCents(current.ceiling_cents)}</p>

  {#if ordered}
    <p role="status">Obrigado! Sua escolha foi registrada.</p>
  {:else if projects.length === 0}
    <p class="muted">Nenhum projeto cadastrado.</p>
  {:else}
    <ul class="list">
      {#each projects as p (p.id)}
        <li>
          <label class="row">
            <input
              type="checkbox"
              disabled={!logged}
              checked={selected.includes(p.id)}
              onchange={() => (selected = toggleProject(selected, p.id))}
            />
            <span>{p.title}</span>
            <span class="muted">{formatCents(p.cost_cents)}</span>
          </label>
        </li>
      {/each}
    </ul>
    {#if logged}
      <p aria-live="polite" class:over={allocated > current.ceiling_cents}>
        Selecionado: {formatCents(allocated)} de {formatCents(current.ceiling_cents)}
      </p>
      <Button disabled={busy || !canPlaceOrder(allocated, current.ceiling_cents)} onclick={order}>
        Registrar minha escolha
      </Button>
    {:else}
      <p class="muted"><a href="/entrar">Entre</a> para escolher projetos.</p>
    {/if}
  {/if}

  {#if logged}
    {@render organizerForm('Adicionar projeto', 'Projeto', 'Custo em reais')}
  {/if}
{:else}
  <h1>Orçamento participativo</h1>
  {#if budgets.length === 0}
    <p class="muted">Nenhum orçamento por enquanto.</p>
  {:else}
    <ul class="list">
      {#each budgets as b (b.id)}
        <li>
          <a href={`/orcamento/?id=${b.id}`}>{b.title}</a>
          <span class="muted">Teto: {formatCents(b.ceiling_cents)}</span>
        </li>
      {/each}
    </ul>
  {/if}
  {#if logged}
    {@render organizerForm('Criar orçamento', 'Título', 'Teto em reais')}
  {/if}
{/if}

<style>
  .list {
    list-style: none;
    margin: 0 0 var(--sp-4);
    padding: 0;
  }
  .list li {
    display: flex;
    flex-direction: column;
    gap: 2px;
    padding: var(--sp-2) 0;
    border-top: 1px solid var(--border-subtle);
  }
  .row {
    display: flex;
    flex-direction: row;
    align-items: center;
    gap: var(--sp-2);
  }
  .row .muted {
    margin-inline-start: auto;
  }
  .over {
    color: var(--danger);
  }
  form {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--sp-3);
    margin-top: var(--sp-3);
  }
  label {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    width: 100%;
  }
</style>
