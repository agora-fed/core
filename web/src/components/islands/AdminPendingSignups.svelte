<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListPending,
    adminApprovePending,
    adminRejectPending,
    type PendingSignupDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let loading = $state(true);
  let items = $state<PendingSignupDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  async function reload() {
    loading = true;
    const res = await adminListPending();
    loading = false;
    if (res.success && res.data) { items = res.data; error = null; }
    else { error = res.error?.message ?? 'Falha.'; }
  }
  onMount(reload);

  async function decide(id: string, action: 'approve' | 'reject') {
    if (busy.has(id)) return;
    busy = new Set(busy).add(id);
    const res = action === 'approve' ? await adminApprovePending(id) : await adminRejectPending(id);
    const done = new Set(busy); done.delete(id); busy = done;
    if (res.success) {
      items = items.filter((i) => i.citizen_id !== id);
      toast.success(action === 'approve' ? 'Aprovada.' : 'Rejeitada.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <Card><EmptyState icon="users" title="Erro" description={error} /></Card>
{:else if items.length === 0}
  <Card>
    <EmptyState
      icon="users"
      title="Fila vazia"
      description="Nada aguardando revisão. Contas caem aqui só quando a flag GATEWAY_SIGNUP_REQUIRES_REVIEW está ligada."
    />
  </Card>
{:else}
  <ul class="rows">
    {#each items as p (p.citizen_id)}
      <li>
        <Card>
          <div class="row">
            <div class="info">
              <strong>{p.display_name ?? p.handle ?? p.citizen_id.slice(0, 8)}</strong>
              {#if p.email}<span class="muted">{p.email}</span>{/if}
              <span class="muted t">criado {formatDate(p.created_at)}</span>
            </div>
            <div class="a">
              <Button variant="primary" size="sm" onclick={() => decide(p.citizen_id, 'approve')} loading={busy.has(p.citizen_id)}>
                Aprovar
              </Button>
              <Button variant="danger" size="sm" onclick={() => decide(p.citizen_id, 'reject')} loading={busy.has(p.citizen_id)}>
                Rejeitar
              </Button>
            </div>
          </div>
        </Card>
      </li>
    {/each}
  </ul>
{/if}

<style>
  .rows { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-2); }
  .row { display: flex; gap: var(--sp-3); align-items: center; flex-wrap: wrap; }
  .info { flex: 1; display: flex; flex-direction: column; gap: 2px; }
  .t { font-size: var(--fs-xs); }
  .a { display: flex; gap: var(--sp-2); }
</style>
