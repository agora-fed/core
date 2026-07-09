<script lang="ts">
  // CRUD de anúncios. Form pra criar (com opção 'Publicar já'), tabela com
  // rascunhos e publicados, ações Publicar/Despublicar/Apagar.
  import { onMount } from 'svelte';
  import {
    adminListAnnouncements,
    adminCreateAnnouncement,
    adminPublishAnnouncement,
    adminUnpublishAnnouncement,
    adminDeleteAnnouncement,
    type AnnouncementDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let items = $state<AnnouncementDto[]>([]);
  let loading = $state(true);
  let error = $state<string | null>(null);
  let busy = $state<Set<string>>(new Set());

  let body = $state('');
  let severity = $state<'info' | 'warning' | 'critical'>('info');
  let publishNow = $state(true);
  let creating = $state(false);

  async function reload() {
    loading = true;
    const res = await adminListAnnouncements();
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      error = null;
    } else {
      error = res.error?.message ?? 'Falha ao carregar.';
    }
  }
  onMount(reload);

  async function onCreate(e: SubmitEvent) {
    e.preventDefault();
    const txt = body.trim();
    if (!txt) return;
    creating = true;
    const res = await adminCreateAnnouncement({
      body: txt,
      severity,
      publish_now: publishNow,
    });
    creating = false;
    if (res.success && res.data) {
      items = [res.data, ...items];
      body = '';
      severity = 'info';
      publishNow = true;
      toast.success('Anúncio criado.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao criar.');
    }
  }

  async function markBusy(id: string, fn: () => Promise<{ success: boolean; error?: { message?: string } }>) {
    busy = new Set(busy).add(id);
    const res = await fn();
    const done = new Set(busy);
    done.delete(id);
    busy = done;
    if (res.success) {
      toast.success('Feito.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha.');
    }
  }

  function severityTone(s: string): 'info' | 'warning' | 'danger' {
    return s === 'critical' ? 'danger' : s === 'warning' ? 'warning' : 'info';
  }
</script>

<Card>
  <h2 class="sub">Novo anúncio</h2>
  <form onsubmit={onCreate} class="form">
    <label class="fld wide">
      <span>Texto (até 4000 chars)</span>
      <textarea
        bind:value={body}
        maxlength="4000"
        rows="4"
        placeholder="Ex.: Manutenção programada para sábado 22h — o site pode ficar fora do ar por até 15 minutos."
      ></textarea>
    </label>
    <label class="fld">
      <span>Severidade</span>
      <select bind:value={severity}>
        <option value="info">Info (azul)</option>
        <option value="warning">Aviso (amarelo)</option>
        <option value="critical">Crítico (vermelho)</option>
      </select>
    </label>
    <label class="fld check">
      <input type="checkbox" bind:checked={publishNow} />
      <span>Publicar imediatamente</span>
    </label>
    <Button type="submit" variant="primary" loading={creating} disabled={!body.trim()}>
      Criar
    </Button>
  </form>
</Card>

<div class="list">
  {#if loading}
    <p class="muted">Carregando…</p>
  {:else if error}
    <Card><EmptyState icon="bell" title="Erro" description={error} /></Card>
  {:else if items.length === 0}
    <Card>
      <EmptyState
        icon="bell"
        title="Sem anúncios ainda"
        description="Use o formulário acima. Anúncios publicados aparecem em banner pra todos os cidadãos."
      />
    </Card>
  {:else}
    <ul class="rows">
      {#each items as a (a.id)}
        <li>
          <Card>
            <div class="row">
              <div class="info">
                <div class="line">
                  <Badge tone={severityTone(a.severity)} size="sm">{a.severity}</Badge>
                  {#if a.published_at}
                    <Badge tone="success" size="sm">Publicado</Badge>
                  {:else}
                    <Badge tone="neutral" size="sm">Rascunho</Badge>
                  {/if}
                  <span class="muted t">criado {formatDate(a.created_at)}</span>
                </div>
                <div class="body">{a.body}</div>
              </div>
              <div class="acts">
                {#if a.published_at}
                  <Button
                    variant="ghost"
                    size="sm"
                    onclick={() => markBusy(a.id, () => adminUnpublishAnnouncement(a.id))}
                    loading={busy.has(a.id)}
                  >Despublicar</Button>
                {:else}
                  <Button
                    variant="primary"
                    size="sm"
                    onclick={() => markBusy(a.id, () => adminPublishAnnouncement(a.id))}
                    loading={busy.has(a.id)}
                  >Publicar</Button>
                {/if}
                <Button
                  variant="danger"
                  size="sm"
                  onclick={() => confirm('Apagar anúncio?') && markBusy(a.id, () => adminDeleteAnnouncement(a.id))}
                  loading={busy.has(a.id)}
                >Apagar</Button>
              </div>
            </div>
          </Card>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .sub {
    margin: 0 0 var(--sp-3);
    font-size: var(--fs-md);
  }
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
  .fld.wide {
    grid-column: 1 / -1;
  }
  .fld.check {
    flex-direction: row;
    align-items: center;
    gap: var(--sp-2);
  }
  .fld textarea,
  .fld select {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .fld textarea { resize: vertical; }
  @media (max-width: 800px) {
    .form {
      grid-template-columns: 1fr;
    }
  }
  .list { margin-top: var(--sp-4); }
  .rows {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .row {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-start;
  }
  .info { flex: 1; min-width: 0; }
  .line {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
    margin-bottom: var(--sp-2);
    flex-wrap: wrap;
  }
  .t { font-size: var(--fs-xs); }
  .body {
    white-space: pre-wrap;
    line-height: 1.5;
    font-size: var(--fs-sm);
  }
  .acts {
    display: flex;
    gap: var(--sp-2);
    flex-shrink: 0;
    flex-wrap: wrap;
  }
</style>
