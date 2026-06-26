<script lang="ts">
  // Header auth menu: shows "Olá @handle · Sair" when a session marker is present in localStorage,
  // falls back to "Entrar / Criar conta" otherwise. The HttpOnly cookie is the actual credential —
  // localStorage just remembers enough to render the header (citizen id + public handle). Logout
  // calls POST /auth/logout so the backend clears the HttpOnly cookie (JS cannot delete it).
  import { onMount } from 'svelte';

  let handle = $state<string | null>(null);
  let avatar = $state<string | null>(null);
  let ready = $state(false);

  function readHandle(): string | null {
    try {
      return (
        localStorage.getItem('dsoc_handle') ||
        // Fallback: older sessions only stored the citizen id; render a short prefix so the
        // header still reflects the logged-in state until the next login refreshes the handle.
        (localStorage.getItem('dsoc_citizen')
          ? `u-${localStorage.getItem('dsoc_citizen')!.replaceAll('-', '').slice(0, 8)}`
          : null)
      );
    } catch {
      return null;
    }
  }

  function readAvatar(): string | null {
    try {
      return localStorage.getItem('dsoc_avatar');
    } catch {
      return null;
    }
  }

  onMount(() => {
    handle = readHandle();
    avatar = readAvatar();
    ready = true;
  });

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
    try {
      localStorage.removeItem('dsoc_citizen');
      localStorage.removeItem('dsoc_handle');
      localStorage.removeItem('dsoc_avatar');
    } catch {
      /* storage may be blocked */
    }
    window.location.href = '/';
  }
</script>

<!-- Render nothing until we read storage on mount, to avoid a flash of "Entrar" on logged-in
     users (SSG ships static HTML so the server has no idea who you are). -->
{#if ready}
  {#if handle}
    <a class="hi" href="/configuracoes" title="Abrir configurações do perfil">
      {#if avatar}
        <img class="avatar" src={avatar} alt="" />
      {:else}
        <span class="avatar-placeholder" aria-hidden="true">👤</span>
      {/if}
      <strong>{handle}</strong>
    </a>
    <button class="btn btn-ghost" type="button" onclick={logout}>Sair</button>
  {:else}
    <a href="/entrar" class="btn btn-ghost">Entrar</a>
    <a href="/cadastrar" class="btn btn-primary">Criar conta</a>
  {/if}
{/if}

<style>
  .hi {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    align-self: center;
    color: var(--c-text-muted);
    font-size: 0.9rem;
    margin-right: 0.5rem;
    max-width: 16rem;
    overflow: hidden;
    text-decoration: none;
    padding: 0.3rem 0.55rem;
    border-radius: 999px;
  }
  .hi:hover {
    background: var(--c-bg);
  }
  .hi strong {
    color: var(--c-navy);
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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
    background: var(--c-bg);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    flex-shrink: 0;
    font-size: 0.95rem;
  }
  .btn {
    padding: 0.55rem 1rem;
    font-size: 0.92rem;
  }
</style>
