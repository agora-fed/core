<script lang="ts">
  // Hashtag timeline: public feed of notes tagged with #<name>. Uses the
  // /api/v1/timelines/tag/{name} endpoint. Query string carries the tag
  // (?nome=<name>) so the SSG page ships one template.
  //
  // Interactions match ThreadView: like/boost/reply. Anonymous users see the
  // list but the action buttons are disabled.
  import { onMount } from 'svelte';
  import {
    getHashtagTimeline,
    toggleLike,
    toggleBoost,
  } from '../../lib/api';
  import type { FeedItemDto } from '../../lib/types';
  import { sanitizeNoteHtml } from '../../lib/sanitize';
  import { formatRelative, formatDate } from '../../lib/format';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Badge from '../ui/Badge.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import ErrorState from '../ui/ErrorState.svelte';

  let ready = $state(false);
  let name = $state<string | null>(null);
  let normalizedTag = $state<string | null>(null);
  let loading = $state(true);
  let items = $state<FeedItemDto[]>([]);
  let loadError = $state<string | null>(null);
  let revealed = $state<Set<string>>(new Set());
  let inFlight = $state<Set<string>>(new Set());
  let loggedIn = $state(false);

  function toggleReveal(u: string) {
    revealed = new Set(revealed);
    if (revealed.has(u)) revealed.delete(u);
    else revealed.add(u);
  }

  async function load() {
    if (!name) return;
    loading = true;
    loadError = null;
    const res = await getHashtagTimeline(name, 30, 0);
    loading = false;
    if (res.success && res.data) {
      items = res.data.items;
      normalizedTag = res.data.tag;
    } else {
      loadError =
        res.error?.message ?? 'Não foi possível carregar a timeline da tag.';
    }
  }

  onMount(() => {
    try {
      loggedIn = !!localStorage.getItem('dsoc_citizen');
    } catch {}
    const params = new URLSearchParams(window.location.search);
    name = params.get('nome')?.trim() || null;
    ready = true;
    if (name) void load();
    else loading = false;
  });

  function patch(u: string, p: Partial<FeedItemDto>) {
    items = items.map((i) => (i.object_uri === u ? { ...i, ...p } : i));
  }

  async function onLike(item: FeedItemDto) {
    if (!loggedIn) return;
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
    if (!loggedIn) return;
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
{:else if !name}
  <Card padding="none">
    <EmptyState
      icon="hashtag"
      title="Sem tag selecionada"
      description="Use /tag?nome=<sua_tag> ou clique numa hashtag no feed."
    />
  </Card>
{:else}
  <header class="head">
    <span class="ic"><Icon name="hashtag" size={24} /></span>
    <h1>#{normalizedTag ?? name}</h1>
    {#if !loading && !loadError}
      <Badge tone="neutral" size="sm">
        {items.length}{items.length === 30 ? '+' : ''} publicaç{items.length === 1 ? 'ão' : 'ões'}
      </Badge>
    {/if}
  </header>

  {#if loading}
    <div class="skeletons">
      {#each [0, 1, 2] as i (i)}
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
        icon="hashtag"
        title="Nada aqui ainda"
        description="Seja a primeira pessoa a publicar com essa tag no fediverso."
      />
    </Card>
  {:else}
    <ol class="list">
      {#each items as item (item.object_uri)}
        <li>
          <Card as="article">
            <header class="row">
              <Avatar
                src={item.author_avatar_url}
                name={item.author_display_name ?? item.author_handle}
                alt=""
                size="base"
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
                  onclick={() => toggleReveal(item.object_uri)}
                >
                  <Icon
                    name={revealed.has(item.object_uri) ? 'eye-off' : 'eye'}
                    size={14}
                  />
                  {revealed.has(item.object_uri) ? 'Ocultar' : 'Mostrar'}
                </button>
              </div>
              {#if revealed.has(item.object_uri)}
                <div class="content">
                  <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                  {@html sanitizeNoteHtml(item.content_html)}
                </div>
              {/if}
            {:else}
              <div class="content">
                <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                {@html sanitizeNoteHtml(item.content_html)}
              </div>
            {/if}

            <footer class="reactions">
              <button
                type="button"
                class="react"
                class:like-on={item.liked_by_me}
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
                disabled={inFlight.has(`boost:${item.object_uri}`) || !loggedIn}
                onclick={() => onBoost(item)}
              >
                <Icon name="boost" size={16} />
                <span class="cnt">{item.boost_count}</span>
              </button>
              <a
                class="react"
                href={`/publicacao?uri=${encodeURIComponent(item.object_uri)}`}
                aria-label="Abrir a publicação"
              >
                <Icon name="chat" size={16} />
                <span>Abrir</span>
              </a>
            </footer>
          </Card>
        </li>
      {/each}
    </ol>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-5);
  }
  .head h1 {
    margin: 0;
    font-size: var(--fs-3xl);
    color: var(--text-1);
    line-height: 1;
  }
  .ic {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 48px;
    height: 48px;
    background: var(--accent-soft);
    color: var(--accent);
    border-radius: var(--r-base);
  }

  .list {
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
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .cw-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--sp-1);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    color: var(--text-2);
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
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-3);
    border-radius: var(--r-full);
    padding: var(--sp-1) var(--sp-3);
    font: inherit;
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    text-decoration: none;
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

  .skeletons {
    display: grid;
    gap: var(--sp-3);
  }
  .muted {
    color: var(--text-3);
  }
</style>
