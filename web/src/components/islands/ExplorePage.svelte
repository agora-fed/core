<script lang="ts">
  // Explore — trending hashtags + suggested follows + a peek at the profile
  // directory. Anonymous users see everything except suggestions (which need
  // a session so the DB can filter "who you don't already follow").
  import { onMount } from 'svelte';
  import {
    getTrendingHashtags,
    getFollowSuggestions,
    getDirectory,
    followRemoteActor,
    type HashtagHit,
    type MentionHit,
  } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import Card from '../ui/Card.svelte';
  import Button from '../ui/Button.svelte';
  import Avatar from '../ui/Avatar.svelte';
  import Icon from '../ui/Icon.svelte';
  import Skeleton from '../ui/Skeleton.svelte';
  import EmptyState from '../ui/EmptyState.svelte';

  let ready = $state(false);
  let loggedIn = $state(false);
  let trends = $state<HashtagHit[]>([]);
  let suggestions = $state<MentionHit[]>([]);
  let directory = $state<MentionHit[]>([]);
  let loading = $state(true);
  let followingBusy = $state<Set<string>>(new Set());
  let followed = $state<Set<string>>(new Set());

  async function load() {
    loading = true;
    const [t, d, s] = await Promise.all([
      getTrendingHashtags(10),
      getDirectory(18, 0),
      loggedIn ? getFollowSuggestions(9) : Promise.resolve(null),
    ]);
    loading = false;
    if (t.success && t.data) trends = t.data.items;
    if (d.success && d.data) directory = d.data.items;
    if (s && s.success && s.data) suggestions = s.data.items;
  }

  async function follow(m: MentionHit) {
    if (!loggedIn || followingBusy.has(m.actor_url)) return;
    followingBusy = new Set(followingBusy).add(m.actor_url);
    const res = await followRemoteActor(m.actor_url);
    const next = new Set(followingBusy);
    next.delete(m.actor_url);
    followingBusy = next;
    if (res.success) {
      followed = new Set(followed).add(m.actor_url);
      toast.success(`Seguindo @${m.handle}.`);
    } else {
      toast.error(res.error?.message ?? 'Não foi possível seguir.');
    }
  }

  onMount(() => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
    ready = true;
    void load();
  });
</script>

{#if !ready}
  <p class="muted" aria-hidden="true">Carregando…</p>
{:else}
  <header class="head">
    <div class="ic"><Icon name="globe" size={24} /></div>
    <div>
      <h1>Explorar</h1>
      <p class="muted">
        Hashtags em alta, pessoas para acompanhar e o diretório público.
      </p>
    </div>
  </header>

  {#if loading}
    <div class="loading">
      <Card><Skeleton lines={4} /></Card>
      <Card><Skeleton lines={4} /></Card>
    </div>
  {:else}
    <div class="grid">
      <section>
        <h2><Icon name="hashtag" size={16} /> Em alta agora</h2>
        {#if trends.length === 0}
          <Card padding="none">
            <EmptyState
              icon="hashtag"
              title="Sem tendências ainda"
              description="Publique algo com #hashtag para aparecer aqui."
            />
          </Card>
        {:else}
          <ul class="trend-list">
            {#each trends as h, i (h.tag_normalized)}
              <li>
                <a href={`/tag?nome=${encodeURIComponent(h.tag_original)}`}>
                  <span class="rank">{i + 1}</span>
                  <div class="body">
                    <strong>#{h.tag_original}</strong>
                    <span class="muted">
                      {h.note_count} {h.note_count === 1 ? 'nota' : 'notas'} · 24h
                    </span>
                  </div>
                </a>
              </li>
            {/each}
          </ul>
        {/if}
      </section>

      <section>
        <h2><Icon name="users" size={16} /> Para acompanhar</h2>
        {#if !loggedIn}
          <Card padding="none">
            <EmptyState
              icon="lock"
              title="Entre para ver sugestões"
              description="Sugestões de quem acompanhar dependem de saber quem você já segue."
              action={loginAction}
            />
            {#snippet loginAction()}
              <Button href="/entrar" variant="primary">Entrar</Button>
            {/snippet}
          </Card>
        {:else if suggestions.length === 0}
          <Card padding="none">
            <EmptyState
              icon="users"
              title="Você já segue todo mundo por aqui"
              description="Volte quando novas pessoas publicarem."
            />
          </Card>
        {:else}
          <ul class="suggestion-list">
            {#each suggestions as m (m.handle)}
              <li>
                <Card>
                  <div class="s-head">
                    <a href={`/perfil/?u=${encodeURIComponent(m.handle)}`} class="s-who">
                      <Avatar
                        src={m.avatar_url}
                        name={m.display_name ?? m.handle}
                        size="base"
                      />
                      <div class="s-body">
                        <strong>{m.display_name ?? m.handle}</strong>
                        <span class="muted">@{m.handle}</span>
                      </div>
                    </a>
                    <Button
                      variant={followed.has(m.actor_url) ? 'ghost' : 'primary'}
                      size="sm"
                      disabled={followed.has(m.actor_url) || followingBusy.has(m.actor_url)}
                      loading={followingBusy.has(m.actor_url)}
                      onclick={() => follow(m)}
                    >
                      {followed.has(m.actor_url) ? 'Seguindo' : 'Seguir'}
                    </Button>
                  </div>
                  {#if m.bio}
                    <p class="s-bio">{m.bio}</p>
                  {/if}
                </Card>
              </li>
            {/each}
          </ul>
        {/if}
      </section>
    </div>

    <section class="dir">
      <h2><Icon name="profile" size={16} /> Diretório</h2>
      {#if directory.length === 0}
        <Card padding="none">
          <EmptyState
            icon="users"
            title="Ninguém deixou o perfil público ainda"
            description="Em Configurações → Perfil, marque-o como público para aparecer aqui."
          />
        </Card>
      {:else}
        <ul class="dir-grid">
          {#each directory as m (m.handle)}
            <li>
              <a class="dir-item" href={`/perfil/?u=${encodeURIComponent(m.handle)}`}>
                <Avatar
                  src={m.avatar_url}
                  name={m.display_name ?? m.handle}
                  size="lg"
                />
                <strong>{m.display_name ?? m.handle}</strong>
                <span class="muted">@{m.handle}</span>
                {#if m.bio}
                  <p class="bio-clip">{m.bio}</p>
                {/if}
              </a>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  {/if}
{/if}

<style>
  .head {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    margin-bottom: var(--sp-6);
  }
  .head h1 {
    margin: 0;
    font-size: var(--fs-3xl);
    color: var(--text-1);
  }
  .head p {
    margin: 2px 0 0;
    font-size: var(--fs-sm);
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
    flex-shrink: 0;
  }
  .grid {
    display: grid;
    gap: var(--sp-6);
    grid-template-columns: 1fr;
    margin-bottom: var(--sp-8);
  }
  @media (min-width: 820px) {
    .grid {
      grid-template-columns: 1fr 1fr;
    }
  }
  section h2 {
    display: flex;
    align-items: center;
    gap: var(--sp-2);
    font-size: var(--fs-lg);
    margin: 0 0 var(--sp-3);
    color: var(--text-1);
  }
  .trend-list,
  .suggestion-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-2);
  }
  .trend-list a {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    text-decoration: none;
    color: var(--text-1);
    transition: background var(--dur-fast) var(--ease-out);
  }
  .trend-list a:hover {
    background: var(--surface-2);
  }
  .rank {
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: var(--surface-2);
    color: var(--text-3);
    border-radius: 50%;
    font-weight: var(--fw-bold);
    font-size: var(--fs-xs);
  }
  .body {
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  .body strong {
    color: var(--text-1);
  }
  .body .muted {
    font-size: var(--fs-xs);
  }
  .s-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
  }
  .s-who {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    text-decoration: none;
    color: var(--text-1);
    flex: 1;
    min-width: 0;
  }
  .s-body {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }
  .s-bio {
    margin: var(--sp-2) 0 0;
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
  .dir h2 {
    font-size: var(--fs-lg);
  }
  .dir-grid {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: var(--sp-3);
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
  }
  .dir-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-1);
    padding: var(--sp-4);
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    text-align: center;
    text-decoration: none;
    color: var(--text-1);
    transition:
      transform var(--dur-fast) var(--ease-out),
      box-shadow var(--dur-fast) var(--ease-out),
      border-color var(--dur-fast) var(--ease-out);
  }
  .dir-item:hover {
    transform: translateY(-2px);
    box-shadow: var(--shadow-lg);
    border-color: var(--border-strong);
  }
  .dir-item strong {
    color: var(--text-1);
    font-size: var(--fs-sm);
    margin-top: var(--sp-2);
  }
  .dir-item .muted {
    font-size: var(--fs-xs);
  }
  .bio-clip {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-xs);
    color: var(--text-3);
    line-height: var(--lh-snug);
    overflow: hidden;
    text-overflow: ellipsis;
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
  }
  .muted {
    color: var(--text-3);
  }
  .loading {
    display: grid;
    gap: var(--sp-3);
  }
</style>
