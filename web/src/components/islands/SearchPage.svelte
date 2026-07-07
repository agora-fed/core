<script lang="ts">
  // Unified search page — mirrors Mastodon's /search behaviour. Reads ?q=
  // client-side (SSG can't pre-generate every query), fires /api/v1/search,
  // renders three collapsible sections: accounts, hashtags, publicações.
  import { onMount } from 'svelte';
  import {
    searchAll,
    type MentionHit,
    type HashtagHit,
    type NoteHit,
  } from '../../lib/api';
  import { sanitizeNoteHtml } from '../../lib/sanitize';
  import { formatRelative } from '../../lib/format';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';
  import Input from '../ui/Input.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';
  import Tabs from '../ui/Tabs.svelte';

  let ready = $state(false);
  let query = $state('');
  let loading = $state(false);
  let accounts = $state<MentionHit[]>([]);
  let hashtags = $state<HashtagHit[]>([]);
  let notes = $state<NoteHit[]>([]);
  let active = $state('all');

  async function run(q: string) {
    if (!q.trim()) {
      accounts = [];
      hashtags = [];
      notes = [];
      return;
    }
    loading = true;
    const res = await searchAll(q.trim(), 20);
    loading = false;
    if (res.success && res.data) {
      accounts = res.data.accounts;
      hashtags = res.data.hashtags;
      notes = res.data.notes;
    }
  }

  onMount(() => {
    const params = new URLSearchParams(window.location.search);
    query = params.get('q') ?? '';
    ready = true;
    if (query) void run(query);
  });

  function submit(e: SubmitEvent) {
    e.preventDefault();
    const url = new URL(window.location.href);
    if (query.trim()) url.searchParams.set('q', query.trim());
    else url.searchParams.delete('q');
    history.replaceState({}, '', url);
    void run(query);
  }

  const tabs = $derived([
    { id: 'all', label: 'Tudo' },
    { id: 'accounts', label: 'Contas', count: accounts.length },
    { id: 'hashtags', label: 'Hashtags', count: hashtags.length },
    { id: 'notes', label: 'Publicações', count: notes.length },
  ]);
  const empty = $derived(
    !loading &&
      query.trim().length > 0 &&
      accounts.length === 0 &&
      hashtags.length === 0 &&
      notes.length === 0,
  );
</script>

{#if !ready}
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else}
  <header class="head">
    <div class="ic"><Icon name="search" size={24} /></div>
    <h1>Buscar</h1>
  </header>

  <form onsubmit={submit} class="search-form">
    <Input
      type="search"
      placeholder="Buscar contas, hashtags ou publicações…"
      bind:value={query}
      leading={leadingIcon}
    />
    {#snippet leadingIcon()}
      <Icon name="search" size={16} />
    {/snippet}
    <Button type="submit" variant="primary">Buscar</Button>
  </form>

  {#if query.trim().length > 0}
    <Tabs {tabs} bind:active />
  {/if}

  {#if loading}
    <div class="loading">
      {#each [0, 1, 2] as i (i)}
        <Card><Skeleton lines={2} /></Card>
      {/each}
    </div>
  {:else if empty}
    <Card padding="none">
      <EmptyState
        icon="search"
        title="Nada bate com esse termo"
        description="Tente uma variação ou uma palavra mais curta."
      />
    </Card>
  {:else if query.trim() === ''}
    <Card padding="none">
      <EmptyState
        icon="search"
        title="O que você quer encontrar?"
        description="Digite um nome, uma hashtag (#brasil) ou um trecho de uma publicação."
      />
    </Card>
  {:else}
    {#if (active === 'all' || active === 'accounts') && accounts.length > 0}
      <section class="sec">
        <h2>Contas</h2>
        <ul class="list">
          {#each accounts as m (m.handle)}
            <li>
              <a href={`/perfil/?u=${encodeURIComponent(m.handle)}`}>
                <Avatar
                  src={m.avatar_url}
                  name={m.display_name ?? m.handle}
                  alt=""
                  size="base"
                />
                <div class="body">
                  <strong>{m.display_name ?? m.handle}</strong>
                  <span class="muted">@{m.handle}</span>
                  {#if m.bio}
                    <p class="bio">{m.bio}</p>
                  {/if}
                </div>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if (active === 'all' || active === 'hashtags') && hashtags.length > 0}
      <section class="sec">
        <h2>Hashtags</h2>
        <ul class="list">
          {#each hashtags as h (h.tag_normalized)}
            <li>
              <a href={`/tag?nome=${encodeURIComponent(h.tag_original)}`}>
                <span class="hash-ic">#</span>
                <div class="body">
                  <strong>{h.tag_original}</strong>
                  <span class="muted">
                    {h.note_count} {h.note_count === 1 ? 'nota' : 'notas'}
                  </span>
                </div>
              </a>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if (active === 'all' || active === 'notes') && notes.length > 0}
      <section class="sec">
        <h2>Publicações</h2>
        <ol class="notes">
          {#each notes as n (n.object_uri)}
            <li>
              <Card>
                <a class="note-link" href={`/publicacao?uri=${encodeURIComponent(n.object_uri)}`}>
                  <header>
                    <Avatar
                      src={n.author_avatar_url}
                      name={n.author_display_name ?? n.author_handle}
                      size="sm"
                    />
                    <div class="who">
                      <strong>{n.author_display_name ?? n.author_handle}</strong>
                      <span class="muted">
                        {n.author_handle.startsWith('@') ? n.author_handle : `@${n.author_handle}`}
                        · {formatRelative(n.published_at)}
                      </span>
                    </div>
                  </header>
                  <div class="content">
                    <!-- eslint-disable-next-line svelte/no-at-html-tags -->
                    {@html sanitizeNoteHtml(n.content_html)}
                  </div>
                </a>
              </Card>
            </li>
          {/each}
        </ol>
      </section>
    {/if}
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
  .search-form {
    display: flex;
    gap: var(--sp-2);
    margin-bottom: var(--sp-4);
    align-items: flex-start;
  }
  .search-form :global(.field) {
    flex: 1;
    margin-bottom: 0;
  }
  .sec {
    margin-bottom: var(--sp-6);
  }
  .sec h2 {
    font-size: var(--fs-xl);
    margin: 0 0 var(--sp-3);
  }
  .list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .list a {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    text-decoration: none;
    color: var(--text-1);
    transition:
      background var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .list a:hover {
    background: var(--surface-2);
    border-color: var(--border-strong);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
    flex: 1;
  }
  .body strong {
    color: var(--text-1);
  }
  .body .muted {
    font-size: var(--fs-sm);
    color: var(--text-3);
  }
  .bio {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-sm);
    color: var(--text-2);
    line-height: var(--lh-snug);
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
  }
  .hash-ic {
    width: 40px;
    height: 40px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--accent-soft);
    color: var(--accent-strong);
    border-radius: 50%;
    font-weight: var(--fw-bold);
    font-size: var(--fs-lg);
  }
  .notes {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
  }
  .note-link {
    text-decoration: none;
    color: inherit;
    display: block;
  }
  .note-link header {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-2);
  }
  .who {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .who strong {
    color: var(--text-1);
  }
  .content {
    color: var(--text-1);
    overflow-wrap: anywhere;
    line-height: var(--lh-base);
    max-height: 8em;
    overflow: hidden;
  }
  .content :global(p) {
    margin: 0 0 var(--sp-2);
  }
  .loading {
    display: grid;
    gap: var(--sp-3);
  }
  .muted {
    color: var(--text-3);
  }
</style>
