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
  import { getMyProfile, refreshMyActor, getMyAdminStatus } from '../../lib/api';
  import { toast } from '../../lib/toasts';
  import ThemeToggle from './ThemeToggle.svelte';

  let name = $state<string | null>(null);
  let avatar = $state<string | null>(null);
  let userHandle = $state<string | null>(null);
  let loggedIn = $state(false);
  let open = $state(false);
  let ready = $state(false);
  let root = $state<HTMLElement | null>(null);
  // isAdmin é cacheado em localStorage pra pintar o link "Administração"
  // sem esperar o round-trip da /me/admin-status.
  let isAdmin = $state(false);

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
    // Cached admin flag: paint imediato, revalida em background.
    isAdmin = read('dsoc_is_admin') === '1';
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
    const ar = await getMyAdminStatus();
    if (ar.success && ar.data) {
      isAdmin = ar.data.is_admin;
      write('dsoc_is_admin', isAdmin ? '1' : '0');
    }
  });

  function onDocumentClick(event: MouseEvent) {
    if (open && root && !root.contains(event.target as Node)) open = false;
  }

  function onKeydown(event: KeyboardEvent) {
    if (event.key === 'Escape') open = false;
  }

  let refreshing = $state(false);
  async function pokeFediverse() {
    if (refreshing) return;
    refreshing = true;
    const res = await refreshMyActor();
    refreshing = false;
    if (res.success && res.data) {
      const { delivered_to, targets } = res.data;
      if (targets === 0) {
        toast.info('Você ainda não tem seguidores remotos para notificar.');
      } else if (delivered_to === 0) {
        toast.warning(
          `Nenhum dos ${targets} seguidores respondeu — tente novamente em instantes.`,
        );
      } else {
        toast.success(
          `Perfil propagado para ${delivered_to}/${targets} seguidores remotos.`,
        );
      }
    } else {
      toast.error(res.error?.message ?? 'Não foi possível propagar o perfil.');
    }
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
    for (const k of ['dsoc_citizen', 'dsoc_handle', 'dsoc_name', 'dsoc_avatar', 'dsoc_is_admin'])
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
          <button class="item item-button" type="button" onclick={pokeFediverse} disabled={refreshing}>
            {refreshing ? 'Propagando…' : 'Atualizar perfil no fediverso'}
          </button>
          <hr class="sep" />
          <a class="item" href="/rede">Seguindo e seguidores</a>
          <a class="item" href="/convites">Convidar pessoas</a>
          <a class="item" href="/configuracoes">Configurações</a>
          {#if isAdmin}
            <a class="item item-admin" href="/admin">
              ⚙️ Administração
            </a>
          {/if}
          <div class="theme-row">
            <span class="theme-label">Tema</span>
            <ThemeToggle />
          </div>
          <hr class="sep" />
          <button class="item item-button" type="button" onclick={logout}>Sair</button>
        </nav>
      {/if}
    </div>
  {:else}
    <ThemeToggle />
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
    gap: var(--sp-2);
    max-width: 16rem;
    padding: var(--sp-1) var(--sp-2);
    border: none;
    border-radius: var(--r-full);
    background: transparent;
    cursor: pointer;
    font: inherit;
    color: var(--text-3);
    font-size: var(--fs-sm);
    transition: background var(--dur-fast) var(--ease-out);
  }
  .trigger:hover,
  .trigger[aria-expanded='true'] {
    background: var(--surface-2);
  }
  .name {
    color: var(--text-1);
    font-weight: var(--fw-semibold);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .caret {
    font-size: 0.7rem;
    color: var(--text-3);
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
    background: var(--accent-soft);
    color: var(--accent-strong);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: var(--fs-sm);
    font-weight: var(--fw-bold);
  }

  .menu {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    min-width: 15rem;
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-base);
    box-shadow: var(--shadow-lg);
    padding: var(--sp-1);
    display: flex;
    flex-direction: column;
    z-index: 60;
  }
  .menu-kicker {
    margin: var(--sp-1) var(--sp-2) 2px;
    font-size: var(--fs-xs);
    font-weight: var(--fw-bold);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  .item {
    display: block;
    padding: var(--sp-2) var(--sp-2);
    border-radius: var(--r-sm);
    text-decoration: none;
    color: var(--text-1);
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
    text-align: left;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .item:hover {
    background: var(--surface-2);
  }
  .item-admin {
    color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .item-button {
    border: none;
    background: transparent;
    cursor: pointer;
    font-family: inherit;
    width: 100%;
    color: var(--text-1);
  }
  .item-button[disabled] {
    opacity: 0.6;
    cursor: not-allowed;
  }
  /* Logout item — same layout, danger color. Marked by being the LAST item-button. */
  .item-button:last-of-type {
    color: var(--danger);
  }
  .sep {
    border: none;
    border-top: 1px solid var(--border-subtle);
    margin: var(--sp-1) var(--sp-1);
  }
  .theme-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--sp-3);
    padding: var(--sp-2) var(--sp-2);
    font-size: var(--fs-sm);
    font-weight: var(--fw-medium);
  }
  .theme-label {
    color: var(--text-1);
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
      border-top: 1px solid var(--border-subtle);
      border-radius: 0;
      padding: var(--sp-1) 0 0;
      min-width: 0;
    }
  }
</style>
