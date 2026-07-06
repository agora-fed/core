<script lang="ts">
  // Feed federado do cidadão autenticado: notas próprias + de quem ele segue (locais e remotas).
  // CSR puro (o site é SSG): sessão detectada pelo mesmo marcador que o AuthMenu usa
  // (localStorage `dsoc_citizen`; o cookie HttpOnly é a credencial real e vai junto no fetch).
  // Reações (Favoritar/Republicar) fazem toggle OTIMISTA: a UI muda na hora e reverte se a
  // API falhar. `content_html` vem de instâncias remotas → sanitizado antes de {@html}.
  import { onMount } from 'svelte';
  import { getMyFeed, toggleLike, toggleBoost } from '../../lib/api';
  import type { FeedItemDto } from '../../lib/types';
  import { sanitizeNoteHtml } from '../../lib/sanitize';
  import { formatRelative, formatDate } from '../../lib/format';
  import NoteComposer from './NoteComposer.svelte';

  const PAGE = 20;

  let ready = $state(false);
  let loggedIn = $state(false);
  let loading = $state(true);
  let loadingMore = $state(false);
  let items = $state<FeedItemDto[]>([]);
  let loadError = $state<string | null>(null);
  let actionError = $state<string | null>(null);
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
    actionError = null;
    const res = await getMyFeed(PAGE, offset);
    loadingMore = false;
    if (res.success && res.data) {
      // Dedup por object_uri: o offset pode deslizar se chegou nota nova entre as páginas.
      const seen = new Set(items.map((i) => i.object_uri));
      items = [...items, ...res.data.filter((i) => !seen.has(i.object_uri))];
      offset += res.data.length;
      hasMore = res.data.length === PAGE;
    } else {
      actionError = res.error?.message ?? 'Não foi possível carregar mais notas.';
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
    actionError = null;
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
      actionError = res.error?.message ?? 'Não foi possível favoritar agora.';
    }
    const next = new Set(inFlight);
    next.delete(key);
    inFlight = next;
  }

  async function onBoost(item: FeedItemDto) {
    const key = `boost:${item.object_uri}`;
    if (inFlight.has(key)) return;
    inFlight = new Set(inFlight).add(key);
    actionError = null;
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
      actionError = res.error?.message ?? 'Não foi possível republicar agora.';
    }
    const next = new Set(inFlight);
    next.delete(key);
    inFlight = next;
  }

  function initials(item: FeedItemDto): string {
    const src = item.author_display_name ?? item.author_handle;
    return (src.replace(/^@/, '').charAt(0) || '?').toUpperCase();
  }
</script>

{#if !ready}
  <!-- SSG manda HTML estático: nada até ler o storage, evitando flash de "entre" pra logados. -->
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else if !loggedIn}
  <div class="card gate">
    <h2>Entre para ver seu feed</h2>
    <p class="muted">
      O feed reúne suas notas e as das pessoas que você segue — aqui e em
      qualquer instância do fediverso.
    </p>
    <div class="gate-cta">
      <a class="btn btn-primary" href="/entrar">Entrar</a>
      <a class="btn btn-ghost" href="/cadastrar">Criar conta</a>
    </div>
  </div>
{:else}
  <div class="card composer-card">
    <NoteComposer variant="feed" onposted={loadFirstPage} />
  </div>

  {#if actionError}
    <p class="hint hint-error" role="alert">{actionError}</p>
  {/if}

  {#if loading}
    <div class="skeletons" aria-label="Carregando o feed…">
      {#each [0, 1, 2] as i (i)}
        <div class="card note sk">
          <div class="sk-head">
            <span class="sk-avatar"></span>
            <span class="sk-line w40"></span>
          </div>
          <span class="sk-line w90"></span>
          <span class="sk-line w70"></span>
        </div>
      {/each}
    </div>
  {:else if loadError}
    <div class="card state" role="alert">
      <h2>Não deu para carregar o feed</h2>
      <p class="muted">{loadError}</p>
      <button class="btn btn-ghost" type="button" onclick={loadFirstPage}>
        Tentar de novo
      </button>
    </div>
  {:else if items.length === 0}
    <div class="card state">
      <h2>Seu feed está vazio</h2>
      <p class="muted">
        Publique sua primeira nota acima — ou siga alguém no fediverso para
        ver as publicações aqui.
      </p>
      <a class="btn btn-primary" href="/configuracoes">Encontrar pessoas</a>
    </div>
  {:else}
    <ol class="notes" aria-label="Notas do seu feed">
      {#each items as item (item.object_uri)}
        <li>
          <article class="card note">
            <header class="note-head">
              {#if item.author_avatar_url}
                <img class="avatar" src={item.author_avatar_url} alt="" loading="lazy" />
              {:else}
                <span class="avatar avatar-fallback" aria-hidden="true">{initials(item)}</span>
              {/if}
              <div class="who">
                <span class="who-line">
                  <strong class="name">
                    {item.author_display_name ?? item.author_handle}
                  </strong>
                  {#if item.is_remote}
                    <span class="badge-remote" title="Publicado em outra instância do fediverso">
                      fediverso
                    </span>
                  {/if}
                </span>
                <span class="handle muted">
                  {item.author_handle.startsWith('@') ? item.author_handle : `@${item.author_handle}`}
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
                class={`react ${item.liked_by_me ? 'on like-on' : ''}`}
                aria-pressed={item.liked_by_me}
                aria-label={item.liked_by_me
                  ? `Remover favorito (${item.like_count})`
                  : `Favoritar (${item.like_count})`}
                disabled={inFlight.has(`like:${item.object_uri}`)}
                onclick={() => onLike(item)}
              >
                <span class="ic" aria-hidden="true">{item.liked_by_me ? '★' : '☆'}</span>
                <span class="lbl">Favoritar</span>
                <span class="cnt">{item.like_count}</span>
              </button>
              <button
                type="button"
                class={`react ${item.boosted_by_me ? 'on boost-on' : ''}`}
                aria-pressed={item.boosted_by_me}
                aria-label={item.boosted_by_me
                  ? `Desfazer republicação (${item.boost_count})`
                  : `Republicar (${item.boost_count})`}
                disabled={inFlight.has(`boost:${item.object_uri}`)}
                onclick={() => onBoost(item)}
              >
                <span class="ic" aria-hidden="true">⇄</span>
                <span class="lbl">Republicar</span>
                <span class="cnt">{item.boost_count}</span>
              </button>
            </footer>
          </article>
        </li>
      {/each}
    </ol>

    {#if hasMore}
      <div class="more">
        <button
          class="btn btn-ghost"
          type="button"
          onclick={loadMore}
          disabled={loadingMore}
        >
          {loadingMore ? 'Carregando…' : 'Carregar mais'}
        </button>
      </div>
    {/if}
  {/if}
{/if}

<style>
  .gate,
  .state {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  .gate h2,
  .state h2 {
    font-size: 1.25rem;
    margin-bottom: 0.4rem;
  }
  .gate-cta {
    display: flex;
    gap: 0.6rem;
    justify-content: center;
    flex-wrap: wrap;
    margin-top: 1rem;
  }
  .state .btn {
    margin-top: 0.5rem;
  }

  .composer-card {
    margin-bottom: 1.25rem;
  }

  .notes {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 0.85rem;
  }
  .note {
    padding: 1rem 1.15rem;
  }

  .note-head {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    margin-bottom: 0.65rem;
  }
  .avatar {
    width: 44px;
    height: 44px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    flex-shrink: 0;
  }
  .avatar-fallback {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-weight: 700;
    font-size: 1.05rem;
    color: var(--c-green-dark);
    background: var(--c-green-soft);
  }
  .who {
    min-width: 0;
    flex: 1;
    display: grid;
    gap: 0.05rem;
  }
  .who-line {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    min-width: 0;
  }
  .name {
    font-size: 0.98rem;
    line-height: 1.25;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge-remote {
    flex-shrink: 0;
    font-size: 0.68rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--c-acted);
    background: #eff4ff;
    border: 1px solid #cdd9f7;
    border-radius: 999px;
    padding: 0.05rem 0.5rem;
  }
  .handle {
    font-size: 0.82rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .when {
    flex-shrink: 0;
    font-size: 0.8rem;
    align-self: flex-start;
    margin-top: 0.15rem;
  }

  .content {
    line-height: 1.55;
    overflow-wrap: anywhere;
  }
  .content :global(p) {
    margin: 0 0 0.6rem;
  }
  .content :global(p:last-child) {
    margin-bottom: 0;
  }
  .content :global(a) {
    color: var(--c-green-dark);
  }

  .reactions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.8rem;
    padding-top: 0.7rem;
    border-top: 1px solid var(--c-border);
  }
  .react {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    font: inherit;
    font-size: 0.88rem;
    font-weight: 500;
    color: var(--c-text-muted);
    background: transparent;
    border: 1px solid transparent;
    border-radius: 999px;
    padding: 0.35rem 0.75rem;
    cursor: pointer;
    transition: background 120ms ease, color 120ms ease;
  }
  .react:hover {
    background: var(--c-bg);
    color: var(--c-navy);
  }
  .react:disabled {
    opacity: 0.55;
    cursor: wait;
  }
  .react .ic {
    font-size: 1.05rem;
    line-height: 1;
  }
  .react .cnt {
    font-variant-numeric: tabular-nums;
    font-weight: 600;
  }
  .react.like-on {
    color: var(--c-pending);
    background: #fff7ed;
    border-color: #fde5c8;
  }
  .react.boost-on {
    color: var(--c-green-dark);
    background: var(--c-green-soft);
    border-color: #c8e5d3;
  }

  .more {
    display: flex;
    justify-content: center;
    margin-top: 1.25rem;
  }

  /* Skeleton loading — leve, some com prefers-reduced-motion (transição global já zera). */
  .skeletons {
    display: grid;
    gap: 0.85rem;
  }
  .sk-head {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    margin-bottom: 0.8rem;
  }
  .sk-avatar {
    width: 44px;
    height: 44px;
    border-radius: 50%;
    background: var(--c-bg);
    flex-shrink: 0;
  }
  .sk-line {
    display: block;
    height: 0.8rem;
    border-radius: 6px;
    background: var(--c-bg);
    margin-bottom: 0.45rem;
    animation: pulse 1.4s ease-in-out infinite;
  }
  .w40 {
    width: 40%;
    margin-bottom: 0;
  }
  .w90 {
    width: 90%;
  }
  .w70 {
    width: 70%;
  }
  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.55;
    }
  }

  @media (max-width: 420px) {
    .note {
      padding: 0.85rem 0.9rem;
    }
    .react .lbl {
      display: none; /* só ícone + contador em telas mínimas; aria-label mantém o nome */
    }
    .react {
      padding: 0.35rem 0.65rem;
    }
  }
</style>
