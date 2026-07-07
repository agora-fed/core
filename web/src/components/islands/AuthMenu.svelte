<script lang="ts">
  // Header profile menu. Logged-out: "Entrar / Criar conta". Logged-in: avatar + real name
  // opening a dropdown that concentrates the SOCIAL/fediverse surface (Feed, meu perfil,
  // buscar no fediverso, configurações, sair) — the main nav stays purely political.
  //
  // The HttpOnly cookie is the credential; localStorage only caches enough to paint the
  // header without a flash. The display name comes from GET /me (cached in dsoc_name):
  // the login response only carries the opaque `u-<hex>` public handle, which we never
  // show as a label anymore.
  import { onMount } from 'svelte';
  import { getMyProfile } from '../../lib/api';

  let name = $state<string | null>(null);
  let avatar = $state<string | null>(null);
  let userHandle = $state<string | null>(null);
  let loggedIn = $state(false);
  let open = $state(false);
  let ready = $state(false);
  let root = $state<HTMLElement | null>(null);

  function read(key: string): string | null {
    try {
      return localStorage.getItem(key);
    } catch {
      return null;
    }
  }

  function write(key: string, value: string | null) {
    try {
      if (value) localStorage.setItem(key, value);
      else localStorage.removeItem(key);
    } catch {
      /* storage may be blocked */
    }
  }

  /** Best display label — never the opaque `u-<hex>` hash. */
  const label = $derived(
    name || (userHandle ? `@${userHandle}` : 'Meu perfil'),
  );

  onMount(async () => {
    loggedIn = Boolean(read('dsoc_citizen'));
    // Paint immediately from cache, then refresh from /me in the background.
    name = read('dsoc_name');
    avatar = read('dsoc_avatar');
    const cachedHandle = read('dsoc_handle');
    userHandle =
      cachedHandle && !cachedHandle.startsWith('u-') ? cachedHandle : null;
    ready = true;
    if (!loggedIn) return;
    const res = await getMyProfile();
    if (res.success && res.data) {
      name = res.data.display_name || null;
      userHandle = res.data.handle || null;
      avatar = res.data.avatar_url || null;
      write('dsoc_name', name);
      write('dsoc_avatar', avatar);
      if (userHandle) write('dsoc_handle', userHandle);
    }
    // On failure keep the cached label — the drawer still works and the next
    // successful load refreshes it.
  });

  function onDocumentClick(event: MouseEvent) {
    if (open && root && !root.contains(event.target as Node)) open = false;
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') open = false;
  }

  async function logout(event: MouseEvent) {
    event.preventDefault();
    try {
      await fetch('/api/v1/auth/logout', {
        method: 'POST',
        credentials: 'include',
      });
    } catch {
      /* still clear locally; the cookie's TTL will expire it server-side */
    }
    for (const k of ['dsoc_citizen', 'dsoc_handle', 'dsoc_name', 'dsoc_avatar'])
      write(k, null);
    window.location.href = '/';
  }
</script>

<svelte:window onclick={onDocumentClick} onkeydown={onKeydown} />

<!-- Render nothing until we read storage on mount, to avoid a flash of "Entrar" on logged-in
     users (SSG ships static HTML so the server has no idea who you are). -->
{#if ready}
  {#if loggedIn}
    <div class="profile" bind:this={root}>
      <button
        type="button"
        class="trigger"
        aria-haspopup="menu"
        aria-expanded={open}
        onclick={() => (open = !open)}
      >
        {#if avatar}
          <img class="avatar" src={avatar} alt="" />
        {:else}
          <span class="avatar-placeholder" aria-hidden="true">
            {label.replace(/^@/, '').slice(0, 1).toUpperCase() || '?'}
          </span>
        {/if}
        <strong class="name">{label}</strong>
        <span class="caret" aria-hidden="true">▾</span>
      </button>

      {#if open}
        <nav class="menu" aria-label="Menu do perfil e rede social">
          <p class="menu-kicker">Rede social</p>
          <a class="item" href="/feed">Feed</a>
          {#if userHandle}
            <a class="item" href={`/perfil/?u=${encodeURIComponent(userHandle)}`}>Meu perfil</a>
          {/if}
          <a class="item" href="/configuracoes#fediverso">Fediverso — buscar e seguir</a>
          <hr class="sep" />
          <a class="item" href="/configuracoes">Configurações</a>
          <button class="item item-button" type="button" onclick={logout}>Sair</button>
        </nav>
      {/if}
    </div>
  {:else}
    <a href="/entrar" class="btn btn-ghost">Entrar</a>
    <a href="/cadastrar" class="btn btn-primary">Criar conta</a>
  {/if}
{/if}

<style>
  .profile {
    position: relative;
    display: inline-flex;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    max-width: 16rem;
    padding: 0.3rem 0.55rem;
    border: none;
    border-radius: 999px;
    background: transparent;
    cursor: pointer;
    font: inherit;
    color: var(--c-text-muted);
    font-size: 0.9rem;
  }
  .trigger:hover,
  .trigger[aria-expanded='true'] {
    background: var(--c-bg);
  }
  .name {
    color: var(--c-navy);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .caret {
    font-size: 0.7rem;
    color: var(--c-text-muted);
  }
  .avatar {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    object-fit: cover;
    flex-shrink: 0;
  }
  .avatar-placeholder {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    background: var(--c-green-soft);
    color: var(--c-green-dark);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: 0.85rem;
    font-weight: 700;
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    min-width: 15rem;
    background: var(--c-paper);
    border: 1px solid var(--c-border);
    border-radius: 12px;
    box-shadow: 0 10px 30px rgba(0, 0, 0, 0.1);
    padding: 0.4rem;
    display: flex;
    flex-direction: column;
    z-index: 60;
  }
  .menu-kicker {
    margin: 0.2rem 0.6rem 0.15rem;
    font-size: 0.72rem;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--c-text-muted);
  }
  .item {
    display: block;
    padding: 0.55rem 0.6rem;
    border-radius: 8px;
    text-decoration: none;
    color: var(--c-navy);
    font-size: 0.95rem;
    font-weight: 500;
    text-align: left;
  }
  .item:hover {
    background: var(--c-bg);
  }
  .item-button {
    border: none;
    background: transparent;
    cursor: pointer;
    font: inherit;
    width: 100%;
    color: var(--c-red, #b3261e);
  }
  .sep {
    border: none;
    border-top: 1px solid var(--c-border);
    margin: 0.3rem 0.2rem;
  }
  .btn {
    padding: 0.55rem 1rem;
    font-size: 0.92rem;
  }

  /* Inside the mobile drawer the dropdown flows in the layout instead of floating. */
  @media (max-width: 920px) {
    .profile {
      flex-direction: column;
      width: 100%;
    }
    .trigger {
      width: 100%;
      max-width: none;
      justify-content: flex-start;
    }
    .menu {
      position: static;
      box-shadow: none;
      border: none;
      border-top: 1px solid var(--c-border);
      border-radius: 0;
      padding: 0.3rem 0 0;
      min-width: 0;
    }
  }
</style>
