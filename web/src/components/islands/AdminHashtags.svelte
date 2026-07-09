<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListHashtags,
    adminUpsertHashtag,
    adminDeleteHashtag,
    type HashtagModDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<HashtagModDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let tag = $state('');
  let stateSel = $state<'banned' | 'promoted'>('banned');
  let reason = $state('');
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListHashtags();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      error = null;
    } else {
      error = res.error?.message ?? 'Falha ao carregar.';
    }
  }
  onMount(reload);

  async function onAdd(e: SubmitEvent) {
    e.preventDefault();
    const t = tag.trim().replace(/^#/, '');
    if (!t) return;
    creating = true;
    const res = await adminUpsertHashtag(t, stateSel, reason.trim() || undefined);
    creating = false;
    if (res.success) {
      tag = '';
      reason = '';
      stateSel = 'banned';
      toast.success('Hashtag configurada.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function onRemove(t: string) {
    if (busy.has(t) || !confirm(`Remover moderação de #${t}?`)) return;
    busy = new Set(busy).add(t);
    const res = await adminDeleteHashtag(t);
    const done = new Set(busy);
    done.delete(t);
    busy = done;
    if (res.success) {
      items = items.filter((i) => i.tag !== t);
      toast.success('Removido.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }
</script>

<Card>
  <h2 class="sub">Nova entrada</h2>
  <form onsubmit={onAdd} class="form">
    <label class="fld">
      <span>Hashtag</span>
      <input type="text" bind:value={tag} placeholder="ex.: covid19 ou #covid19" />
    </label>
    <label class="fld">
      <span>Estado</span>
      <select bind:value={stateSel}>
        <option value="banned">Banida</option>
        <option value="promoted">Promovida</option>
      </select>
    </label>
    <label class="fld wide">
      <span>Motivo (opcional)</span>
      <input type="text" bind:value={reason} maxlength="500" placeholder="Aparece só pra outros admins" />
    </label>
    <Button type="submit" variant="primary" loading={creating} disabled={!tag.trim()}>Aplicar</Button>
  </form>
</Card>

<div class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card><EmptyState icon="hashtag" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState
        icon="hashtag"
        title="Sem moderação de hashtag"
        description="O trending funciona pelo volume natural. Adicione uma entrada acima quando quiser banir ou promover."
      />
    </Card>
  {:else}
    <ul class="rows">
      {#each items as h (h.tag)}
        <li>
          <Card>
            <div class="row">
              <code>#{h.tag}</code>
              <Badge tone={h.state === 'banned' ? 'danger' : 'success'} size="sm">
                {h.state === 'banned' ? 'Banida' : 'Promovida'}
              </Badge>
              {#if h.reason}<span class="muted">{h.reason}</span>{/if}
              <div class="spacer"></div>
              <Button variant="ghost" size="sm" onclick={() => onRemove(h.tag)} loading={busy.has(h.tag)}>
                Remover
              </Button>
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
    grid-template-columns: 1fr 150px auto;
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
  .fld.wide { grid-column: 1 / -1; }
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
  @media (max-width: 700px) {
    .form { grid-template-columns: 1fr; }
  }
  .list { margin-top: var(--sp-4); }
  .rows { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .row code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--surface-2);
    padding: 2px 8px;
    border-radius: 4px;
    font-size: var(--fs-sm);
  }
  .spacer { flex: 1; }
</style>
