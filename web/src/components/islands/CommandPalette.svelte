<script lang="ts">
  // Command palette: Cmd/Ctrl+K opens a searchable list of navigation targets
  // + quick actions (change theme, compose, log out). Mounted globally in
  // BaseLayout so every page inherits the shortcut.
  //
  // Keeps its own trivial fuzzy matcher (Levenshtein-ish scoring on lowercase
  // substring hits) to avoid dragging in a dep. Escape closes; ↑/↓ navigates;
  // Enter executes.
  import { onMount, tick } from 'svelte';
  import Icon from '../ui/Icon.svelte';
  import { setChoice, type ThemeChoice } from '../../lib/theme';

  interface Command {
    id: string;
    label: string;
    hint?: string;
    icon: string;
    kbd?: string;
    action: () => void;
    when?: 'always' | 'auth' | 'anon';
  }

  const nav = (href: string) => () => (window.location.href = href);
  const applyTheme = (t: ThemeChoice) => () => setChoice(t);

  let logged = $state(false);
  onMount(() => {
    try {
      logged = Boolean(localStorage.getItem('dsoc_citizen'));
    } catch {}
  });

  const commands = $derived<Command[]>(
    (
      [
        { id: 'go-home', label: 'Ir para o início', icon: 'home', action: nav('/') },
        { id: 'go-feed', label: 'Ir para o feed', icon: 'feed', action: nav('/feed'), when: 'auth' },
        { id: 'go-notif', label: 'Notificações', icon: 'bell', action: nav('/notificacoes'), when: 'auth' },
        { id: 'go-search', label: 'Buscar no fediverso', icon: 'search', action: nav('/buscar') },
        { id: 'go-explore', label: 'Explorar', icon: 'globe', action: nav('/explorar') },
        { id: 'go-politicos', label: 'Placar dos políticos', icon: 'users', action: nav('/politicos') },
        { id: 'go-partidos', label: 'Partidos', icon: 'party', action: nav('/partidos') },
        { id: 'go-propostas', label: 'Propostas', icon: 'ballot', action: nav('/propostas') },
        { id: 'go-debates', label: 'Debates', icon: 'chat', action: nav('/debates') },
        { id: 'go-consultas', label: 'Consultas', icon: 'mic', action: nav('/consultas') },
        { id: 'act-propor', label: 'Propor algo novo', hint: 'Ação', icon: 'plus', action: nav('/propor') },
        { id: 'act-config', label: 'Configurações', hint: 'Ação', icon: 'settings', action: nav('/configuracoes'), when: 'auth' },
        { id: 'act-login', label: 'Entrar', hint: 'Ação', icon: 'unlock', action: nav('/entrar'), when: 'anon' },
        { id: 'act-signup', label: 'Criar conta', hint: 'Ação', icon: 'plus', action: nav('/cadastrar'), when: 'anon' },
        { id: 'theme-auto', label: 'Tema: seguir sistema', hint: 'Tema', icon: 'settings', action: applyTheme('auto') },
        { id: 'theme-light', label: 'Tema: claro', hint: 'Tema', icon: 'sun', action: applyTheme('light') },
        { id: 'theme-dark', label: 'Tema: escuro', hint: 'Tema', icon: 'moon', action: applyTheme('dark') },
      ] as Command[]
    ).filter((c) => {
      if (c.when === 'auth') return logged;
      if (c.when === 'anon') return !logged;
      return true;
    }),
  );

  let open = $state(false);
  let query = $state('');
  let cursor = $state(0);
  let input = $state<HTMLInputElement | null>(null);

  function score(label: string, q: string): number {
    const l = label.toLowerCase();
    const s = q.toLowerCase().trim();
    if (!s) return 1;
    if (l.includes(s)) return 100 - l.indexOf(s);
    // fuzzy: every char of q appears in order
    let i = 0;
    for (const ch of l) {
      if (ch === s[i]) i++;
      if (i === s.length) return 20;
    }
    return 0;
  }

  const filtered = $derived(
    commands
      .map((c) => ({ c, s: score(c.label, query) }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .map((x) => x.c),
  );

  function toggle() {
    open = !open;
    query = '';
    cursor = 0;
    if (open) tick().then(() => input?.focus());
  }

  function onKey(e: KeyboardEvent) {
    const mod = e.metaKey || e.ctrlKey;
    if (mod && e.key.toLowerCase() === 'k') {
      e.preventDefault();
      toggle();
      return;
    }
    if (!open) return;
    if (e.key === 'Escape') {
      open = false;
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      cursor = Math.min(cursor + 1, filtered.length - 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      cursor = Math.max(cursor - 1, 0);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      const target = filtered[cursor];
      if (target) {
        open = false;
        target.action();
      }
    }
  }

  $effect(() => {
    // reset cursor when the query changes
    void query;
    cursor = 0;
  });
</script>

<svelte:window onkeydown={onKey} />

{#if open}
  <div class="scrim" onclick={() => (open = false)} role="presentation">
    <div
      class="palette"
      role="dialog"
      aria-modal="true"
      aria-label="Paleta de comandos"
      onclick={(e) => e.stopPropagation()}
    >
      <div class="search">
        <Icon name="search" size={18} />
        <input
          bind:this={input}
          type="text"
          bind:value={query}
          placeholder="Buscar comandos, páginas, ações…"
          aria-label="Buscar"
          autocomplete="off"
          spellcheck="false"
        />
        <kbd>Esc</kbd>
      </div>
      <ul role="listbox">
        {#each filtered as c, i (c.id)}
          <li
            role="option"
            aria-selected={i === cursor}
            class:active={i === cursor}
            onmouseenter={() => (cursor = i)}
            onclick={() => {
              open = false;
              c.action();
            }}
          >
            <span class="ic"><Icon name={c.icon} size={16} /></span>
            <span class="lbl">{c.label}</span>
            {#if c.hint}<span class="hint">{c.hint}</span>{/if}
          </li>
        {/each}
        {#if filtered.length === 0}
          <li class="empty">Nada corresponde a "{query}"</li>
        {/if}
      </ul>
      <div class="foot">
        <span><kbd>↑</kbd><kbd>↓</kbd> navegar</span>
        <span><kbd>Enter</kbd> abrir</span>
        <span><kbd>Esc</kbd> fechar</span>
      </div>
    </div>
  </div>
{/if}

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: color-mix(in srgb, var(--surface-inverse) 45%, transparent);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: flex-start;
    justify-content: center;
    padding-top: 10vh;
    z-index: 110;
    animation: fade var(--dur-fast) var(--ease-out);
  }
  .palette {
    width: min(560px, calc(100vw - var(--sp-4) * 2));
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-lg);
    box-shadow: var(--shadow-xl);
    overflow: hidden;
    display: flex;
    flex-direction: column;
    animation: rise var(--dur-base) var(--ease-out);
  }
  .search {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-4);
    border-bottom: 1px solid var(--border-subtle);
    color: var(--text-3);
  }
  .search input {
    flex: 1;
    background: transparent;
    border: 0;
    outline: none;
    color: var(--text-1);
    font: inherit;
    font-size: var(--fs-lg);
    min-width: 0;
  }
  .search input::placeholder {
    color: var(--text-3);
  }
  ul {
    list-style: none;
    padding: var(--sp-1);
    margin: 0;
    max-height: 55vh;
    overflow-y: auto;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--sp-3);
    padding: var(--sp-3);
    border-radius: var(--r-sm);
    cursor: pointer;
    color: var(--text-2);
    font-size: var(--fs-sm);
  }
  li.active {
    background: var(--accent-soft);
    color: var(--accent-strong);
  }
  li .ic {
    color: var(--text-3);
    display: inline-flex;
  }
  li.active .ic {
    color: var(--accent);
  }
  li .lbl {
    flex: 1;
    font-weight: var(--fw-medium);
  }
  li .hint {
    font-size: var(--fs-xs);
    color: var(--text-3);
    background: var(--surface-2);
    padding: 2px 8px;
    border-radius: var(--r-full);
  }
  li.empty {
    color: var(--text-3);
    text-align: center;
    padding: var(--sp-6);
    font-size: var(--fs-sm);
  }
  .foot {
    display: flex;
    justify-content: center;
    gap: var(--sp-4);
    padding: var(--sp-2) var(--sp-4);
    border-top: 1px solid var(--border-subtle);
    background: var(--surface-2);
    font-size: var(--fs-xs);
    color: var(--text-3);
  }
  kbd {
    background: var(--surface-1);
    border: 1px solid var(--border-subtle);
    border-radius: var(--r-xs);
    padding: 1px 6px;
    font-family: inherit;
    font-size: 11px;
    color: var(--text-2);
    box-shadow: 0 1px 0 var(--border-subtle);
    margin: 0 2px;
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(-6px) scale(0.98);
    }
    to {
      opacity: 1;
      transform: translateY(0) scale(1);
    }
  }
</style>
