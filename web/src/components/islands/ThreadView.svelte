<script lang="ts">
  // Single-status thread view: renders the root note prominently and every
  // descendant reply underneath, ordered chronologically. Reads `uri` from
  // window.location (query string, since Astro SSG can't pre-generate every
  // possible note URI). Consumes GET /api/v1/notes/context.
  //
  // Interactions kept minimal: like/boost/reply (opens NoteComposer inline).
  // Full-featured reactions match the Feed island — this is intentionally
  // narrower to keep the file readable; if needed we later factor out a
  // shared NoteCard.
  import { onMount } from 'svelte';
  import {
    getThreadContext,
    toggleLike,
    toggleBoost,
  } from '../../lib/api';
  import type { FeedItemDto } from '../../lib/types';
  import { sanitizeNoteHtml } from '../../lib/sanitize';
  import { formatRelative, formatDate } from '../../lib/format';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Badge from '../ui/Badge.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';
  import NoteComposer from './NoteComposer.svelte';
  import MediaGrid from '../social/MediaGrid.svelte';

  let ready = $state(false);
  let uri = $state<string | null>(null);
  let loading = $state(true);
  let items = $state<FeedItemDto[]>([]);
  let loadError = $state<string | null>(null);
  let revealed = $state<Set<string>>(new Set());
  let replyingTo = $state<string | null>(null);
  let inFlight = $state<Set<string>>(new Set());
  let loggedIn = $state(false);

  function toggleReveal(u: string) {
    revealed = new Set(revealed);
    if (revealed.has(u)) revealed.delete(u);
    else revealed.add(u);
  }

  async function load() {
    if (!uri) return;
    loading = true;
    loadError = null;
    const res = await getThreadContext(uri);
    loading = false;
    if (res.success && res.data) items = res.data;
    else loadError = res.error?.message ?? 'Não foi possível carregar a publicação.';
  }

  onMount(() => {
    try {
      loggedIn = !!localStorage.getItem('dsoc_citizen');
    } catch {}
    const params = new URLSearchParams(window.location.search);
    uri = params.get('uri');
    ready = true;
    if (uri) void load();
    else loading = false;
  });

  function patch(u: string, p: Partial<FeedItemDto>) {
    items = items.map((i) => (i.object_uri === u ? { ...i, ...p } : i));
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

  // Descendants depth: how many hops from the root (uri). Used to indent replies.
  const depths = $derived.by(() => {
    const d = new Map<string, number>();
    if (!uri) return d;
    d.set(uri, 0);
    // BFS: since the API returns items sorted by published_at ASC, and every
    // in_reply_to_uri points at an earlier item, one pass suffices.
    for (const it of items) {
      if (d.has(it.object_uri)) continue;
      const parent = it.in_reply_to_uri;
      const parentDepth = parent && d.has(parent) ? d.get(parent)! : 0;
      d.set(it.object_uri, parentDepth + 1);
    }
    return d;
  });
</script>

{#if !ready}
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else if !uri}
  <Card padding="none">
    <EmptyState
      icon="chat"
      title="Nenhuma publicação selecionada"
      description="Volte pelo feed ou por um link de publicação."
    />
  </Card>
{:else if loading}
  <div class="skeletons">
    {#each [0, 1] as i (i)}
      <Card>
        <Skeleton lines={3} />
      </Card>
    {/each}
  </div>
{:else if loadError}
  <ErrorState message={loadError} retry={load} />
{:else if items.length === 0}
  <Card padding="none">
    <EmptyState
      icon="chat"
      title="Publicação não encontrada"
      description="Ela pode ter sido apagada ou não passou pelo seu feed."
    />
  </Card>
{:else}
  <ol class="thread" aria-label="Fio da publicação">
    {#each items as item, i (item.object_uri)}
      {@const depth = depths.get(item.object_uri) ?? 0}
      <li class="row" style={`--d:${Math.min(depth, 6)}`}>
        <Card as="article" padding={i === 0 ? 'lg' : 'base'}>
          <header class="head">
            <Avatar
              src={item.author_avatar_url}
              name={item.author_display_name ?? item.author_handle}
              alt=""
              size={i === 0 ? 'lg' : 'base'}
            />
            <div class="who">
              <span class="who-line">
                <strong>
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

          {#if item.spoiler_text}
            <div class="cw">
              <div class="cw-head">
                <Icon name="cw" size={14} />
                <span class="cw-text">{item.spoiler_text}</span>
              </div>
              <button
                type="button"
                class="cw-toggle"
                aria-expanded={revealed.has(item.object_uri)}
                onclick={() => toggleReveal(item.object_uri)}
              >
                <Icon
                  name={revealed.has(item.object_uri) ? 'eye-off' : 'eye'}
                  size={14}
                />
                {revealed.has(item.object_uri) ? 'Ocultar' : 'Mostrar conteúdo'}
              </button>
            </div>
            {#if revealed.has(item.object_uri)}
              <div class="content" class:root={i === 0}>
                <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                {@html sanitizeNoteHtml(item.content_html)}
              </div>
            {/if}
          {:else}
            <div class="content" class:root={i === 0}>
              <!-- eslint-disable-next-line svelte/no-at-html-tags -->
              {@html sanitizeNoteHtml(item.content_html)}
            </div>
          {/if}

          {#if item.attachments && item.attachments.length > 0}
            <MediaGrid media={item.attachments} />
          {/if}

          <footer class="reactions">
            <button
              type="button"
              class="react"
              class:like-on={item.liked_by_me}
              aria-pressed={item.liked_by_me}
              disabled={inFlight.has(`like:${item.object_uri}`) || !loggedIn}
              onclick={() => onLike(item)}
            >
              <Icon
                name={item.liked_by_me ? 'heart-fill' : 'heart'}
                size={16}
              />
              <span class="cnt">{item.like_count}</span>
            </button>
            <button
              type="button"
              class="react"
              class:boost-on={item.boosted_by_me}
              aria-pressed={item.boosted_by_me}
              disabled={inFlight.has(`boost:${item.object_uri}`) || !loggedIn}
              onclick={() => onBoost(item)}
            >
              <Icon name="boost" size={16} />
              <span class="cnt">{item.boost_count}</span>
            </button>
            <button
              type="button"
              class="react"
              disabled={!loggedIn}
              onclick={() =>
                (replyingTo =
                  replyingTo === item.object_uri ? null : item.object_uri)}
              aria-label="Responder"
            >
              <Icon name="reply" size={16} />
              <span>Responder</span>
            </button>
          </footer>

          {#if replyingTo === item.object_uri}
            <div class="reply-composer">
              <NoteComposer
                variant="reply"
                replyTo={{
                  uri: item.object_uri,
                  handle: item.author_handle.replace(/^@/, ''),
                }}
                autofocus
                oncancel={() => (replyingTo = null)}
                onposted={() => {
                  replyingTo = null;
                  load();
                }}
              />
            </div>
          {/if}
        </Card>
      </li>
    {/each}
  </ol>
{/if}

<style>
  .thread {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .row {
    padding-left: calc(var(--d, 0) * var(--sp-3));
    position: relative;
  }
  .row[style*='--d:1']::before,
  .row[style*='--d:2']::before,
  .row[style*='--d:3']::before,
  .row[style*='--d:4']::before,
  .row[style*='--d:5']::before,
  .row[style*='--d:6']::before {
    content: '';
    position: absolute;
    left: calc(var(--d, 0) * var(--sp-3) - var(--sp-2));
    top: 0;
    bottom: 0;
    width: 2px;
    background: var(--border-subtle);
    border-radius: 2px;
  }

  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-3);
  }
  .who {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .who-line {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    min-width: 0;
  }
  .who strong {
    color: var(--text-1);
    font-weight: var(--fw-semibold);
    font-size: var(--fs-md);
  }
  .handle {
    font-size: var(--fs-sm);
  }
  .when {
    font-size: var(--fs-xs);
    align-self: flex-start;
  }

  .content {
    line-height: var(--lh-base);
    overflow-wrap: anywhere;
    color: var(--text-1);
  }
  .content.root {
    font-size: var(--fs-lg);
    line-height: var(--lh-snug);
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

  .cw {
    background: var(--warning-soft);
    border: 1px solid color-mix(in srgb, var(--warning) 20%, transparent);
    border-radius: var(--r-sm);
    padding: var(--sp-2) var(--sp-3);
    margin-bottom: var(--sp-2);
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .cw-head {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-2);
    color: var(--warning);
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    min-width: 0;
  }
  .cw-text {
    color: var(--text-1);
    font-weight: var(--fw-medium);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cw-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    background: var(--surface-1);
    color: var(--text-2);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-full);
    padding: var(--sp-1) var(--sp-3);
    font: inherit;
    font-size: var(--fs-xs);
    font-weight: var(--fw-semibold);
    cursor: pointer;
    flex-shrink: 0;
  }
  .cw-toggle:hover {
    background: var(--surface-2);
    color: var(--text-1);
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
    gap: var(--sp-1);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    color: var(--text-3);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--r-full);
    padding: var(--sp-1) var(--sp-3);
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out), color var(--dur-fast) var(--ease-out);
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

  .reply-composer {
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--border-subtle);
  }
  .skeletons {
    display: grid;
    gap: var(--sp-3);
  }
  .muted {
    color: var(--text-3);
  }
</style>
