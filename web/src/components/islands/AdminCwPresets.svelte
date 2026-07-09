<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListCwPresets,
    adminCreateCwPreset,
    adminDeleteCwPreset,
    type CwPresetDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<CwPresetDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let phrase = $state('');
  let spoiler = $state('');
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListCwPresets();
    loading = false;
    if (res.success && res.data) { items = res.data; error = null; }
    else { error = res.error?.message ?? 'Falha ao carregar.'; }
  }
  onMount(reload);

  async function onAdd(e: SubmitEvent) {
    e.preventDefault();
    const p = phrase.trim();
    if (!p) return;
    creating = true;
    const res = await adminCreateCwPreset(p, spoiler.trim() || undefined);
    creating = false;
    if (res.success && res.data) {
      items = [res.data, ...items.filter((i) => i.phrase !== p)];
      phrase = '';
      spoiler = '';
      toast.success('Predefinição criada.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function onRemove(id: string) {
    if (busy.has(id) || !confirm('Remover?')) return;
    busy = new Set(busy).add(id);
    const res = await adminDeleteCwPreset(id);
    const done = new Set(busy); done.delete(id); busy = done;
    if (res.success) {
      items = items.filter((i) => i.id !== id);
      toast.success('Removida.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }
</script>

<Card>
  <h2 class="sub">Nova predefinição</h2>
  <form onsubmit={onAdd} class="form">
    <label class="fld">
      <span>Frase-gatilho (case-insensitive)</span>
      <input type="text" bind:value={phrase} maxlength="200" placeholder="ex.: spoiler" />
    </label>
    <label class="fld">
      <span>Rótulo do CW (opcional)</span>
      <input type="text" bind:value={spoiler} maxlength="200" placeholder="ex.: Spoiler" />
    </label>
    <Button type="submit" variant="primary" loading={creating} disabled={!phrase.trim()}>Adicionar</Button>
  </form>
</Card>

<div class="list">
  {#if loading}<p class="muted">Carregando…</p>
  {:else if error}<Card><EmptyState icon="cw" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState icon="cw" title="Nada configurado" description="Adicione a primeira predefinição acima." />
    </Card>
  {:else}
    <ul class="rows">
      {#each items as p (p.id)}
        <li>
          <Card>
            <div class="row">
              <code class="ph">{p.phrase}</code>
              <span class="muted">→</span>
              <span class="lbl">{p.spoiler_text ?? '(só marca sensível)'}</span>
              <div class="spacer"></div>
              <Button variant="ghost" size="sm" onclick={() => onRemove(p.id)} loading={busy.has(p.id)}>Remover</Button>
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sub { margin: 0 0 var(--sp-3); font-size: var(--fs-md); }
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
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
  .fld input {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  @media (max-width: 800px) { .form { grid-template-columns: 1fr; } }
  .list { margin-top: var(--sp-4); }
  .rows { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .ph {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--surface-2);
    padding: 2px 8px;
    border-radius: 4px;
  }
  .lbl { font-weight: var(--fw-semibold); }
  .spacer { flex: 1; }
</style>
