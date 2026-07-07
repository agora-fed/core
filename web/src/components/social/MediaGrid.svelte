<script lang="ts">
  // Twitter-like grid for 1–4 image attachments. Clicking any thumb opens
  // the MediaLightbox with the tapped index as the initial slide.
  import type { MediaAttachmentDto } from '../../lib/types';
  import MediaLightbox from './MediaLightbox.svelte';

  interface Props {
    media: MediaAttachmentDto[];
  }
  let { media }: Props = $props();

  let openIdx = $state<number | null>(null);

  function open(i: number) {
    openIdx = i;
  }

  const images = $derived(media.filter((m) => m.kind === 'image'));
</script>

{#if images.length > 0}
  <div class={`grid n-${Math.min(images.length, 4)}`} role="group" aria-label="Anexos">
    {#each images.slice(0, 4) as m, i (m.id)}
      <button
        type="button"
        class="cell"
        onclick={() => open(i)}
        aria-label={m.alt_text || `Imagem ${i + 1}`}
      >
        <img
          src={m.url}
          alt={m.alt_text ?? ''}
          loading="lazy"
          width={m.width ?? undefined}
          height={m.height ?? undefined}
        />
        {#if !m.alt_text}
          <span class="no-alt" aria-hidden="true">SEM ALT</span>
        {/if}
      </button>
    {/each}
  </div>
  {#if openIdx !== null}
    <MediaLightbox
      items={images}
      startAt={openIdx}
      onclose={() => (openIdx = null)}
    />
  {/if}
{/if}

<style>
  .grid {
    display: grid;
    gap: 4px;
    margin-top: var(--sp-3);
    border-radius: var(--r-base);
    overflow: hidden;
    max-height: 420px;
  }
  .n-1 {
    grid-template-columns: 1fr;
  }
  .n-2 {
    grid-template-columns: 1fr 1fr;
  }
  .n-3 {
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
  }
  .n-3 .cell:first-child {
    grid-row: 1 / 3;
  }
  .n-4 {
    grid-template-columns: 1fr 1fr;
    grid-template-rows: 1fr 1fr;
  }
  .cell {
    position: relative;
    display: block;
    padding: 0;
    margin: 0;
    border: 0;
    cursor: zoom-in;
    background: var(--surface-2);
    overflow: hidden;
  }
  .cell img {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: cover;
    transition: transform var(--dur-base) var(--ease-out);
  }
  .cell:hover img {
    transform: scale(1.02);
  }
  .cell:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
  .no-alt {
    position: absolute;
    bottom: var(--sp-2);
    left: var(--sp-2);
    background: color-mix(in srgb, var(--surface-inverse) 65%, transparent);
    color: var(--text-inverse);
    padding: 2px 6px;
    border-radius: var(--r-xs);
    font-size: 10px;
    font-weight: var(--fw-bold);
    letter-spacing: 0.06em;
  }
</style>
