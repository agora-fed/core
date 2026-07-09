<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListEmailDomains,
    adminAddEmailDomain,
    adminRemoveEmailDomain,
    type EmailDomainDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<EmailDomainDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let domain = $state('');
  let reason = $state('');
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListEmailDomains();
    loading = false;
    if (res.success && res.data) { items = res.data; error = null; }
    else { error = res.error?.message ?? 'Falha.'; }
  }
  onMount(reload);

  async function onAdd(e: SubmitEvent) {
    e.preventDefault();
    const d = domain.trim().toLowerCase();
    if (!d) return;
    creating = true;
    const res = await adminAddEmailDomain(d, reason.trim() || undefined);
    creating = false;
    if (res.success) {
      domain = ''; reason = '';
      toast.success('Adicionado.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function onRemove(d: string) {
    if (busy.has(d) || !confirm(`Remover ${d}?`)) return;
    busy = new Set(busy).add(d);
    const res = await adminRemoveEmailDomain(d);
    const done = new Set(busy); done.delete(d); busy = done;
    if (res.success) {
      items = items.filter((i) => i.domain !== d);
      toast.success('Removido.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }
</script>

<Card>
  <form onsubmit={onAdd} class="form">
    <label class="fld">
      <span>Domínio</span>
      <input type="text" bind:value={domain} placeholder="ex.: mailinator.com" required />
    </label>
    <label class="fld wide">
      <span>Motivo (opcional)</span>
      <input type="text" bind:value={reason} maxlength="500" />
    </label>
    <Button type="submit" variant="danger" loading={creating}>Adicionar</Button>
  </form>
</Card>

<div class="list">
  {#if loading}<p class="muted">Carregando…</p>
  {:else if error}<Card><EmptyState icon="block" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card><EmptyState icon="block" title="Sem domínios bloqueados" description="Adicione um acima quando precisar." /></Card>
  {:else}
    <ul class="rows">
      {#each items as d (d.domain)}
        <li>
          <Card>
            <div class="row">
              <code>{d.domain}</code>
              {#if d.reason}<span class="muted">{d.reason}</span>{/if}
              <span class="muted t">{formatDate(d.created_at)}</span>
              <div class="spacer"></div>
              <Button variant="ghost" size="sm" onclick={() => onRemove(d.domain)} loading={busy.has(d.domain)}>
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
  .form {
    display: grid;
    grid-template-columns: 1fr 1fr auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld { display: flex; flex-direction: column; gap: var(--sp-1); font-size: var(--fs-sm); font-weight: var(--fw-semibold); }
  .fld.wide { grid-column: 2 / -1; }
  .fld input {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit; font-size: var(--fs-sm);
  }
  @media (max-width: 700px) { .form { grid-template-columns: 1fr; } .fld.wide { grid-column: auto; } }
  .list { margin-top: var(--sp-4); }
  .rows { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
  .row { display: flex; align-items: center; gap: var(--sp-3); flex-wrap: wrap; }
  code { font-family: ui-monospace, SFMono-Regular, monospace; background: var(--surface-2); padding: 2px 8px; border-radius: 4px; }
  .t { font-size: var(--fs-xs); }
  .spacer { flex: 1; }
</style>
