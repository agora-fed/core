<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListIpRules,
    adminAddIpRule,
    adminRemoveIpRule,
    type IpRuleDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<IpRuleDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let cidr = $state('');
  let scope = $state<IpRuleDto['scope']>('signup');
  let ruleState = $state<IpRuleDto['state']>('deny');
  let reason = $state('');
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListIpRules();
    loading = false;
    if (res.success && res.data) { items = res.data; error = null; }
    else { error = res.error?.message ?? 'Falha.'; }
  }
  onMount(reload);

  async function onAdd(e: SubmitEvent) {
    e.preventDefault();
    const c = cidr.trim();
    if (!c) return;
    creating = true;
    const res = await adminAddIpRule({ cidr: c, scope, state: ruleState, reason: reason.trim() || undefined });
    creating = false;
    if (res.success) {
      cidr = ''; reason = ''; scope = 'signup'; ruleState = 'deny';
      toast.success('Regra aplicada.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function onRemove(id: string) {
    if (busy.has(id) || !confirm('Remover?')) return;
    busy = new Set(busy).add(id);
    const res = await adminRemoveIpRule(id);
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
  <form onsubmit={onAdd} class="form">
    <label class="fld">
      <span>CIDR / IP</span>
      <input type="text" bind:value={cidr} placeholder="ex.: 203.0.113.5 ou 192.0.2.0/24" required />
    </label>
    <label class="fld">
      <span>Scope</span>
      <select bind:value={scope}>
        <option value="signup">Cadastro</option>
        <option value="login">Login</option>
        <option value="all">Ambos</option>
      </select>
    </label>
    <label class="fld">
      <span>Estado</span>
      <select bind:value={ruleState}>
        <option value="deny">Negar</option>
        <option value="allow">Permitir</option>
      </select>
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
    <Card><EmptyState icon="block" title="Sem regras" description="Adicione a primeira acima." /></Card>
  {:else}
    <ul class="rows">
      {#each items as r (r.id)}
        <li>
          <Card>
            <div class="row">
              <code>{r.cidr}</code>
              <Badge tone={r.state === 'deny' ? 'danger' : 'success'} size="sm">
                {r.state === 'deny' ? 'Negar' : 'Permitir'} · {r.scope}
              </Badge>
              {#if r.reason}<span class="muted">{r.reason}</span>{/if}
              <div class="spacer"></div>
              <Button variant="ghost" size="sm" onclick={() => onRemove(r.id)} loading={busy.has(r.id)}>
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
    grid-template-columns: 1fr 140px 140px auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld { display: flex; flex-direction: column; gap: var(--sp-1); font-size: var(--fs-sm); font-weight: var(--fw-semibold); }
  .fld.wide { grid-column: 1 / -1; }
  .fld input, .fld select {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit; font-size: var(--fs-sm);
  }
  @media (max-width: 800px) { .form { grid-template-columns: 1fr; } }
  .list { margin-top: var(--sp-4); }
  .rows { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
  .row { display: flex; align-items: center; gap: var(--sp-3); flex-wrap: wrap; }
  code { font-family: ui-monospace, SFMono-Regular, monospace; background: var(--surface-2); padding: 2px 8px; border-radius: 4px; }
  .spacer { flex: 1; }
</style>
