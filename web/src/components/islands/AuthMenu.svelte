<script lang="ts">
  // Header auth menu: shows "Olá @handle · Sair" when a session marker is present in localStorage,
  // falls back to "Entrar / Criar conta" otherwise. The HttpOnly cookie is the actual credential —
  // localStorage just remembers enough to render the header (citizen id + public handle). Logout
  // calls POST /auth/logout so the backend clears the HttpOnly cookie (JS cannot delete it).
  import { onMount } from 'svelte';

  let handle = $state<string | null>(null);
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

  onMount(() => {
    handle = readHandle();
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
    <span class="hi" title="Sua identidade pública nesta plataforma">
      Olá <strong>{handle}</strong>
    </span>
    <button class="btn btn-ghost" type="button" onclick={logout}>Sair</button>
  {:else}
    <a href="/entrar" class="btn btn-ghost">Entrar</a>
    <a href="/cadastrar" class="btn btn-primary">Criar conta</a>
  {/if}
{/if}

<style>
  .hi {
    align-self: center;
    color: var(--c-text-muted);
    font-size: 0.9rem;
    margin-right: 0.5rem;
    max-width: 14rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .hi strong {
    color: var(--c-navy);
    font-weight: 600;
  }
  .btn {
    padding: 0.55rem 1rem;
    font-size: 0.92rem;
  }
</style>
