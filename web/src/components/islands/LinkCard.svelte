<script lang="ts">
  /**
   * Link preview card — the thumbnail a pasted URL deserves (migration 0680).
   *
   * Rendered under a note when the backend resolved a card for its first link.
   * The image is the point of the whole feature: a YouTube link used to render as
   * bare text, which reads as broken.
   */
  import type { LinkPreviewCardDto } from '../../lib/types';

  export let card: LinkPreviewCardDto;

  /** Host shown as the card's footer, so the destination is legible before the click. */
  function hostOf(url: string): string {
    try {
      return new URL(url).host.replace(/^www\./, '');
    } catch {
      return '';
    }
  }

  $: host = card.site_name || hostOf(card.url);
  $: isVideo = card.kind === 'video';
</script>

<a
  class="link-card"
  class:video={isVideo}
  href={card.url}
  target="_blank"
  rel="noopener noreferrer nofollow ugc"
>
  {#if card.image_url}
    <div class="thumb">
      <!-- Referrer withheld: fetching a thumbnail must not tell the origin which
           note the reader is looking at. -->
      <img
        src={card.image_url}
        alt={card.title ?? 'Prévia do link'}
        loading="lazy"
        referrerpolicy="no-referrer"
      />
      {#if isVideo}
        <span class="play" aria-hidden="true">▶</span>
      {/if}
    </div>
  {/if}
  <div class="meta">
    {#if card.title}<strong class="title">{card.title}</strong>{/if}
    {#if card.description}<span class="desc">{card.description}</span>{/if}
    {#if host}<span class="host">{host}</span>{/if}
  </div>
</a>

<style>
  .link-card {
    display: block;
    margin-top: 0.6rem;
    border: 1px solid var(--border, #d8dde3);
    border-radius: 12px;
    overflow: hidden;
    text-decoration: none;
    color: inherit;
    background: var(--surface, #fff);
    transition: border-color 0.15s ease;
  }
  .link-card:hover {
    border-color: var(--brand, #2b6cb0);
  }
  .thumb {
    position: relative;
    /* A fixed ratio keeps the feed from reflowing as thumbnails load. */
    aspect-ratio: 16 / 9;
    background: var(--muted-bg, #eef1f4);
  }
  .thumb img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .play {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    font-size: 2.5rem;
    color: #fff;
    text-shadow: 0 2px 12px rgba(0, 0, 0, 0.6);
    pointer-events: none;
  }
  .meta {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    padding: 0.7rem 0.85rem;
  }
  .title {
    font-size: 0.95rem;
    line-height: 1.3;
    /* Two lines maximum: a card must not push the next post off the screen. */
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .desc {
    font-size: 0.85rem;
    color: var(--text-muted, #5a6570);
    display: -webkit-box;
    -webkit-line-clamp: 2;
    line-clamp: 2;
    -webkit-box-orient: vertical;
    overflow: hidden;
  }
  .host {
    font-size: 0.78rem;
    color: var(--text-muted, #5a6570);
    text-transform: lowercase;
  }
</style>
