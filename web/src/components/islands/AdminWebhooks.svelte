<script lang="ts">
  import { onMount } from 'svelte';
  import {
    adminListWebhooks,
    adminCreateWebhook,
    adminUpdateWebhook,
    adminDeleteWebhook,
    type WebhookDto,
    type WebhookWithSecretDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  const EVENTS = [
    { id: 'report.created', label: 'Denúncia criada' },
    { id: 'account.approved', label: 'Conta aprovada' },
    { id: 'account.suspended', label: 'Conta suspensa' },
    { id: 'account.silenced', label: 'Conta silenciada' },
  ] as const;

  let loading = $state(true);
  let items = $state<WebhookDto[]>([]);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let url = $state('');
  let selected = $state<Record<string, boolean>>({});
  let creating = $state(false);
  let secretShown = $state<WebhookWithSecretDto | null>(null);

  async function reload() {
    loading = true;
    const res = await adminListWebhooks();
    loading = false;
    if (res.success && res.data) { items = res.data; error = null; }
    else { error = res.error?.message ?? 'Falha.'; }
  }
  onMount(reload);

  async function onCreate(e: SubmitEvent) {
    e.preventDefault();
    const events = Object.keys(selected).filter((k) => selected[k]);
    if (events.length === 0) {
      toast.error('Selecione ao menos 1 evento.');
      return;
    }
    creating = true;
    const res = await adminCreateWebhook(url.trim(), events);
    creating = false;
    if (res.success && res.data) {
      secretShown = res.data;
      url = '';
      selected = {};
      toast.success('Criado. Copie o segredo — só aparece agora.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function onToggle(w: WebhookDto) {
    if (busy.has(w.id)) return;
    busy = new Set(busy).add(w.id);
    const res = await adminUpdateWebhook(w.id, !w.enabled);
    const done = new Set(busy); done.delete(w.id); busy = done;
    if (res.success) {
      items = items.map((x) => x.id === w.id ? { ...x, enabled: !x.enabled } : x);
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function onDelete(w: WebhookDto) {
    if (busy.has(w.id) || !confirm('Apagar webhook?')) return;
    busy = new Set(busy).add(w.id);
    const res = await adminDeleteWebhook(w.id);
    const done = new Set(busy); done.delete(w.id); busy = done;
    if (res.success) {
      items = items.filter((x) => x.id !== w.id);
      toast.success('Apagado.');
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  async function copySecret() {
    if (!secretShown) return;
    try {
      await navigator.clipboard.writeText(secretShown.secret);
      toast.success('Segredo copiado.');
    } catch {
      toast.error('Copie manualmente.');
    }
  }
</script>

<Card>
  <h2 class="sub">Novo webhook</h2>
  <form onsubmit={onCreate} class="form">
    <label class="fld wide">
      <span>URL de destino</span>
      <input type="url" bind:value={url} placeholder="https://…" required />
    </label>
    <fieldset class="fld wide events">
      <legend>Eventos</legend>
      {#each EVENTS as ev (ev.id)}
        <label class="chk">
          <input type="checkbox" bind:checked={selected[ev.id]} />
          <span>{ev.label} <code>{ev.id}</code></span>
        </label>
      {/each}
    </fieldset>
    <Button type="submit" variant="primary" loading={creating} disabled={!url.trim()}>Criar</Button>
  </form>
</Card>

{#if secretShown}
  <Card>
    <h3 class="sub">Segredo do webhook <code>{secretShown.url}</code></h3>
    <p class="muted small">
      Use pra validar o header <code>X-DemocraciaBR-Signature: sha256=…</code>.
      <strong>Só aparece agora.</strong>
    </p>
    <div class="secret">
      <code class="sec">{secretShown.secret}</code>
      <Button variant="ghost" size="sm" onclick={copySecret}>Copiar</Button>
      <Button variant="ghost" size="sm" onclick={() => (secretShown = null)}>Ok, guardei</Button>
    </div>
  </Card>
{/if}

<div class="list">
  {#if loading}<p class="muted">Carregando…</p>
  {:else if error}<Card><EmptyState icon="bell" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card><EmptyState icon="bell" title="Sem webhooks" description="Adicione o primeiro acima." /></Card>
  {:else}
    <ul class="rows">
      {#each items as w (w.id)}
        <li>
          <Card>
            <div class="row">
              <div class="info">
                <code class="u">{w.url}</code>
                <div class="ev">
                  {#each w.events as e (e)}
                    <Badge tone="info" size="sm">{e}</Badge>
                  {/each}
                </div>
                <div class="muted small">
                  {#if w.enabled}<Badge tone="success" size="sm">Ativo</Badge>{:else}<Badge tone="neutral" size="sm">Pausado</Badge>{/if}
                  {#if w.last_status !== null}
                    · última resposta HTTP <strong>{w.last_status}</strong> em {formatDate(w.last_delivery_at ?? '')}
                  {/if}
                </div>
              </div>
              <div class="a">
                <Button variant="ghost" size="sm" onclick={() => onToggle(w)} loading={busy.has(w.id)}>
                  {w.enabled ? 'Pausar' : 'Ativar'}
                </Button>
                <Button variant="danger" size="sm" onclick={() => onDelete(w)} loading={busy.has(w.id)}>
                  Apagar
                </Button>
              </div>
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sub { margin: 0 0 var(--sp-3); font-size: var(--fs-md); }
  .small { font-size: var(--fs-sm); }
  .form {
    display: grid;
    grid-template-columns: 1fr auto;
    gap: var(--sp-3);
    align-items: end;
  }
  .fld { display: flex; flex-direction: column; gap: var(--sp-1); font-size: var(--fs-sm); font-weight: var(--fw-semibold); }
  .fld.wide { grid-column: 1 / -1; }
  .fld input {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit; font-size: var(--fs-sm);
  }
  .events { border: 1px solid var(--border-subtle); border-radius: var(--r-sm); padding: var(--sp-3); }
  .events legend { padding: 0 var(--sp-1); font-size: var(--fs-xs); text-transform: uppercase; color: var(--text-3); }
  .chk { display: flex; align-items: center; gap: var(--sp-2); font-size: var(--fs-sm); padding: 4px 0; font-weight: var(--fw-medium); }
  .chk code { font-family: ui-monospace, SFMono-Regular, monospace; background: var(--surface-2); padding: 1px 5px; border-radius: 4px; }
  .secret { display: flex; align-items: center; gap: var(--sp-2); margin-top: var(--sp-2); flex-wrap: wrap; }
  .sec { font-family: ui-monospace, SFMono-Regular, monospace; background: var(--surface-2); padding: 6px 10px; border-radius: var(--r-sm); word-break: break-all; flex: 1; min-width: 200px; }
  .list { margin-top: var(--sp-4); }
  .rows { list-style: none; margin: 0; padding: 0; display: grid; gap: var(--sp-3); }
  .row { display: flex; gap: var(--sp-3); align-items: flex-start; }
  .info { flex: 1; min-width: 0; }
  .u { font-family: ui-monospace, SFMono-Regular, monospace; background: var(--surface-2); padding: 2px 8px; border-radius: 4px; display: inline-block; word-break: break-all; }
  .ev { display: flex; gap: var(--sp-1); flex-wrap: wrap; margin: var(--sp-1) 0; }
  .a { display: flex; gap: var(--sp-1); flex-shrink: 0; }
</style>
