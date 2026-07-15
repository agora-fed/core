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
    getFollowStatus,
    toggleLike,
    toggleBoost,
    getAttestations,
    attestCitizen,
    revokeAttestation,
    getCampanhaPublica,
    DEFAULT_ORG_ID,
    type RemoteNoteDto,
    type AttestationsDto,
  } from '../../lib/api';
  import type { ProfileDto } from '../../lib/types';
  import { formatDate, formatRelative } from '../../lib/format';
  import { toast } from '../../lib/toasts';
  import { sanitizeNoteHtml } from '../../lib/sanitize';
  import NoteComposer from './NoteComposer.svelte';
  import Icon from '../ui/Icon.svelte';

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
  let alreadyFollowing = $state(false);
  let loggedIn = $state(false);
  // Timeline remota carregada via /federation/actor-outbox proxy — cache backend
  // 60 s, front pinta enquanto o card do perfil fica no lugar.
  let remoteNotes = $state<RemoteNoteDto[]>([]);
  let notesLoading = $state(false);
  let notesError = $state<string | null>(null);
  // Estado local das reações (otimista) — chave: object_uri da nota.
  let liked = $state<Set<string>>(new Set());
  let boosted = $state<Set<string>>(new Set());
  let reactBusy = $state<Set<string>>(new Set());
  let replyingTo = $state<string | null>(null);

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

  // Atestado de cidadania (0.28.3, web-of-trust): quem já é verificado
  // (operador de mandato / admin de partido) atesta que conhece o cidadão.
  let attestations = $state<AttestationsDto | null>(null);
  let attestBusy = $state(false);

  async function loadAttestations(citizenId: string) {
    const res = await getAttestations(citizenId);
    if (res.success && res.data) attestations = res.data;
  }

  async function doAttest() {
    if (!profile || attestBusy) return;
    attestBusy = true;
    const res = await attestCitizen(profile.citizen_id);
    attestBusy = false;
    if (res.success) {
      toast('Atestado registrado — obrigado por fortalecer a rede de confiança.', 'success');
      void loadAttestations(profile.citizen_id);
    } else {
      toast(res.error?.message ?? 'Não foi possível atestar agora.', 'error');
    }
  }

  async function doRevoke() {
    if (!profile || attestBusy) return;
    attestBusy = true;
    const res = await revokeAttestation(profile.citizen_id);
    attestBusy = false;
    if (res.success) {
      toast('Atestado revogado.', 'success');
      void loadAttestations(profile.citizen_id);
    } else {
      toast(res.error?.message ?? 'Não foi possível revogar agora.', 'error');
    }
  }

  // Selo de financiamento transparente (0.31): a candidatura publicou a
  // declaração — o chip leva pra página pública /campanha/?u=<handle>.
  let campanhaHandle = $state<string | null>(null);

  async function loadCampanha(h: string) {
    const res = await getCampanhaPublica(h);
    if (res.success && res.data) campanhaHandle = h;
  }

  let attestTitle = $derived.by(() => {
    if (!attestations || attestations.count === 0) return '';
    const names = attestations.items
      .slice(0, 5)
      .map((i) => i.display_name ?? (i.handle ? `@${i.handle}` : 'operador'))
      .join(', ');
    return `Atestado por: ${names}${attestations.count > 5 ? '…' : ''}`;
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
      // Se já seguimos, o botão nasce como "Seguindo"; senão fica "Seguir".
      void checkFollowStatus(res.data.remote_actor_url);
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
    void loadAttestations(profile.citizen_id);
    if (profile.handle) void loadCampanha(profile.handle);
    if (profile.is_public && profile.handle) {
      fediAddress = `@${profile.handle}@${window.location.host}`;
      // Timeline do perfil local reusa o mesmo proxy do outbox — passa a URL
      // do próprio actor. Rebate no gateway e cai no fluxo AP, sem novo endpoint.
      if (loggedIn) {
        const selfActorUrl = `${window.location.origin}/actors/${profile.handle}`;
        void loadRemoteNotes(selfActorUrl);
      }
    }
  });

  async function checkFollowStatus(actorUrl: string) {
    const res = await getFollowStatus(actorUrl);
    if (res.success && res.data) {
      if (res.data.following) {
        alreadyFollowing = true;
        followState = 'sent';
      } else if (res.data.pending) {
        followState = 'sent';
      }
    }
  }

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

  async function onLikeNote(noteId: string) {
    if (!loggedIn) {
      toast.info('Entre pra favoritar.');
      return;
    }
    const key = `like:${noteId}`;
    if (reactBusy.has(key)) return;
    reactBusy = new Set(reactBusy).add(key);
    // Otimista: alterna já.
    const wasLiked = liked.has(noteId);
    const next = new Set(liked);
    if (wasLiked) next.delete(noteId); else next.add(noteId);
    liked = next;
    const res = await toggleLike(noteId);
    const done = new Set(reactBusy);
    done.delete(key);
    reactBusy = done;
    if (!res.success) {
      // Reverte otimismo.
      const revert = new Set(liked);
      if (wasLiked) revert.add(noteId); else revert.delete(noteId);
      liked = revert;
      toast.error(res.error?.message ?? 'Não consegui favoritar agora.');
    }
  }

  async function onBoostNote(noteId: string) {
    if (!loggedIn) {
      toast.info('Entre pra republicar.');
      return;
    }
    const key = `boost:${noteId}`;
    if (reactBusy.has(key)) return;
    reactBusy = new Set(reactBusy).add(key);
    const wasBoosted = boosted.has(noteId);
    const next = new Set(boosted);
    if (wasBoosted) next.delete(noteId); else next.add(noteId);
    boosted = next;
    const res = await toggleBoost(noteId);
    const done = new Set(reactBusy);
    done.delete(key);
    reactBusy = done;
    if (!res.success) {
      const revert = new Set(boosted);
      if (wasBoosted) revert.add(noteId); else revert.delete(noteId);
      boosted = revert;
      toast.error(res.error?.message ?? 'Não consegui republicar agora.');
    }
  }

  function replyHandleFor(): string {
    // "@user@host" sem o @ inicial — casa com o que o NoteComposer espera.
    if (!remote) return '';
    return remote.handle.replace(/^@/, '');
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
      {#if alreadyFollowing}
        <span class="hint hint-ok">Você segue este perfil ✓</span>
      {:else if followState === 'sent'}
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
              <footer class="note-actions">
                <button
                  type="button"
                  class="react"
                  class:on={liked.has(note.id)}
                  disabled={reactBusy.has(`like:${note.id}`)}
                  onclick={() => onLikeNote(note.id)}
                  aria-pressed={liked.has(note.id)}
                  aria-label={liked.has(note.id) ? 'Remover favorito' : 'Favoritar'}
                >
                  <Icon name={liked.has(note.id) ? 'heart-fill' : 'heart'} size={16} />
                  <span>Favoritar</span>
                </button>
                <button
                  type="button"
                  class="react"
                  class:on={boosted.has(note.id)}
                  disabled={reactBusy.has(`boost:${note.id}`)}
                  onclick={() => onBoostNote(note.id)}
                  aria-pressed={boosted.has(note.id)}
                  aria-label={boosted.has(note.id) ? 'Desfazer republicação' : 'Republicar'}
                >
                  <Icon name="boost" size={16} />
                  <span>Republicar</span>
                </button>
                <button
                  type="button"
                  class="react"
                  onclick={() => (replyingTo = replyingTo === note.id ? null : note.id)}
                  aria-label="Responder"
                >
                  <Icon name="reply" size={16} />
                  <span>Responder</span>
                </button>
                {#if note.url}
                  <a class="react react-external" href={note.url} target="_blank" rel="noopener noreferrer" title="Abrir no servidor original">
                    <Icon name="external" size={14} />
                    <span>Origem</span>
                  </a>
                {/if}
              </footer>
              {#if replyingTo === note.id}
                <div class="reply-inline">
                  <NoteComposer
                    variant="reply"
                    replyTo={{ uri: note.id, handle: replyHandleFor() }}
                    onposted={() => { replyingTo = null; toast.success('Resposta publicada.'); }}
                  />
                </div>
              {/if}
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
          {#if attestations && attestations.count > 0}
            <span class="chip chip-ok" title={attestTitle}>
              🤝 Cidadania atestada por {attestations.count}
              {attestations.count === 1 ? 'operador(a)' : 'operadores'}
            </span>
          {/if}
          {#if campanhaHandle}
            <a
              class="chip chip-ok chip-link"
              href={`/campanha/?u=${encodeURIComponent(campanhaHandle)}`}
              title="Esta candidatura declara publicamente o financiamento de campanha."
            >
              💰 Financiamento declarado
            </a>
          {/if}
          {#if profile.created_at}
            <span class="chip chip-plain" title={formatDate(profile.created_at)}>
              Por aqui desde {formatDate(profile.created_at)}
            </span>
          {/if}
        </div>
        {#if attestations?.viewer_can_attest}
          <div class="attest-cta">
            {#if attestations.viewer_attested}
              <button type="button" class="copy" onclick={doRevoke} disabled={attestBusy}>
                {attestBusy ? '…' : 'Revogar meu atestado'}
              </button>
            {:else}
              <button type="button" class="copy" onclick={doAttest} disabled={attestBusy}
                title="Você opera um mandato ou administra um partido: pode atestar publicamente que conhece esta pessoa. O atestado é público e revogável.">
                {attestBusy ? '…' : '🤝 Atestar que conheço esta pessoa'}
              </button>
            {/if}
          </div>
        {/if}
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

    {#if profile.is_public && loggedIn}
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
                </div>
                <div class="note-body">
                  {@html sanitizeNoteHtml(note.content_html)}
                </div>
                <footer class="note-actions">
                  <button
                    type="button"
                    class="react"
                    class:on={liked.has(note.id)}
                    disabled={reactBusy.has(`like:${note.id}`)}
                    onclick={() => onLikeNote(note.id)}
                    aria-pressed={liked.has(note.id)}
                    aria-label={liked.has(note.id) ? 'Remover favorito' : 'Favoritar'}
                  >
                    <Icon name={liked.has(note.id) ? 'heart-fill' : 'heart'} size={16} />
                    <span>Favoritar</span>
                  </button>
                  <button
                    type="button"
                    class="react"
                    class:on={boosted.has(note.id)}
                    disabled={reactBusy.has(`boost:${note.id}`)}
                    onclick={() => onBoostNote(note.id)}
                    aria-pressed={boosted.has(note.id)}
                    aria-label={boosted.has(note.id) ? 'Desfazer republicação' : 'Republicar'}
                  >
                    <Icon name="boost" size={16} />
                    <span>Republicar</span>
                  </button>
                  <button
                    type="button"
                    class="react"
                    onclick={() => (replyingTo = replyingTo === note.id ? null : note.id)}
                    aria-label="Responder"
                  >
                    <Icon name="reply" size={16} />
                    <span>Responder</span>
                  </button>
                </footer>
                {#if replyingTo === note.id}
                  <div class="reply-inline">
                    <NoteComposer
                      variant="reply"
                      replyTo={{ uri: note.id, handle: (profile.handle ?? profile.public_handle) ?? '' }}
                      onposted={() => { replyingTo = null; toast.success('Resposta publicada.'); }}
                    />
                  </div>
                {/if}
              </li>
            {/each}
          </ol>
        {/if}
      </section>
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
  .chip-link {
    text-decoration: none;
    cursor: pointer;
  }
  .chip-link:hover {
    filter: brightness(0.96);
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
  .note-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.65rem;
    padding-top: 0.5rem;
    border-top: 1px dashed var(--c-border);
  }
  .react {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.3rem 0.65rem;
    border: 1px solid var(--c-border);
    border-radius: 999px;
    background: transparent;
    color: var(--c-text-muted);
    font: inherit;
    font-size: 0.82rem;
    font-weight: 600;
    cursor: pointer;
    text-decoration: none;
    transition:
      background 120ms ease-out,
      color 120ms ease-out;
  }
  .react:hover:not(:disabled) {
    background: var(--c-bg, #f2f4f7);
    color: var(--c-text, #0f172a);
  }
  .react.on {
    color: var(--c-green-dark, #115c2d);
    background: var(--c-green-soft, #e6f7ed);
    border-color: #b7e4c7;
  }
  .react:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .react-external {
    margin-inline-start: auto;
  }
  .reply-inline {
    margin-top: 0.7rem;
    padding-top: 0.7rem;
    border-top: 1px dashed var(--c-border);
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
  .attest-cta {
    margin-top: 0.5rem;
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
