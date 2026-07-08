<script lang="ts">
  // Mobile bottom navigation. Fixed to viewport bottom, 5 icons.
  // Route detection is hydrated client-side (SSG can't know the URL at
  // compile time for every host); we use window.location.pathname.
  import Icon from '../ui/Icon.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { getMyNotifications, isAuthError, clearLocalSession } from '../../lib/api';

  const items = [
    { href: '/', icon: 'home', label: 'Início' },
    { href: '/feed', icon: 'feed', label: 'Feed' },
    { href: '/propor', icon: 'plus', label: 'Propor', cta: true },
    { href: '/notificacoes', icon: 'bell', label: 'Notif.', unread: true },
    { href: '/politicos', icon: 'users', label: 'Políticos' },
  ];

  let path = $state('');
  let loggedIn = $state(false);
  let unread = $state(0);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function refreshUnread() {
    if (!loggedIn) return;
    const res = await getMyNotifications(1, 0);
    if (res.success && res.data) {
      unread = res.data.unread_count;
    } else if (isAuthError(res)) {
      clearLocalSession();
      loggedIn = false;
      unread = 0;
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
    }
  }

  // Handler compartilhado com LeftRail: /notificacoes emite este evento após
  // clearAll ou quando um push chega enquanto a aba tá aberta.
  const onChanged = () => void refreshUnread();

  onMount(() => {
    path = window.location.pathname;
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
    if (loggedIn) {
      void refreshUnread();
      pollTimer = setInterval(refreshUnread, 60_000);
    }
    window.addEventListener('dsoc-notifications-changed', onChanged);
    // Push chegando com aba aberta: SW postMessage → refresh instantâneo.
    if ('serviceWorker' in navigator) {
      navigator.serviceWorker.addEventListener('message', (e) => {
        if (e.data?.type === 'dsoc-push') void refreshUnread();
      });
    }
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
    if (typeof window === 'undefined') return;
    window.removeEventListener('dsoc-notifications-changed', onChanged);
  });

  function isActive(href: string) {
    if (href === '/') return path === '/';
    return path.startsWith(href);
  }
</script>

<nav class="bn" aria-label="Navegação inferior">
  {#each items as it}
    <a
      href={it.href}
      class:active={isActive(it.href)}
      class:cta={it.cta}
      aria-current={isActive(it.href) ? 'page' : undefined}
    >
      <span class="ic-wrap">
        <Icon name={it.icon} size={22} />
        {#if it.unread && loggedIn && unread > 0}
          <span class="dot" aria-label={`${unread} não lidas`}>
            {unread > 9 ? '9+' : unread}
          </span>
        {/if}
      </span>
      <span>{it.label}</span>
    </a>
  {/each}
</nav>

<style>
  .bn {
    display: none;
  }
  @media (max-width: 920px) {
    .bn {
      display: grid;
      grid-template-columns: repeat(5, 1fr);
      position: fixed;
      bottom: 0;
      left: 0;
      right: 0;
      background: var(--surface-1);
      border-top: 1px solid var(--border-subtle);
      padding: var(--sp-1) var(--sp-1);
      padding-bottom: calc(var(--sp-1) + env(safe-area-inset-bottom, 0));
      z-index: 40;
      box-shadow: 0 -2px 8px rgba(0, 0, 0, 0.05);
    }
  }
  a {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 2px;
    padding: var(--sp-2) var(--sp-1);
    text-decoration: none;
    color: var(--text-3);
    font-size: 11px;
    font-weight: var(--fw-medium);
    border-radius: var(--r-sm);
    transition: color var(--dur-fast) var(--ease-out);
  }
  a:hover,
  a:focus-visible {
    color: var(--text-1);
    outline: none;
  }
  a.active {
    color: var(--accent);
  }
  a.cta {
    color: var(--accent-contrast);
    background: var(--accent);
    align-self: center;
    justify-self: center;
    width: 44px;
    height: 44px;
    border-radius: 50%;
    margin-top: -8px;
    box-shadow: var(--shadow-lg);
  }
  a.cta span {
    display: none;
  }
  a.cta:hover {
    color: var(--accent-contrast);
    background: var(--accent-strong);
  }
  .ic-wrap {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .dot {
    position: absolute;
    top: -6px;
    right: -10px;
    min-width: 16px;
    height: 16px;
    padding: 0 4px;
    background: var(--accent);
    color: var(--accent-contrast);
    border: 1.5px solid var(--surface-1);
    border-radius: var(--r-full);
    font-size: 10px;
    font-weight: var(--fw-bold);
    line-height: 13px;
    text-align: center;
    font-variant-numeric: tabular-nums;
  }
</style>
