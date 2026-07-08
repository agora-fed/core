<script lang="ts">
  // Perfil público humano (cidadão OU político). CSR: como o site é SSG (ADR-0009) e não dá pra
  // pré-gerar um HTML por handle arbitrário no build, esta ilha lê o handle do query-param `?u=`
  // (client-side) e hidrata o perfil via GET /api/v1/profiles/{handle}. É o alvo do 302 que o
  // gateway emite quando um navegador acessa /actors/{handle}.
  //
  // Handles remotos do fediverso (`@user@host`) resolvem via WebFinger + Actor fetch
  // proxy no backend (`/api/v1/federation/lookup?acct=…`) e renderizam DENTRO do site
  // com um card equivalente — sem redirecionar pro Mastodon original.
  import { onMount } from 'svelte';
  import {
    getPublicProfile,
    lookupRemoteActor,
    followRemoteActor,
    getRemoteActorOutbox,
    DEFAULT_ORG_ID,
    type RemoteNoteDto,
  } from '../../lib/api';
  import type { ProfileDto } from '../../lib/types';
  import { formatDate, formatRelative } from '../../lib/format';
  import { toast } from '../../lib/toasts';
  import { sanitizeNoteHtml } from '../../lib/sanitize';

  let { handle: handleProp = '' }: { handle?: string } = $props();

  let handle = $state(handleProp);
  let loading = $state(true);
  let profile = $state<ProfileDto | null>(null);
  let loadError = $state<string | null>(null);
  // Endereço federado (@handle@host) — só faz sentido pra perfis públicos com @ escolhido.
  let fediAddress = $state<string | null>(null);
  let copied = $state(false);
  // Remote fediverse view (populado quando handle é @user@host).
  let remote = $state<{
    name: string | null;
    handle: string;
    avatar_url: string | null;
    summary: string | null;
    actor_url: string;
  } | null>(null);
  let following = $state(false);
  let followState = $state<'idle' | 'sent' | 'failed'>('idle');
  let loggedIn = $state(false);
  // Timeline remota carregada via /federation/actor-outbox proxy — cache backend
  // 60 s, front pinta enquanto o card do perfil fica no lugar.
  let remoteNotes = $state<RemoteNoteDto[]>([]);
  let notesLoading = $state(false);
  let notesError = $state<string | null>(null);

  function stripHtml(html: string): string {
    return html
      .replace(/<[^>]+>/g, ' ')
      .replace(/\s+/g, ' ')
      .trim();
  }

  function isRemoteHandle(h: string): boolean {
    // `@user@host.tld` ou `user@host.tld` — precisa ter DUAS partes separadas por @.
    const trimmed = h.replace(/^@/, '');
    return /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(trimmed);
  }

  let verifBadge = $derived.by(() => {
    const lvl = profile?.verification_level ?? 'none';
    switch (lvl) {
      case 'directory':
        return { label: 'Vínculo verificado', cls: 'ok' };
      case 'cpf':
        return { label: 'CPF verificado', cls: 'ok' };
      default:
        return null;
    }
  });

  // Badge de cidadania política (0.25.0-fediverso): sinaliza pra outros que a
  // conta é de brasileira(o) apta a votar em pauta urgente. Não expõe o número
  // do título — só o status.
  let tituloBadge = $derived.by(() => {
    switch (profile?.titulo_status) {
      case 'verified':
        return { label: 'Cidadania política verificada (TSE)', cls: 'ok' };
      case 'validated':
        return { label: 'Título de eleitor validado', cls: 'ok' };
      default:
        return null;
    }
  });

  let displayName = $derived(
    profile?.display_name ?? profile?.handle ?? 'Cidadã(o)',
  );
  let initials = $derived((displayName.charAt(0) || '?').toUpperCase());

  onMount(async () => {
    if (!handle) {
      handle = new URLSearchParams(window.location.search).get('u')?.trim() ?? '';
    }
    if (!handle) {
      loading = false;
      loadError = 'Perfil não informado.';
      return;
    }
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {
      /* storage bloqueado */
    }
    if (isRemoteHandle(handle)) {
      if (!loggedIn) {
        loading = false;
        loadError =
          'Perfis do fediverso são carregados só pra quem está logado — entre pra ver este perfil dentro do DemocraciaBR.';
        return;
      }
      const res = await lookupRemoteActor(handle);
      loading = false;
      if (!res.success || !res.data) {
        loadError =
          res.error?.message ??
          'Não consegui carregar esse perfil do fediverso agora.';
        return;
      }
      remote = {
        name: res.data.name ?? res.data.preferred_username,
        handle: res.data.handle.startsWith('@')
          ? res.data.handle
          : `@${res.data.handle}`,
        avatar_url: res.data.avatar_url,
        summary: res.data.summary ? stripHtml(res.data.summary) : null,
        actor_url: res.data.remote_actor_url,
      };
      // Puxa timeline em background: se o outbox demorar, o card já apareceu.
      void loadRemoteNotes(res.data.remote_actor_url);
      return;
    }
    const res = await getPublicProfile(handle, DEFAULT_ORG_ID);
    loading = false;
    if (!res.ok || !res.data) {
      loadError = res.error?.includes('not found')
        ? 'Este perfil não existe ou não é público.'
        : (res.error ?? 'Não foi possível carregar o perfil.');
      return;
    }
    profile = res.data;
    if (profile.is_public && profile.handle) {
      fediAddress = `@${profile.handle}@${window.location.host}`;
    }
  });

  async function followRemote() {
    if (!remote || following) return;
    following = true;
    const res = await followRemoteActor(remote.actor_url);
    following = false;
    if (res.success) {
      followState = 'sent';
      toast.success('Solicitação de seguir enviada.');
    } else {
      followState = 'failed';
      toast.error(res.error?.message ?? 'Não foi possível seguir agora.');
    }
  }

  async function loadRemoteNotes(actorUrl: string) {
    notesLoading = true;
    notesError = null;
    const res = await getRemoteActorOutbox(actorUrl);
    notesLoading = false;
    if (res.success && res.data) {
      remoteNotes = res.data;
    } else {
      notesError =
        res.error?.message ??
        'Não consegui carregar as notas desse perfil agora.';
    }
  }

  async function copyFedi() {
    if (!fediAddress) return;
    try {
      await navigator.clipboard.writeText(fediAddress);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      /* clipboard bloqueado — o endereço continua visível pra copiar na mão */
    }
  }
</script>

{#if loading}
  <div class="profile sk" aria-label="Carregando perfil…">
    <div class="cover sk-cover"></div>
    <div class="head">
      <span class="avatar-lg sk-block sk-circle"></span>
      <div class="head-meta">
        <span class="sk-line w50"></span>
        <span class="sk-line w30"></span>
      </div>
    </div>
  </div>
{:else if loadError}
  <div class="card state" role="alert">
    <h2>{loadError}</h2>
    {#if !loggedIn && isRemoteHandle(handle)}
      <p class="muted">Perfis do fediverso são renderizados dentro do DemocraciaBR, mas precisam de login.</p>
      <a class="btn btn-primary" href={`/entrar?next=${encodeURIComponent(`/perfil/?u=${handle}`)}`}>Entrar</a>
    {:else}
      <p class="muted">
        O perfil pode ser privado — na DemocraciaBR todo perfil nasce privado e
        só aparece aqui se a pessoa o tornar público.
      </p>
      <a class="btn btn-ghost" href="/">Voltar para o início</a>
    {/if}
  </div>
{:else if remote}
  <article class="profile">
    <div class="cover"></div>
    <header class="head">
      {#if remote.avatar_url}
        <img class="avatar-lg" src={remote.avatar_url} alt="" referrerpolicy="no-referrer" />
      {:else}
        <span class="avatar-lg avatar-fallback" aria-hidden="true">
          {(remote.name ?? remote.handle).charAt(0).toUpperCase()}
        </span>
      {/if}
      <div class="head-meta">
        <h1>{remote.name ?? remote.handle}</h1>
        <p class="handle">{remote.handle}</p>
        <div class="chips">
          <span class="chip chip-fedi" title="Perfil hospedado noutro servidor do fediverso, exibido aqui via proxy.">
            🌐 Fediverso
          </span>
        </div>
      </div>
    </header>

    {#if remote.summary}
      <section class="bio">
        <h2 class="visually-hidden">Bio</h2>
        <p>{remote.summary}</p>
      </section>
    {/if}

    <footer class="fedi remote-actions">
      {#if followState === 'sent'}
        <span class="hint hint-ok">Solicitação de seguir enviada ✓</span>
      {:else}
        <button
          type="button"
          class="btn btn-primary"
          onclick={followRemote}
          disabled={following}
        >
          {following ? 'Enviando…' : 'Seguir'}
        </button>
      {/if}
    </footer>

    <section class="remote-timeline" aria-label="Publicações">
      <h2 class="timeline-h">Publicações</h2>
      {#if notesLoading}
        <p class="muted">Carregando notas…</p>
      {:else if notesError}
        <p class="hint-error">{notesError}</p>
      {:else if remoteNotes.length === 0}
        <p class="muted">Sem publicações públicas recentes.</p>
      {:else}
        <ol class="notes-list">
          {#each remoteNotes as note (note.id)}
            <li class="note">
              <div class="note-meta">
                {#if note.published_at}
                  <time class="muted" datetime={note.published_at} title={formatDate(note.published_at)}>
                    {formatRelative(note.published_at)}
                  </time>
                {/if}
                {#if note.in_reply_to}
                  <span class="muted"> · em resposta a algo</span>
                {/if}
              </div>
              <div class="note-body">
                {@html sanitizeNoteHtml(note.content_html)}
              </div>
            </li>
          {/each}
        </ol>
      {/if}
    </section>
  </article>
{:else if profile}
  <article class="profile">
    <div
      class="cover"
      style={profile.cover_url ? `background-image:url(${profile.cover_url})` : ''}
    ></div>
    <header class="head">
      {#if profile.avatar_url}
        <img class="avatar-lg" src={profile.avatar_url} alt="" />
      {:else}
        <span class="avatar-lg avatar-fallback" aria-hidden="true">{initials}</span>
      {/if}
      <div class="head-meta">
        <h1>{displayName}</h1>
        <p class="handle">@{profile.handle ?? profile.public_handle}</p>
        <div class="chips">
          {#if verifBadge}
            <span class="chip chip-ok">✓ {verifBadge.label}</span>
          {/if}
          {#if tituloBadge}
            <span
              class="chip chip-titulo"
              title="Cidadã(o) com título de eleitor validado — vota em pauta urgente."
            >
              🇧🇷 {tituloBadge.label}
            </span>
          {/if}
          {#if profile.created_at}
            <span class="chip chip-plain" title={formatDate(profile.created_at)}>
              Por aqui desde {formatDate(profile.created_at)}
            </span>
          {/if}
        </div>
      </div>
    </header>

    {#if profile.bio}
      <section class="bio">
        <h2 class="visually-hidden">Bio</h2>
        <p>{profile.bio}</p>
      </section>
    {/if}

    {#if fediAddress}
      <footer class="fedi">
        <span class="muted">Siga no fediverso:</span>
        <code>{fediAddress}</code>
        <button
          type="button"
          class="copy"
          onclick={copyFedi}
          aria-label={`Copiar endereço ${fediAddress}`}
        >
          {copied ? 'Copiado ✓' : 'Copiar'}
        </button>
      </footer>
    {/if}
  </article>
{/if}

<style>
  .profile {
    border: 1px solid var(--c-border);
    border-radius: var(--radius, 14px);
    overflow: hidden;
    background: var(--c-paper, #fff);
    box-shadow: var(--shadow);
  }
  .cover {
    height: clamp(120px, 28vw, 190px);
    background:
      linear-gradient(115deg, var(--c-navy, #0f172a) 0%, var(--c-green-dark, #115c2d) 60%, var(--c-green, #15803d) 100%);
    background-size: cover;
    background-position: center;
  }
  .head {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 1.25rem;
    align-items: flex-end;
    padding: 0 1.5rem 1.25rem;
    margin-top: -52px;
  }
  .avatar-lg {
    width: 116px;
    height: 116px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--c-bg);
    border: 4px solid var(--c-paper, #fff);
    box-shadow: 0 2px 10px rgba(15, 23, 42, 0.12);
    flex-shrink: 0;
  }
  .avatar-fallback {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: 2.6rem;
    font-weight: 700;
    color: var(--c-green-dark);
    background: var(--c-green-soft);
  }
  .head-meta {
    padding-bottom: 0.4rem;
    min-width: 0;
  }
  .head h1 {
    margin: 0 0 0.1rem;
    font-size: clamp(1.3rem, 4vw, 1.6rem);
    overflow-wrap: anywhere;
  }
  .handle {
    margin: 0 0 0.55rem;
    color: var(--c-text-muted);
    font-size: 0.95rem;
    overflow-wrap: anywhere;
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
  }
  .chip {
    border-radius: 999px;
    padding: 0.15rem 0.6rem;
    font-size: 0.8rem;
    font-weight: 600;
    white-space: nowrap;
  }
  .chip-ok {
    background: var(--c-green-soft, #e6f7ed);
    color: var(--c-green-dark, #115c2d);
    border: 1px solid #b7e4c7;
  }
  .chip-plain {
    background: var(--c-bg, #f2f4f7);
    color: var(--c-text-muted);
    border: 1px solid var(--c-border);
    font-weight: 500;
  }
  .chip-titulo {
    background: var(--c-blue-soft, #e6efff);
    color: var(--c-blue-dark, #143c78);
    border: 1px solid #b7d0ff;
  }
  .chip-fedi {
    background: var(--c-blue-soft, #e6efff);
    color: var(--c-blue-dark, #143c78);
    border: 1px solid #b7d0ff;
  }
  .remote-actions {
    gap: 0.75rem;
  }
  .remote-actions .btn {
    padding: 0.45rem 1rem;
  }
  .note-remote {
    font-size: 0.85rem;
    line-height: 1.4;
    flex-basis: 100%;
  }
  .hint-ok {
    color: var(--c-green-dark, #115c2d);
    font-weight: 600;
  }
  .remote-timeline {
    border-top: 1px solid var(--c-border);
    padding: 1rem 1.5rem 1.5rem;
  }
  .timeline-h {
    font-size: 1rem;
    margin: 0 0 0.75rem;
    color: var(--c-text-muted);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .notes-list {
    list-style: none;
    padding: 0;
    margin: 0;
    display: grid;
    gap: 1rem;
  }
  .note {
    padding: 0.85rem 1rem;
    border: 1px solid var(--c-border);
    border-radius: 10px;
    background: var(--c-paper, #fff);
  }
  .note-meta {
    font-size: 0.82rem;
    margin-bottom: 0.4rem;
  }
  .note-body {
    line-height: 1.55;
    font-size: 0.95rem;
    overflow-wrap: anywhere;
  }
  .note-body :global(p) {
    margin: 0 0 0.5rem;
  }
  .note-body :global(p:last-child) {
    margin-bottom: 0;
  }
  .note-body :global(a) {
    color: var(--c-green-dark, #115c2d);
  }
  .hint-error {
    color: var(--danger, #b91c1c);
    font-size: 0.9rem;
  }
  @media (max-width: 560px) {
    .remote-timeline {
      padding-inline: 1rem;
    }
  }
  .bio {
    padding: 0 1.5rem 1.5rem;
  }
  .bio p {
    margin: 0;
    white-space: pre-wrap;
    line-height: 1.55;
    overflow-wrap: anywhere;
  }
  .fedi {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.5rem;
    padding: 0.85rem 1.5rem;
    border-top: 1px solid var(--c-border);
    background: var(--c-bg);
    font-size: 0.88rem;
  }
  .fedi code {
    font-family: ui-monospace, SFMono-Regular, monospace;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 6px;
    padding: 0.1rem 0.45rem;
    overflow-wrap: anywhere;
  }
  .copy {
    font: inherit;
    font-size: 0.82rem;
    font-weight: 600;
    color: var(--c-green-dark);
    background: transparent;
    border: 1px solid var(--c-border);
    border-radius: 999px;
    padding: 0.2rem 0.7rem;
    cursor: pointer;
  }
  .copy:hover {
    background: var(--c-paper);
  }
  .state {
    text-align: center;
    padding: 2.5rem 1.5rem;
  }
  .state h2 {
    font-size: 1.25rem;
  }
  .state .btn {
    margin-top: 0.5rem;
  }

  /* Skeleton */
  .sk-cover {
    opacity: 0.35;
  }
  .sk-block,
  .sk-line {
    background: var(--c-bg);
    animation: pulse 1.4s ease-in-out infinite;
  }
  .sk-circle {
    display: inline-block;
    border-radius: 50%;
  }
  .sk-line {
    display: block;
    height: 0.9rem;
    border-radius: 6px;
    margin-bottom: 0.5rem;
  }
  .w50 {
    width: 50%;
  }
  .w30 {
    width: 30%;
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

  /* Mobile: empilha avatar sobre o texto, centralizado — nada de coluna estreita apertada. */
  @media (max-width: 560px) {
    .head {
      grid-template-columns: 1fr;
      justify-items: center;
      text-align: center;
      gap: 0.6rem;
      padding-inline: 1rem;
      margin-top: -46px;
    }
    .avatar-lg {
      width: 92px;
      height: 92px;
    }
    .chips {
      justify-content: center;
    }
    .bio {
      padding-inline: 1rem;
      text-align: center;
    }
    .fedi {
      padding-inline: 1rem;
      justify-content: center;
    }
  }
</style>
