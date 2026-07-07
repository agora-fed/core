<script lang="ts">
  // Fullscreen image viewer with alt text visible + arrow navigation.
  // Reads Esc to close, ArrowLeft/Right to move between slides.
  import { onMount } from 'svelte';
  import type { MediaAttachmentDto } from '../../lib/types';
  import Icon from '../ui/Icon.svelte';

  interface Props {
    items: MediaAttachmentDto[];
    startAt: number;
    onclose: () => void;
  }
  let { items, startAt, onclose }: Props = $props();

  let idx = $state(startAt);

  function prev() {
    idx = (idx - 1 + items.length) % items.length;
  }
  function next() {
    idx = (idx + 1) % items.length;
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Escape') onclose();
    else if (e.key === 'ArrowLeft') prev();
    else if (e.key === 'ArrowRight') next();
  }

  onMount(() => {
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = '';
    };
  });

  const current = $derived(items[idx]);
</script>

<svelte:window onkeydown={onKey} />

<div
  class="scrim"
  role="dialog"
  aria-modal="true"
  aria-label="Visualização de imagem"
  onclick={onclose}
>
  <button
    type="button"
    class="close"
    aria-label="Fechar"
    onclick={(e) => {
      e.stopPropagation();
      onclose();
    }}
  >
    <Icon name="x" size={22} />
  </button>

  {#if items.length > 1}
    <button
      type="button"
      class="nav prev"
      aria-label="Anterior"
      onclick={(e) => {
        e.stopPropagation();
        prev();
      }}
    >
      <Icon name="chevron-left" size={28} />
    </button>
    <button
      type="button"
      class="nav next"
      aria-label="Próxima"
      onclick={(e) => {
        e.stopPropagation();
        next();
      }}
    >
      <Icon name="chevron-right" size={28} />
    </button>
  {/if}

  <figure onclick={(e) => e.stopPropagation()}>
    <img
      src={current.url}
      alt={current.alt_text ?? ''}
    />
    {#if current.alt_text}
      <figcaption>{current.alt_text}</figcaption>
    {/if}
    {#if items.length > 1}
      <p class="counter">{idx + 1} / {items.length}</p>
    {/if}
  </figure>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.86);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--sp-6);
    z-index: 140;
    animation: fade var(--dur-fast) var(--ease-out);
  }
  figure {
    margin: 0;
    max-width: 100%;
    max-height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--sp-3);
  }
  img {
    max-width: 100%;
    max-height: calc(100vh - 160px);
    object-fit: contain;
    border-radius: var(--r-sm);
    box-shadow: 0 0 40px rgba(0, 0, 0, 0.5);
  }
  figcaption {
    color: #f1f5f9;
    font-size: var(--fs-sm);
    max-width: 44rem;
    text-align: center;
    background: rgba(0, 0, 0, 0.5);
    padding: var(--sp-2) var(--sp-4);
    border-radius: var(--r-full);
  }
  .counter {
    margin: 0;
    color: #cbd5e1;
    font-size: var(--fs-xs);
    font-variant-numeric: tabular-nums;
  }
  .close {
    position: absolute;
    top: var(--sp-4);
    right: var(--sp-4);
    width: 44px;
    height: 44px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: rgba(255, 255, 255, 0.08);
    color: #f1f5f9;
    border: 0;
    border-radius: 50%;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease-out);
  }
  .close:hover {
    background: rgba(255, 255, 255, 0.16);
  }
  .nav {
    position: absolute;
    top: 50%;
    transform: translateY(-50%);
    width: 48px;
    height: 48px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: rgba(255, 255, 255, 0.08);
    color: #f1f5f9;
    border: 0;
    border-radius: 50%;
    cursor: pointer;
  }
  .nav:hover {
    background: rgba(255, 255, 255, 0.16);
  }
  .prev {
    left: var(--sp-3);
  }
  .next {
    right: var(--sp-3);
  }
  @keyframes fade {
    from {
      opacity: 0;
    }
    to {
      opacity: 1;
    }
  }
</style>
