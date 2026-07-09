<script lang="ts">
  // Fila de denúncias pra moderação humana. Tabs pending / resolved / all.
  // Cada linha expande pra mostrar razão, denunciante e contagem de
  // denúncias distintas na mesma nota. Botão "Resolver" abre modal com
  // textarea de notas do moderador. Zero paginação nesta fatia — limit=100.
  import { onMount } from 'svelte';
  import {
    adminListReports,
    adminResolveReport,
    adminReopenReport,
    type AdminReportDto,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatDate, formatRelative } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Modal from '../ui/Modal.svelte';
  import Badge from '../ui/Badge.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  type Tab = 'pending' | 'resolved' | 'all';

  let tab = $state<Tab>('pending');
  let loading = $state(true);
  let items = $state<AdminReportDto[]>([]);
  let error = $state<string | null>(null);
  let expanded = $state<Set<string>>(new Set());

  let resolveOpen = $state(false);
  let resolveTarget = $state<AdminReportDto | null>(null);
  let resolveNotes = $state('');
  let resolveBusy = $state(false);
  let reopenBusy = $state<Set<string>>(new Set());

  async function reload(t: Tab = tab) {
    tab = t;
    loading = true;
    error = null;
    const res = await adminListReports(t, 100, 0);
    loading = false;
    if (res.success && res.data) {
      items = res.data;
    } else {
      error = res.error?.message ?? 'Falha ao carregar denúncias.';
    }
  }

  onMount(() => reload('pending'));

  function toggle(id: string) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }

  function askResolve(r: AdminReportDto) {
    resolveTarget = r;
    resolveNotes = '';
    resolveOpen = true;
  }

  async function submitResolve() {
    if (!resolveTarget || resolveBusy) return;
    resolveBusy = true;
    const res = await adminResolveReport(resolveTarget.id, resolveNotes.trim() || undefined);
    resolveBusy = false;
    if (res.success) {
      toast.success('Denúncia marcada como resolvida.');
      resolveOpen = false;
      resolveTarget = null;
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha ao resolver.');
    }
  }

  async function onReopen(r: AdminReportDto) {
    if (reopenBusy.has(r.id)) return;
    reopenBusy = new Set(reopenBusy).add(r.id);
    const res = await adminReopenReport(r.id);
    const done = new Set(reopenBusy);
    done.delete(r.id);
    reopenBusy = done;
    if (res.success) {
      toast.success('Denúncia reaberta.');
      reload();
    } else {
      toast.error(res.error?.message ?? 'Falha ao reabrir.');
    }
  }

  function categoryLabel(c: string): string {
    return c === 'spam' ? 'Spam' : c === 'violation' ? 'Violação de regras' : 'Outro';
  }

  function categoryTone(c: string): 'info' | 'warning' | 'danger' {
    return c === 'spam' ? 'info' : c === 'violation' ? 'danger' : 'warning';
  }

  function hostOf(url: string): string {
    try {
      return new URL(url).host;
    } catch {
      return url;
    }
  }
</script>

<nav class="tabs" aria-label="Filtro de denúncias">
  <button type="button" class:on={tab === 'pending'} onclick={() => reload('pending')}>
    Pendentes
  </button>
  <button type="button" class:on={tab === 'resolved'} onclick={() => reload('resolved')}>
    Resolvidas
  </button>
  <button type="button" class:on={tab === 'all'} onclick={() => reload('all')}>
    Todas
  </button>
</nav>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <Card>
    <EmptyState icon="flag" title="Erro" description={error} />
  </Card>
{:else if items.length === 0}
  <Card>
    <EmptyState
      icon="flag"
      title={tab === 'pending' ? 'Nada pendente' : 'Sem denúncias'}
      description={tab === 'pending'
        ? 'Fila vazia. Assim que uma denúncia for enviada, ela aparece aqui.'
        : 'Não há denúncias nesse filtro.'}
    />
  </Card>
{:else}
  <ol class="reports">
    {#each items as r (r.id)}
      <li>
        <Card>
          <header class="r-head">
            <div class="r-summary">
              <Badge tone={categoryTone(r.category)} size="sm">
                {categoryLabel(r.category)}
              </Badge>
              <span class="r-meta">
                <a href={`/publicacao/?uri=${encodeURIComponent(r.object_uri)}`} target="_blank" rel="noopener">
                  Ver publicação ↗
                </a>
                · autor <code>{hostOf(r.author_actor_url)}</code>
                {#if r.total_for_note > 1}
                  · <strong>{r.total_for_note}</strong> denúncias nesta nota
                {/if}
              </span>
              <time class="muted t" datetime={r.created_at} title={formatDate(r.created_at)}>
                {formatRelative(r.created_at)}
              </time>
            </div>
            <div class="r-actions">
              <button type="button" class="link" onclick={() => toggle(r.id)}>
                {expanded.has(r.id) ? 'Ocultar detalhes' : 'Ver detalhes'}
              </button>
              {#if r.resolved_at}
                <Button variant="ghost" size="sm" onclick={() => onReopen(r)} loading={reopenBusy.has(r.id)}>
                  Reabrir
                </Button>
              {:else}
                <Button variant="primary" size="sm" onclick={() => askResolve(r)}>
                  Resolver
                </Button>
              {/if}
            </div>
          </header>
          {#if expanded.has(r.id)}
            <div class="r-body">
              <dl class="kv">
                <dt>Denunciante</dt>
                <dd>
                  {r.reporter_display_name ?? r.reporter_handle ?? '—'}
                  {#if r.reporter_handle}
                    <span class="muted">@{r.reporter_handle}</span>
                  {/if}
                </dd>
                <dt>Autor da nota</dt>
                <dd><code>{r.author_actor_url}</code></dd>
                <dt>URI da nota</dt>
                <dd><code>{r.object_uri}</code></dd>
                {#if r.reason}
                  <dt>Descrição da denúncia</dt>
                  <dd class="reason">{r.reason}</dd>
                {/if}
                {#if r.resolved_at}
                  <dt>Resolvida em</dt>
                  <dd>{formatDate(r.resolved_at)}</dd>
                {/if}
                {#if r.resolution_notes}
                  <dt>Notas do moderador</dt>
                  <dd class="reason">{r.resolution_notes}</dd>
                {/if}
              </dl>
            </div>
          {/if}
        </Card>
      </li>
    {/each}
  </ol>
{/if}

<Modal bind:open={resolveOpen} title="Resolver denúncia" onclose={() => (resolveOpen = false)}>
  {#if resolveTarget}
    <p class="muted">
      Marca esta denúncia como <strong>resolvida</strong>. As notas ficam
      visíveis apenas para outros moderadores.
    </p>
    <label class="rlbl">
      Notas do moderador (opcional)
      <textarea
        bind:value={resolveNotes}
        rows="4"
        maxlength="2000"
        placeholder="Ex.: nota removida, autor advertido, etc."
      ></textarea>
    </label>
  {/if}
  {#snippet footer()}
    <Button variant="ghost" onclick={() => (resolveOpen = false)}>Cancelar</Button>
    <Button variant="primary" onclick={submitResolve} loading={resolveBusy}>
      Marcar como resolvida
    </Button>
  {/snippet}
</Modal>

<style>
  .tabs {
    display: flex;
    gap: var(--sp-1);
    margin-bottom: var(--sp-4);
    border-bottom: 1px solid var(--border-subtle);
  }
  .tabs button {
    background: transparent;
    border: 0;
    padding: var(--sp-2) var(--sp-3);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    color: var(--text-2);
    cursor: pointer;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }
  .tabs button:hover {
    color: var(--text-1);
  }
  .tabs button.on {
    color: var(--accent-strong);
    border-bottom-color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .reports {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .r-head {
    display: flex;
    align-items: flex-start;
    gap: var(--sp-3);
    flex-wrap: wrap;
  }
  .r-summary {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    flex-wrap: wrap;
    flex: 1;
    min-width: 250px;
  }
  .r-meta {
    font-size: var(--fs-sm);
    color: var(--text-2);
  }
  .r-meta code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    font-size: 0.85em;
  }
  .t {
    font-size: var(--fs-xs);
  }
  .r-actions {
    display: flex;
    gap: var(--sp-2);
    align-items: center;
  }
  .link {
    background: transparent;
    border: 0;
    color: var(--accent-strong);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-semibold);
    cursor: pointer;
  }
  .link:hover {
    text-decoration: underline;
  }
  .r-body {
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px dashed var(--border-subtle);
  }
  .kv {
    display: grid;
    grid-template-columns: 10rem 1fr;
    gap: var(--sp-2) var(--sp-3);
    margin: 0;
  }
  .kv dt {
    font-weight: var(--fw-semibold);
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  .kv dd {
    margin: 0;
    color: var(--text-1);
    font-size: var(--fs-sm);
    word-break: break-all;
  }
  .kv code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--surface-2);
    padding: 1px 4px;
    border-radius: 4px;
    font-size: 0.85em;
  }
  .reason {
    white-space: pre-wrap;
  }
  .rlbl {
    display: flex;
    flex-direction: column;
    gap: var(--sp-1);
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
  }
  .rlbl textarea {
    padding: var(--sp-2) var(--sp-3);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    background: var(--surface-1);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
    resize: vertical;
    min-height: 100px;
  }
  @media (max-width: 640px) {
    .kv {
      grid-template-columns: 1fr;
    }
  }
</style>
