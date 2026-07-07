<script lang="ts">
  // Feed federado do cidadão autenticado: notas próprias + de quem ele segue
  // (locais e remotas). CSR puro. Reações (Favoritar/Republicar) fazem toggle
  // OTIMISTA: a UI muda na hora e reverte se a API falhar. `content_html` vem
  // de instâncias remotas → sanitizado antes de {@html}.
  //
  // 0.17.0: adotou a biblioteca ui/* (Card, Button, Avatar, Badge, Icon,
  // Skeleton, EmptyState, ErrorState) e escreve erros via toast.
  import { onMount } from 'svelte';
  import { getMyFeed, toggleLike, toggleBoost } from '../../lib/api';
  import type { FeedItemDto } from '../../lib/types';
  import { sanitizeNoteHtml } from '../../lib/sanitize';
  import { formatRelative, formatDate } from '../../lib/format';
  import { toast } from '../../lib/toasts';
  import NoteComposer from './NoteComposer.svelte';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Badge from '../ui/Badge.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';

  const PAGE = 20;

  let ready = $state(false);
  let loggedIn = $state(false);
  let loading = $state(true);
  let loadingMore = $state(false);
  let items = $state<FeedItemDto[]>([]);
  let loadError = $state<string | null>(null);
  let hasMore = $state(false);
  let offset = 0;
  // Reações em voo, chaveadas por `${kind}:${uri}` — trava o botão certo, não o card todo.
  let inFlight = $state<Set<string>>(new Set());

  function isLogged(): boolean {
    try {
      return !!localStorage.getItem('dsoc_citizen');
    } catch {
      return false;
    }
  }

  async function loadFirstPage() {
    loading = true;
    loadError = null;
    const res = await getMyFeed(PAGE, 0);
    loading = false;
    if (res.success && res.data) {
      items = res.data;
      offset = res.data.length;
      hasMore = res.data.length === PAGE;
    } else {
      loadError = res.error?.message ?? 'Não foi possível carregar o seu feed.';
    }
  }

  async function loadMore() {
    if (loadingMore || !hasMore) return;
    loadingMore = true;
    const res = await getMyFeed(PAGE, offset);
    loadingMore = false;
    if (res.success && res.data) {
      // Dedup por object_uri: o offset pode deslizar se chegou nota nova entre as páginas.
      const seen = new Set(items.map((i) => i.object_uri));
      items = [...items, ...res.data.filter((i) => !seen.has(i.object_uri))];
      offset += res.data.length;
      hasMore = res.data.length === PAGE;
    } else {
      toast.error(res.error?.message ?? 'Não foi possível carregar mais notas.');
    }
  }

  onMount(() => {
    loggedIn = isLogged();
    ready = true;
    if (loggedIn) {
      void loadFirstPage();
    } else {
      loading = false;
    }
  });

  function patch(uri: string, p: Partial<FeedItemDto>) {
    items = items.map((i) => (i.object_uri === uri ? { ...i, ...p } : i));
  }

  async function onLike(item: FeedItemDto) {
    const key = `like:${item.object_uri}`;
    if (inFlight.has(key)) return;
    inFlight = new Set(inFlight).add(key);
    const before = { liked_by_me: item.liked_by_me, like_count: item.like_count };
    patch(item.object_uri, {
      liked_by_me: !item.liked_by_me,
      like_count: Math.max(0, item.like_count + (item.liked_by_me ? -1 : 1)),
    });
    const res = await toggleLike(item.object_uri);
    if (res.success && res.data) {
      patch(item.object_uri, {
        liked_by_me: res.data.liked,
        like_count: res.data.like_count,
      });
    } else {
      patch(item.object_uri, before);
      toast.error(res.error?.message ?? 'Não foi possível favoritar agora.');
    }
    const next = new Set(inFlight);
    next.delete(key);
    inFlight = next;
  }

  async function onBoost(item: FeedItemDto) {
    const key = `boost:${item.object_uri}`;
    if (inFlight.has(key)) return;
    inFlight = new Set(inFlight).add(key);
    const before = { boosted_by_me: item.boosted_by_me, boost_count: item.boost_count };
    patch(item.object_uri, {
      boosted_by_me: !item.boosted_by_me,
      boost_count: Math.max(0, item.boost_count + (item.boosted_by_me ? -1 : 1)),
    });
    const res = await toggleBoost(item.object_uri);
    if (res.success && res.data) {
      patch(item.object_uri, {
        boosted_by_me: res.data.boosted,
        boost_count: res.data.boost_count,
      });
    } else {
      patch(item.object_uri, before);
      toast.error(res.error?.message ?? 'Não foi possível republicar agora.');
    }
    const next = new Set(inFlight);
    next.delete(key);
    inFlight = next;
  }
</script>

{#if !ready}
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else if !loggedIn}
  <Card padding="none">
    <div class="gate">
      <Icon name="feed" size={40} />
      <h2>Entre para ver seu feed</h2>
      <p>
        O feed reúne suas notas e as das pessoas que você segue — aqui e em
        qualquer instância do fediverso.
      </p>
      <div class="gate-cta">
        <Button href="/entrar" variant="primary">Entrar</Button>
        <Button href="/cadastrar" variant="ghost">Criar conta</Button>
      </div>
    </div>
  </Card>
{:else}
  <div class="composer">
    <Card>
      <NoteComposer variant="feed" onposted={loadFirstPage} />
    </Card>
  </div>

  {#if loading}
    <div class="skeletons">
      {#each [0, 1, 2] as i (i)}
        <Card>
          <div class="sk-head">
            <Skeleton variant="circle" width="44px" />
            <div style="flex:1">
              <Skeleton width="40%" />
              <Skeleton width="25%" />
            </div>
          </div>
          <Skeleton lines={2} />
        </Card>
      {/each}
    </div>
  {:else if loadError}
    <ErrorState
      title="Não deu para carregar o feed"
      message={loadError}
      retry={loadFirstPage}
    />
  {:else if items.length === 0}
    <Card padding="none">
      <EmptyState
        icon="feed"
        title="Seu feed está vazio"
        description="Publique sua primeira nota acima — ou siga alguém no fediverso para ver as publicações aqui."
        action={emptyAction}
      />
    </Card>
    {#snippet emptyAction()}
      <Button href="/configuracoes" variant="primary">Encontrar pessoas</Button>
    {/snippet}
  {:else}
    <ol class="notes" aria-label="Notas do seu feed">
      {#each items as item (item.object_uri)}
        <li>
          <Card as="article">
            <header class="note-head">
              <Avatar
                src={item.author_avatar_url}
                name={item.author_display_name ?? item.author_handle}
                alt=""
                size="base"
              />
              <div class="who">
                <span class="who-line">
                  <strong class="name">
                    {item.author_display_name ?? item.author_handle}
                  </strong>
                  {#if item.is_remote}
                    <Badge tone="info" size="sm">fediverso</Badge>
                  {/if}
                </span>
                <span class="handle muted">
                  {item.author_handle.startsWith('@')
                    ? item.author_handle
                    : `@${item.author_handle}`}
                </span>
              </div>
              <time
                class="when muted"
                datetime={item.published_at}
                title={formatDate(item.published_at)}
              >
                {formatRelative(item.published_at)}
              </time>
            </header>

            <div class="content">
              <!-- eslint-disable-next-line svelte/no-at-html-tags — sanitizado em sanitizeNoteHtml -->
              {@html sanitizeNoteHtml(item.content_html)}
            </div>

            <footer class="reactions">
              <button
                type="button"
                class="react"
                class:on={item.liked_by_me}
                class:like-on={item.liked_by_me}
                aria-pressed={item.liked_by_me}
                aria-label={item.liked_by_me
                  ? `Remover favorito (${item.like_count})`
                  : `Favoritar (${item.like_count})`}
                disabled={inFlight.has(`like:${item.object_uri}`)}
                onclick={() => onLike(item)}
              >
                <Icon
                  name={item.liked_by_me ? 'heart-fill' : 'heart'}
                  size={18}
                />
                <span class="lbl">Favoritar</span>
                <span class="cnt">{item.like_count}</span>
              </button>
              <button
                type="button"
                class="react"
                class:on={item.boosted_by_me}
                class:boost-on={item.boosted_by_me}
                aria-pressed={item.boosted_by_me}
                aria-label={item.boosted_by_me
                  ? `Desfazer republicação (${item.boost_count})`
                  : `Republicar (${item.boost_count})`}
                disabled={inFlight.has(`boost:${item.object_uri}`)}
                onclick={() => onBoost(item)}
              >
                <Icon name="boost" size={18} />
                <span class="lbl">Republicar</span>
                <span class="cnt">{item.boost_count}</span>
              </button>
              <button
                type="button"
                class="react"
                aria-label="Responder"
                disabled
                title="Em breve"
              >
                <Icon name="reply" size={18} />
                <span class="lbl">Responder</span>
              </button>
            </footer>
          </Card>
        </li>
      {/each}
    </ol>

    {#if hasMore}
      <div class="more">
        <Button
          variant="ghost"
          onclick={loadMore}
          loading={loadingMore}
        >
          Carregar mais
        </Button>
      </div>
    {/if}
  {/if}
{/if}

<style>
  .gate {
    text-align: center;
    padding: var(--sp-10) var(--sp-6);
    color: var(--text-2);
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-3);
  }
  .gate > :global(svg) {
    color: var(--text-3);
    background: var(--surface-2);
    padding: var(--sp-3);
    border-radius: 50%;
    box-sizing: content-box;
  }
  .gate h2 {
    font-size: var(--fs-xl);
    margin: 0;
    color: var(--text-1);
  }
  .gate p {
    max-width: 32rem;
    margin: 0;
    color: var(--text-3);
  }
  .gate-cta {
    display: flex;
    gap: var(--sp-2);
    justify-content: center;
    flex-wrap: wrap;
    margin-top: var(--sp-2);
  }
  .composer {
    margin-bottom: var(--sp-4);
  }
  .notes {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .note-head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-3);
  }
  .who {
    min-width: 0;
    flex: 1;
    display: grid;
    gap: 2px;
  }
  .who-line {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }
  .name {
    font-size: var(--fs-md);
    color: var(--text-1);
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .handle {
    font-size: var(--fs-sm);
    color: var(--text-3);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .when {
    flex-shrink: 0;
    font-size: var(--fs-xs);
    color: var(--text-3);
    align-self: flex-start;
    margin-top: 2px;
  }

  .content {
    line-height: var(--lh-base);
    overflow-wrap: anywhere;
    color: var(--text-1);
  }
  .content :global(p) {
    margin: 0 0 var(--sp-2);
  }
  .content :global(p:last-child) {
    margin-bottom: 0;
  }
  .content :global(a) {
    color: var(--accent);
  }
  .content :global(a:hover) {
    color: var(--accent-strong);
  }

  .reactions {
    display: flex;
    gap: var(--sp-1);
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--border-subtle);
  }
  .react {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    color: var(--text-3);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-full);
    padding: var(--sp-1) var(--sp-3);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  .react:hover:not(:disabled) {
    background: var(--surface-2);
    color: var(--text-1);
  }
  .react:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .react .cnt {
    font-variant-numeric: tabular-nums;
    font-weight: var(--fw-semibold);
  }
  .react.like-on {
    color: var(--danger);
    background: var(--danger-soft);
  }
  .react.boost-on {
    color: var(--accent);
    background: var(--accent-soft);
  }

  .more {
    display: flex;
    justify-content: center;
    margin-top: var(--sp-5);
  }

  .skeletons {
    display: grid;
    gap: var(--sp-3);
  }
  .sk-head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-3);
  }

  @media (max-width: 420px) {
    .react .lbl {
      display: none;
    }
  }
</style>
