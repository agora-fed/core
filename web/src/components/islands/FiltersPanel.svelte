<script lang="ts">
  // Painel de filtros pessoais de conteúdo. Usa /api/v1/filters (backend
  // já existente em social_graph.rs). Cria com contexto default 'home'
  // (feed principal); expiração opcional em 7/30 dias.
  import { onMount } from 'svelte';
  import {
    listMyFilters,
    createMyFilter,
    deleteMyFilter,
    type ContentFilterDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<ContentFilterDto[]>([]);
  let error = $state<string | null>(null);
  let removingBusy = $state<Set<string>>(new Set());

  let newPhrase = $state('');
  let expiresChoice = $state<'never' | '7' | '30'>('never');
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await listMyFilters();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      error = null;
    } else {
      error = res.error?.message ?? 'Falha ao carregar filtros.';
    }
  }
  onMount(reload);

  async function onCreate(e: SubmitEvent) {
    e.preventDefault();
    const phrase = newPhrase.trim();
    if (!phrase) return;
    creating = true;
    const expires_in =
      expiresChoice === 'never'
        ? undefined
        : expiresChoice === '7'
          ? 7 * 24 * 3600
          : 30 * 24 * 3600;
    const res = await createMyFilter(phrase, ['home'], expires_in);
    creating = false;
    if (res.success && res.data) {
      items = [res.data, ...items];
      newPhrase = '';
      expiresChoice = 'never';
      toast.success('Filtro criado.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao criar filtro.');
    }
  }

  async function onDelete(id: string) {
    if (removingBusy.has(id)) return;
    removingBusy = new Set(removingBusy).add(id);
    const res = await deleteMyFilter(id);
    const done = new Set(removingBusy);
    done.delete(id);
    removingBusy = done;
    if (res.success) {
      items = items.filter((i) => i.id !== id);
      toast.success('Filtro removido.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao remover.');
    }
  }
</script>

<Card>
  <form onsubmit={onCreate} class="form">
    <label class="fld wide">
      <span>Termo a filtrar</span>
      <input
        type="text"
        bind:value={newPhrase}
        maxlength="400"
        placeholder="ex.: bolsonaro, spoiler, criptomoeda"
        autocomplete="off"
      />
    </label>
    <label class="fld">
      <span>Expiração</span>
      <select bind:value={expiresChoice}>
        <option value="never">Nunca</option>
        <option value="7">Em 7 dias</option>
        <option value="30">Em 30 dias</option>
      </select>
    </label>
    <Button type="submit" variant="primary" loading={creating} disabled={!newPhrase.trim()}>
      Adicionar filtro
    </Button>
  </form>
</Card>

<div class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card>
      <EmptyState icon="volume-off" title="Erro" description={error} />
    </Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState
        icon="volume-off"
        title="Sem filtros ativos"
        description="Adicione um termo acima. Publicações com esse texto no corpo somem do seu feed."
      />
    </Card>
  {:else}
    <Card padding="none">
      <ul class="rows">
        {#each items as f (f.id)}
          <li>
            <div class="rowc">
              <code class="phrase">{f.phrase}</code>
              <span class="muted t">
                {#if f.expires_at}
                  Expira {formatDate(f.expires_at)}
                {:else}
                  Sem expiração
                {/if}
              </span>
              <Button
                variant="ghost"
                size="sm"
                onclick={() => onDelete(f.id)}
                loading={removingBusy.has(f.id)}
              >
                Remover
              </Button>
            </div>
          </li>
        {/each}
      </ul>
    </Card>
  {/if}
</div>

<style>
  .form {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
  }
  .fld.wide {
    grid-column: 1;
  }
  .fld input,
  .fld select {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  @media (max-width: 720px) {
    .form {
      grid-template-columns: 1fr;
    }
  }
  .list {
    margin-top: var(--sp-4);
  }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .rows li {
    border-bottom: 1px solid var(--border-subtle);
  }
  .rows li:last-child {
    border-bottom: 0;
  }
  .rowc {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3);
  }
  .phrase {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--surface-2);
    padding: 2px 8px;
    border-radius: var(--r-sm);
    flex: 1;
    min-width: 0;
    word-break: break-word;
  }
  .t {
    font-size: var(--fs-xs);
  }
</style>
