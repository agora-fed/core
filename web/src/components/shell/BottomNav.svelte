<script lang="ts">
  // Mobile bottom navigation. Fixed to viewport bottom, 5 icons.
  // Route detection is hydrated client-side (SSG can't know the URL at
  // compile time for every host); we use window.location.pathname.
  import Icon from '../ui/Icon.svelte';
  import { onMount } from 'svelte';

  const items = [
    { href: '/', icon: 'home', label: 'Início' },
    { href: '/feed', icon: 'feed', label: 'Feed' },
    { href: '/propor', icon: 'plus', label: 'Propor', cta: true },
    { href: '/notificacoes', icon: 'bell', label: 'Notif.' },
    { href: '/politicos', icon: 'users', label: 'Políticos' },
  ];

  let path = $state('');
  onMount(() => {
    path = window.location.pathname;
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
      <Icon name={it.icon} size={22} />
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
</style>
