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
    lookupRemoteActor,
    type HashtagHit,
    type MentionHit,
    type RemoteActorDto,
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

  // Busca no fediverso — logo abaixo do header. Aceita @user@host, faz WebFinger
  // + Actor fetch via /api/v1/federation/lookup e mostra card com Seguir + link
  // pro perfil dentro do próprio DemocraciaBR.
  let fediQuery = $state('');
  let fediLooking = $state(false);
  let fediResult = $state<RemoteActorDto | null>(null);
  let fediError = $state<string | null>(null);
  let fediFollowing = $state(false);
  let fediFollowSent = $state(false);

  let fediValid = $derived(
    /^@?[^\s@]+@[^\s@]+\.[^\s@]+$/.test(fediQuery.trim()),
  );

  function stripHtml(html: string): string {
    return html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
  }

  async function lookupFedi(event: SubmitEvent) {
    event.preventDefault();
    if (!fediValid || fediLooking) return;
    if (!loggedIn) {
      fediError =
        'Entre pra buscar contas do fediverso — é uma limitação anti-crawler.';
      return;
    }
    fediLooking = true;
    fediError = null;
    fediResult = null;
    fediFollowSent = false;
    const res = await lookupRemoteActor(fediQuery.trim());
    fediLooking = false;
    if (res.success && res.data) {
      fediResult = res.data;
    } else {
      fediError = res.error?.message ?? 'Não consegui encontrar essa conta.';
    }
  }

  async function followFedi() {
    if (!fediResult || fediFollowing) return;
    fediFollowing = true;
    const res = await followRemoteActor(fediResult.remote_actor_url);
    fediFollowing = false;
    if (res.success) {
      fediFollowSent = true;
      toast.success('Solicitação de seguir enviada.');
    } else {
      toast.error(res.error?.message ?? 'Não foi possível seguir.');
    }
  }

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

  <section class="fedi-lookup">
    <form onsubmit={lookupFedi} class="fedi-form" novalidate>
      <label for="fedi-q" class="fedi-label">
        Procurar alguém no fediverso
      </label>
      <div class="fedi-row">
        <input
          id="fedi-q"
          type="text"
          class="fedi-input"
          bind:value={fediQuery}
          placeholder="@usuario@instancia (ex.: @zedirceu@masto.social)"
          autocomplete="off"
          spellcheck="false"
        />
        <Button
          type="submit"
          variant="primary"
          disabled={!fediValid || fediLooking}
          loading={fediLooking}
        >
          Buscar
        </Button>
      </div>
      <p class="fedi-hint muted">
        Digite o endereço completo no formato <code>@usuario@instancia</code>.
        O perfil abre dentro do DemocraciaBR e você pode seguir sem sair.
      </p>
    </form>

    {#if fediError}
      <p class="fedi-err" role="alert">{fediError}</p>
    {/if}

    {#if fediResult}
      <Card>
        <div class="fedi-card">
          <a
            class="fedi-who"
            href={`/perfil/?u=${encodeURIComponent(fediResult.handle)}`}
          >
            <Avatar
              src={fediResult.avatar_url}
              name={fediResult.name ?? fediResult.handle}
              size="base"
            />
            <div class="fedi-meta">
              <strong>{fediResult.name ?? fediResult.preferred_username ?? fediResult.handle}</strong>
              <span class="muted">{fediResult.handle}</span>
              {#if fediResult.summary}
                <p class="fedi-summary muted">{stripHtml(fediResult.summary)}</p>
              {/if}
            </div>
          </a>
          <div class="fedi-actions">
            {#if fediFollowSent}
              <span class="fedi-ok">Solicitação enviada ✓</span>
            {:else}
              <Button
                variant="primary"
                size="sm"
                onclick={followFedi}
                disabled={fediFollowing}
                loading={fediFollowing}
              >
                Seguir
              </Button>
            {/if}
            <a
              class="fedi-open"
              href={`/perfil/?u=${encodeURIComponent(fediResult.handle)}`}
            >
              Abrir perfil
            </a>
          </div>
        </div>
      </Card>
    {/if}
  </section>

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
  .fedi-lookup {
    margin-bottom: var(--sp-6);
    padding: var(--sp-4);
    background: var(--surface-2);
    border-radius: var(--r-base);
  }
  .fedi-label {
    display: block;
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
    color: var(--text-1);
    margin-bottom: var(--sp-2);
  }
  .fedi-row {
    display: flex;
    gap: var(--sp-2);
    flex-wrap: wrap;
  }
  .fedi-input {
    flex: 1;
    min-width: 220px;
    padding: var(--sp-3);
    height: 44px;
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-sm);
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-sm);
  }
  .fedi-input:focus {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .fedi-hint {
    margin: var(--sp-2) 0 0;
    font-size: var(--fs-xs);
  }
  .fedi-hint code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    padding: 1px 5px;
    border-radius: 4px;
  }
  .fedi-err {
    margin: var(--sp-3) 0 0;
    color: var(--danger);
    font-size: var(--fs-sm);
  }
  .fedi-card {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-start;
    flex-wrap: wrap;
  }
  .fedi-who {
    display: flex;
    gap: var(--sp-3);
    align-items: flex-start;
    text-decoration: none;
    color: inherit;
    flex: 1;
    min-width: 200px;
  }
  .fedi-meta {
    min-width: 0;
    display: grid;
    gap: 2px;
  }
  .fedi-summary {
    margin: var(--sp-1) 0 0;
    font-size: var(--fs-sm);
    line-height: 1.4;
  }
  .fedi-actions {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    flex-shrink: 0;
  }
  .fedi-open {
    font-size: var(--fs-sm);
    color: var(--accent-strong);
    text-decoration: none;
    font-weight: var(--fw-semibold);
  }
  .fedi-open:hover {
    text-decoration: underline;
  }
  .fedi-ok {
    color: var(--accent-strong);
    font-weight: var(--fw-semibold);
    font-size: var(--fs-sm);
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
