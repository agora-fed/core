<script lang="ts">
  // Lista de bookmarks. A API só retorna object_uris + created_at. Pra ver
  // o conteúdo, o cidadão clica em "Abrir publicação" → cai no ThreadView
  // (/publicacao/?uri=…). Botão de remover chama unbookmarkUri em cada card.
  import { onMount } from 'svelte';
  import { listBookmarks, unbookmarkUri } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import { formatRelative, formatDate } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Icon from '../ui/Icon.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  interface Row {
    object_uri: string;
    created_at: string;
  }

  let loading = $state(true);
  let error = $state<string | null>(null);
  let items = $state<Row[]>([]);
  let removing = $state<Set<string>>(new Set());

  onMount(async () => {
    if (typeof localStorage === 'undefined' || !localStorage.getItem('dsoc_citizen')) {
      loading = false;
      error = 'Entre para ver seus salvos.';
      return;
    }
    const res = await listBookmarks(30, 0);
    loading = false;
    if (!res.success || !res.data) {
      error = res.error?.message ?? 'Não consegui carregar os salvos.';
      return;
    }
    items = res.data;
  });

  async function onRemove(uri: string) {
    if (removing.has(uri)) return;
    removing = new Set(removing).add(uri);
    const res = await unbookmarkUri(uri);
    if (res.success) {
      items = items.filter((i) => i.object_uri !== uri);
      toast.success('Removido dos salvos.');
    } else {
      toast.error(res.error?.message ?? 'Falha ao remover.');
    }
    const next = new Set(removing);
    next.delete(uri);
    removing = next;
  }

  function shortUri(uri: string): string {
    try {
      const u = new URL(uri);
      return `${u.host}${u.pathname.replace(/\/objects\//, '/').slice(0, 40)}${u.pathname.length > 40 ? '…' : ''}`;
    } catch {
      return uri.slice(0, 60);
    }
  }
</script>

{#if loading}
  <p class="muted">Carregando…</p>
{:else if error}
  <Card>
    <EmptyState
      icon="bookmark"
      title="Nada aqui ainda"
      description={error}
    />
  </Card>
{:else if items.length === 0}
  <Card>
    <EmptyState
      icon="bookmark"
      title="Sem publicações salvas"
      description="Ao ver uma publicação no feed, clique nos três pontinhos e escolha 'Salvar' para guardar aqui."
    />
  </Card>
{:else}
  <ol class="rows" aria-label="Publicações salvas">
    {#each items as row (row.object_uri)}
      <li>
        <Card>
          <div class="row">
            <div class="info">
              <a class="perma" href={`/publicacao/?uri=${encodeURIComponent(row.object_uri)}`}>
                Abrir publicação
              </a>
              <span class="uri muted">{shortUri(row.object_uri)}</span>
              <time class="muted t" datetime={row.created_at} title={formatDate(row.created_at)}>
                Salvo {formatRelative(row.created_at)}
              </time>
            </div>
            <button
              type="button"
              class="rm"
              onclick={() => onRemove(row.object_uri)}
              disabled={removing.has(row.object_uri)}
              aria-label="Remover dos salvos"
              title="Remover dos salvos"
            >
              <Icon name="bookmark-fill" size={18} />
            </button>
          </div>
        </Card>
      </li>
    {/each}
  </ol>
{/if}

<style>
  .rows {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .row {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
  }
  .info {
    display: flex;
    flex-direction: column;
    gap: 2px;
    flex: 1;
    min-width: 0;
  }
  .perma {
    font-weight: var(--fw-semibold);
    color: var(--accent-strong);
    text-decoration: none;
    font-size: var(--fs-base);
  }
  .perma:hover {
    text-decoration: underline;
  }
  .uri {
    font-size: var(--fs-xs);
    font-family: ui-monospace, SFMono-Regular, monospace;
    word-break: break-all;
  }
  .t {
    font-size: var(--fs-xs);
  }
  .rm {
    background: transparent;
    border: 0;
    cursor: pointer;
    color: var(--accent-strong);
    padding: var(--sp-2);
    border-radius: var(--r-sm);
    flex-shrink: 0;
  }
  .rm:hover {
    background: var(--surface-2);
  }
</style>
