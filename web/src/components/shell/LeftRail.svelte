<script lang="ts">
  // Left navigation rail — shown on wide screens inside AppShell. Combines the
  // social surface (Feed, Explorar, Notif, Perfil, Configurações) that used to
  // live only in AuthMenu with an always-visible primary nav. On narrow
  // screens the rail collapses (BottomNav takes over).
  import Icon from '../ui/Icon.svelte';
  import Button from '../ui/Button.svelte';
  import { onMount, onDestroy } from 'svelte';
  import { getMyNotifications, isAuthError, clearLocalSession } from '../../lib/api';

  interface Props {
    active?: string;
  }
  let { active = '' }: Props = $props();

  const socialItems = [
    { id: 'feed', href: '/feed', icon: 'feed', label: 'Feed' },
    { id: 'notifs', href: '/notificacoes', icon: 'bell', label: 'Notificações' },
    { id: 'search', href: '/buscar', icon: 'search', label: 'Buscar' },
    { id: 'explore', href: '/explorar', icon: 'globe', label: 'Explorar' },
  ];
  const politicalItems = [
    { id: 'politicos', href: '/politicos', icon: 'users', label: 'Políticos' },
    { id: 'partidos', href: '/partidos', icon: 'party', label: 'Partidos' },
    { id: 'propostas', href: '/propostas', icon: 'ballot', label: 'Propostas' },
    { id: 'debates', href: '/debates', icon: 'chat', label: 'Debates' },
    { id: 'consultas', href: '/consultas', icon: 'mic', label: 'Consultas' },
  ];

  let handle = $state<string | null>(null);
  let loggedIn = $state(false);
  let unread = $state(0);
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  async function refreshUnread() {
    if (!loggedIn) return;
    const res = await getMyNotifications(1, 0);
    if (res.success && res.data) {
      unread = res.data.unread_count;
    } else if (isAuthError(res)) {
      // Session gone: clear cache + stop polling so the header switches back
      // to the anonymous state on the next render.
      clearLocalSession();
      loggedIn = false;
      unread = 0;
      if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
      }
    }
  }

  // Handler compartilhado — /notificacoes dispara este evento após clearAll;
  // BottomNav faz o mesmo. Assinatura instantânea sem esperar o poll de 60s.
  const onChanged = () => void refreshUnread();
  const onSwMessage = (e: MessageEvent) => {
    if (e.data?.type === 'dsoc-push') void refreshUnread();
  };

  onMount(() => {
    try {
      loggedIn = Boolean(localStorage.getItem('dsoc_citizen'));
      const h = localStorage.getItem('dsoc_handle');
      handle = h && !h.startsWith('u-') ? h : null;
    } catch {}
    if (loggedIn) {
      void refreshUnread();
      // Poll every 60s. Cheap query (partial-indexed COUNT).
      pollTimer = setInterval(refreshUnread, 60_000);
    }
    window.addEventListener('dsoc-notifications-changed', onChanged);
    if ('serviceWorker' in navigator) {
      navigator.serviceWorker.addEventListener('message', onSwMessage);
    }
  });

  onDestroy(() => {
    if (pollTimer) clearInterval(pollTimer);
    if (typeof window === 'undefined') return;
    window.removeEventListener('dsoc-notifications-changed', onChanged);
    if ('serviceWorker' in navigator) {
      navigator.serviceWorker.removeEventListener('message', onSwMessage);
    }
  });
</script>

<aside class="rail" aria-label="Navegação lateral">
  <p class="kicker">Rede social</p>
  <ul>
    {#each socialItems as it}
      <li>
        <a href={it.href} class:active={active === it.id}>
          <Icon name={it.icon} size={20} />
          <span>{it.label}</span>
          {#if it.id === 'notifs' && loggedIn && unread > 0}
            <span class="badge" aria-label={`${unread} não lidas`}>
              {unread > 99 ? '99+' : unread}
            </span>
          {/if}
        </a>
      </li>
    {/each}
    {#if loggedIn && handle}
      <li>
        <a
          href={`/perfil/?u=${encodeURIComponent(handle)}`}
          class:active={active === 'profile'}
        >
          <Icon name="profile" size={20} />
          <span>Meu perfil</span>
        </a>
      </li>
    {/if}
  </ul>

  <p class="kicker">Política</p>
  <ul>
    {#each politicalItems as it}
      <li>
        <a href={it.href} class:active={active === it.id}>
          <Icon name={it.icon} size={20} />
          <span>{it.label}</span>
        </a>
      </li>
    {/each}
  </ul>

  <div class="cta">
    <Button href="/propor" variant="primary" size="lg" fullWidth>
      <Icon name="plus" size={18} />
      Propor
    </Button>
  </div>
</aside>

<style>
  .rail {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    padding: var(--sp-4) 0;
    position: sticky;
    top: calc(64px + var(--sp-2));
    max-height: calc(100vh - 80px);
    overflow-y: auto;
    scrollbar-width: thin;
  }
  .kicker {
    margin: var(--sp-3) var(--sp-3) var(--sp-1);
    font-size: var(--fs-xs);
    font-weight: var(--fw-bold);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-3);
  }
  ul {
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }
  li a {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3) var(--sp-3);
    border-radius: var(--r-base);
    text-decoration: none;
    color: var(--text-2);
    font-weight: var(--fw-medium);
    font-size: var(--fs-md);
    transition:
      background var(--dur-fast) var(--ease-out),
      color var(--dur-fast) var(--ease-out);
  }
  li a:hover {
    background: var(--surface-2);
    color: var(--text-1);
  }
  li a.active {
    background: var(--accent-soft);
    color: var(--accent-strong);
    font-weight: var(--fw-semibold);
  }
  .badge {
    margin-left: auto;
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: var(--fs-xs);
    font-weight: var(--fw-bold);
    padding: 1px 8px;
    border-radius: var(--r-full);
    line-height: 1.4;
    font-variant-numeric: tabular-nums;
  }
  .cta {
    padding: var(--sp-3) var(--sp-2) 0;
  }
</style>
