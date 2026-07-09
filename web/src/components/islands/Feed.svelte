<script lang="ts">
  // Feed federado do cidadão autenticado: notas próprias + de quem ele segue
  // (locais e remotas). CSR puro. Reações (Favoritar/Republicar) fazem toggle
  // OTIMISTA: a UI muda na hora e reverte se a API falhar. `content_html` vem
  // de instâncias remotas → sanitizado antes de {@html}.
  //
  // 0.17.0: adotou a biblioteca ui/* (Card, Button, Avatar, Badge, Icon,
  // Skeleton, EmptyState, ErrorState) e escreve erros via toast.
  import { onMount, onDestroy } from 'svelte';
  import {
    getMyFeed,
    toggleLike,
    toggleBoost,
    bookmarkUri,
    unbookmarkUri,
    deleteNote,
    isAuthError,
    clearLocalSession,
    getFollowSuggestions,
    followRemoteActor,
    type MentionHit,
  } from '../../lib/api';
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
  import Menu from '../ui/Menu.svelte';
  import Modal from '../ui/Modal.svelte';
  import MediaGrid from '../social/MediaGrid.svelte';
  import PollView from '../social/PollView.svelte';

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
  // Bookmarks: só estado local otimista; a lista completa fica em /salvos.
  let bookmarked = $state<Set<string>>(new Set());
  // 0.18.0: CW collapse — set of object_uri revelados; e composer de reply inline.
  let revealed = $state<Set<string>>(new Set());
  let replyingTo = $state<string | null>(null);
  let editingTo = $state<string | null>(null);
  let deletingTo = $state<FeedItemDto | null>(null);
  let deleteOpen = $state(false);
  let deleteBusy = $state(false);
  // 0.19.0-polish2: real-time indicator + follow suggestions for empty state.
  let newNotesCount = $state(0);
  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let suggestions = $state<MentionHit[]>([]);
  let sentinelEl = $state<HTMLElement | null>(null);
  let observer: IntersectionObserver | null = null;
  let followBusy = $state<Set<string>>(new Set());
  let followed = $state<Set<string>>(new Set());

  function askDelete(item: FeedItemDto) {
    deletingTo = item;
    deleteOpen = true;
  }
  function cancelDelete() {
    deleteOpen = false;
    deletingTo = null;
  }
  let myHandle = $state<string | null>(null);

  function toggleReveal(uri: string) {
    revealed = new Set(revealed);
    if (revealed.has(uri)) revealed.delete(uri);
    else revealed.add(uri);
  }

  /** Poll for new notes — cheap `/me/feed?limit=1` compared against the
   *  current head item. On a change, bump `newNotesCount` so the "N novas"
   *  ribbon renders; click prepends the fresh page. */
  async function pollForNew() {
    if (!loggedIn || items.length === 0) return;
    const res = await getMyFeed(5, 0);
    if (!res.success || !res.data || res.data.length === 0) return;
    const currentTop = items[0]?.object_uri;
    const uris = new Set(items.map((i) => i.object_uri));
    const fresh = res.data.filter((n) => !uris.has(n.object_uri));
    if (fresh.length > 0 && fresh[0].object_uri !== currentTop) {
      newNotesCount = fresh.length;
    }
  }

  async function loadNewNotes() {
    newNotesCount = 0;
    const res = await getMyFeed(PAGE, 0);
    if (res.success && res.data) {
      const seen = new Set(items.map((i) => i.object_uri));
      const fresh = res.data.filter((n) => !seen.has(n.object_uri));
      if (fresh.length > 0) items = [...fresh, ...items];
    }
  }

  async function fetchSuggestions() {
    if (!loggedIn) return;
    const res = await getFollowSuggestions(6);
    if (res.success && res.data) suggestions = res.data.items;
  }

  async function follow(m: MentionHit) {
    if (followBusy.has(m.actor_url)) return;
    followBusy = new Set(followBusy).add(m.actor_url);
    const res = await followRemoteActor(m.actor_url);
    const next = new Set(followBusy);
    next.delete(m.actor_url);
    followBusy = next;
    if (res.success) {
      followed = new Set(followed).add(m.actor_url);
      toast.success(`Seguindo @${m.handle}.`);
      void loadFirstPage();
    } else {
      toast.error(res.error?.message ?? 'Não foi possível seguir.');
    }
  }

  function isMine(item: FeedItemDto): boolean {
    if (!myHandle) return false;
    const h = item.author_handle.replace(/^@/, '');
    // Local authors are `@handle` (no domain); remote include `@host.tld`.
    return h === myHandle && !item.is_remote;
  }

  async function confirmDelete() {
    if (!deletingTo || deleteBusy) return;
    deleteBusy = true;
    const res = await deleteNote(deletingTo.object_uri);
    deleteBusy = false;
    if (res.success) {
      toast.success('Publicação apagada.');
      deleteOpen = false;
      deletingTo = null;
      loadFirstPage();
    } else {
      toast.error(res.error?.message ?? 'Não foi possível apagar.');
    }
  }

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
    } else if (isAuthError(res)) {
      // Session expired — drop the stale localStorage and flip to the gate.
      clearLocalSession();
      loggedIn = false;
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
    try {
      const h = localStorage.getItem('dsoc_handle');
      myHandle = h && !h.startsWith('u-') ? h : null;
    } catch {}
    ready = true;
    if (loggedIn) {
      void loadFirstPage();
      void fetchSuggestions();
      // Poll for new notes every 45 s. Cheap query (limit=5).
      pollTimer = setInterval(pollForNew, 45_000);
      // IntersectionObserver on the bottom sentinel drives infinite scroll.
      observer = new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            if (e.isIntersecting && !loadingMore && hasMore) {
              void loadMore();
            }
          }
        },
        { rootMargin: '400px' },
      );
    } else {
      loading = false;
    }
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
    if (observer) observer.disconnect();
  });

  $effect(() => {
    // Attach the observer to the sentinel whenever it enters the DOM.
    if (!observer || !sentinelEl) return;
    observer.observe(sentinelEl);
    return () => observer?.unobserve(sentinelEl!);
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

  async function onBookmark(item: FeedItemDto) {
    const key = `bookmark:${item.object_uri}`;
    if (inFlight.has(key)) return;
    inFlight = new Set(inFlight).add(key);
    const was = bookmarked.has(item.object_uri);
    const next = new Set(bookmarked);
    if (was) next.delete(item.object_uri); else next.add(item.object_uri);
    bookmarked = next;
    const res = was
      ? await unbookmarkUri(item.object_uri)
      : await bookmarkUri(item.object_uri);
    if (!res.success) {
      const revert = new Set(bookmarked);
      if (was) revert.add(item.object_uri); else revert.delete(item.object_uri);
      bookmarked = revert;
      toast.error(res.error?.message ?? 'Não foi possível salvar agora.');
    } else {
      toast.success(was ? 'Removido dos salvos.' : 'Salvo.');
    }
    const done = new Set(inFlight);
    done.delete(key);
    inFlight = done;
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

  {#if newNotesCount > 0}
    <button type="button" class="new-notes-ribbon" onclick={loadNewNotes}>
      <Icon name="arrow-right" size={16} />
      {newNotesCount} {newNotesCount === 1 ? 'nova publicação' : 'novas publicações'} — ver
    </button>
  {/if}

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
        description="Publique sua primeira nota acima — ou siga alguém para ver as publicações aqui."
        action={emptyAction}
      />
    </Card>
    {#snippet emptyAction()}
      <Button href="/explorar" variant="primary">Explorar</Button>
    {/snippet}
    {#if suggestions.length > 0}
      <section class="suggestions">
        <h3>Comece seguindo alguém</h3>
        <ul class="sug-list">
          {#each suggestions as m (m.handle)}
            <li>
              <Card>
                <div class="sug-row">
                  <a href={`/perfil/?u=${encodeURIComponent(m.handle)}`} class="sug-who">
                    <Avatar
                      src={m.avatar_url}
                      name={m.display_name ?? m.handle}
                      size="base"
                    />
                    <div class="sug-body">
                      <strong>{m.display_name ?? m.handle}</strong>
                      <span class="muted">@{m.handle}</span>
                    </div>
                  </a>
                  <Button
                    variant={followed.has(m.actor_url) ? 'ghost' : 'primary'}
                    size="sm"
                    disabled={followed.has(m.actor_url) || followBusy.has(m.actor_url)}
                    loading={followBusy.has(m.actor_url)}
                    onclick={() => follow(m)}
                  >
                    {followed.has(m.actor_url) ? 'Seguindo' : 'Seguir'}
                  </Button>
                </div>
              </Card>
            </li>
          {/each}
        </ul>
      </section>
    {/if}
  {:else}
    <ol class="notes" aria-label="Notas do seu feed">
      {#each items as item (item.object_uri)}
        <li>
          <Card as="article">
            <header class="note-head">
              <a
                class="author-link"
                href={`/perfil/?u=${encodeURIComponent(item.author_handle)}`}
                aria-label={`Ver perfil de ${item.author_display_name ?? item.author_handle}`}
              >
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
              </a>
              <time
                class="when muted"
                datetime={item.published_at}
                title={formatDate(item.published_at)}
              >
                {formatRelative(item.published_at)}
                {#if item.edited_at}
                  <span class="edited" title={`Editado em ${formatDate(item.edited_at)}`}>
                    · editada
                  </span>
                {/if}
              </time>
              {#if isMine(item)}
                <Menu align="right" label="Ações da publicação">
                  {#snippet trigger({ toggle })}
                    <button
                      type="button"
                      class="more"
                      onclick={toggle}
                      aria-label="Mais ações"
                    >
                      <Icon name="more" size={16} />
                    </button>
                  {/snippet}
                  {#snippet items()}
                    <button
                      type="button"
                      onclick={() => {
                        editingTo = item.object_uri;
                      }}
                    >
                      <Icon name="edit" size={14} />
                      Editar
                    </button>
                    <button type="button" onclick={() => askDelete(item)}>
                      <Icon name="trash" size={14} />
                      Apagar
                    </button>
                  {/snippet}
                </Menu>
              {/if}
            </header>

            {#if item.in_reply_to_uri}
              <p class="reply-line muted">
                <Icon name="reply" size={12} />
                em resposta a
                <a href={item.in_reply_to_uri}>{item.in_reply_to_uri.replace(/^https?:\/\//, '').split('/').slice(0, 2).join('/')}…</a>
              </p>
            {/if}

            {#if editingTo === item.object_uri}
              <div class="edit-composer">
                <NoteComposer
                  variant="edit"
                  edit={{
                    uri: item.object_uri,
                    content: item.content_html.replace(/<[^>]+>/g, '').trim(),
                    spoiler_text: item.spoiler_text,
                    sensitive: item.sensitive,
                  }}
                  autofocus
                  oncancel={() => (editingTo = null)}
                  onposted={() => {
                    editingTo = null;
                    loadFirstPage();
                  }}
                />
              </div>
            {:else if item.spoiler_text}
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
                <div class="content">
                  <!-- eslint-disable-next-line svelte/no-at-html-tags — sanitizado em sanitizeNoteHtml -->
                  {@html sanitizeNoteHtml(item.content_html)}
                </div>
              {/if}
            {:else}
              <div class="content">
                <!-- eslint-disable-next-line svelte/no-at-html-tags — sanitizado em sanitizeNoteHtml -->
                {@html sanitizeNoteHtml(item.content_html)}
              </div>
            {/if}

            {#if editingTo !== item.object_uri && item.attachments && item.attachments.length > 0}
              <MediaGrid media={item.attachments} />
            {/if}

            {#if editingTo !== item.object_uri && item.poll}
              <PollView
                noteUri={item.object_uri}
                poll={item.poll}
                {loggedIn}
                onvoted={(v) => (item.poll = v)}
              />
            {/if}

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
                onclick={() =>
                  (replyingTo =
                    replyingTo === item.object_uri ? null : item.object_uri)}
              >
                <Icon name="reply" size={18} />
                <span class="lbl">Responder</span>
              </button>
              <button
                type="button"
                class="react"
                class:on={bookmarked.has(item.object_uri)}
                aria-pressed={bookmarked.has(item.object_uri)}
                aria-label={bookmarked.has(item.object_uri) ? 'Remover dos salvos' : 'Salvar'}
                disabled={inFlight.has(`bookmark:${item.object_uri}`)}
                onclick={() => onBookmark(item)}
              >
                <Icon
                  name={bookmarked.has(item.object_uri) ? 'bookmark-fill' : 'bookmark'}
                  size={18}
                />
                <span class="lbl">Salvar</span>
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
                    loadFirstPage();
                  }}
                />
              </div>
            {/if}
          </Card>
        </li>
      {/each}
    </ol>

    {#if hasMore}
      <!-- Sentinel drives the IntersectionObserver-based infinite scroll. -->
      <div bind:this={sentinelEl} class="sentinel" aria-hidden="true"></div>
      <div class="more-wrap">
        <Button
          variant="ghost"
          onclick={loadMore}
          loading={loadingMore}
        >
          {loadingMore ? 'Carregando…' : 'Carregar mais'}
        </Button>
      </div>
    {/if}
  {/if}

  <Modal
    bind:open={deleteOpen}
    title="Apagar publicação?"
    onclose={cancelDelete}
  >
    <p>
      Isso apaga a publicação da DemocraciaBR e envia um pedido para as
      instâncias que a receberam também a removerem. Ação irreversível.
    </p>
    {#snippet footer()}
      <Button variant="ghost" onclick={cancelDelete}>Cancelar</Button>
      <Button variant="danger" onclick={confirmDelete} loading={deleteBusy}>
        Apagar
      </Button>
    {/snippet}
  </Modal>
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
  .author-link {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    text-decoration: none;
    color: inherit;
    min-width: 0;
    flex: 1;
    border-radius: var(--r-sm);
  }
  .author-link:hover .name {
    color: var(--accent-strong);
    text-decoration: underline;
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

  .edit-composer {
    margin-bottom: var(--sp-3);
  }
  .new-notes-ribbon {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--sp-2);
    width: 100%;
    background: var(--accent);
    color: var(--accent-contrast);
    border: 0;
    border-radius: var(--r-base);
    padding: var(--sp-3);
    margin-bottom: var(--sp-3);
    font: inherit;
    font-weight: var(--fw-semibold);
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .new-notes-ribbon:hover {
    background: var(--accent-strong);
  }
  .sentinel {
    height: 1px;
    width: 100%;
  }
  .suggestions {
    margin-top: var(--sp-4);
  }
  .suggestions h3 {
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
    color: var(--text-1);
  }
  .sug-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .sug-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
  }
  .sug-who {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    text-decoration: none;
    color: var(--text-1);
    flex: 1;
    min-width: 0;
  }
  .sug-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .edited {
    color: var(--text-3);
    font-size: 0.85em;
    margin-left: 3px;
  }
  .more {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 0;
    color: var(--text-3);
    cursor: pointer;
    padding: 4px;
    border-radius: var(--r-sm);
    margin-left: var(--sp-1);
    align-self: flex-start;
  }
  .more:hover {
    background: var(--surface-2);
    color: var(--text-1);
  }
  .reply-line {
    display: flex;
    align-items: center;
    gap: var(--sp-1);
    font-size: var(--fs-xs);
    color: var(--text-3);
    margin: 0 0 var(--sp-2);
  }
  .reply-line a {
    color: var(--text-3);
    text-decoration: none;
    border-bottom: 1px dotted var(--border-strong);
  }
  .reply-line a:hover {
    color: var(--text-1);
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
  .reply-composer {
    margin-top: var(--sp-3);
    padding-top: var(--sp-3);
    border-top: 1px solid var(--border-subtle);
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
