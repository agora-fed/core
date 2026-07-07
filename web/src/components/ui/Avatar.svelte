<script lang="ts">
  // Avatar: round image with graceful fallback to initials.
  // Sizes on the 8px scale — sm 32, base 40, lg 56, xl 80.
  interface Props {
    src?: string | null;
    alt?: string;
    name?: string;
    size?: 'xs' | 'sm' | 'base' | 'lg' | 'xl';
    ring?: boolean;
  }
  let {
    src = null,
    alt = '',
    name = '',
    size = 'base',
    ring = false,
  }: Props = $props();

  let broken = $state(false);
  const initial = $derived(
    (name || alt).replace(/^@/, '').trim().charAt(0).toUpperCase() || '?',
  );
  const showImg = $derived(!!src && !broken);
</script>

<span class={`a s-${size}`} class:ring aria-hidden={alt ? undefined : true}>
  {#if showImg}
    <img src={src ?? undefined} {alt} onerror={() => (broken = true)} />
  {:else}
    <span class="ph">{initial}</span>
  {/if}
</span>

<style>
  .a {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 50%;
    overflow: hidden;
    flex-shrink: 0;
    background: var(--accent-soft);
    color: var(--accent-strong);
    position: relative;
  }
  .a.ring {
    outline: 2px solid var(--surface-1);
    outline-offset: 2px;
    box-shadow: 0 0 0 3px var(--border-subtle);
  }
  img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .ph {
    font-weight: var(--fw-bold);
    line-height: 1;
  }
  .s-xs {
    width: 24px;
    height: 24px;
    font-size: 10px;
  }
  .s-sm {
    width: 32px;
    height: 32px;
    font-size: 12px;
  }
  .s-base {
    width: 40px;
    height: 40px;
    font-size: 14px;
  }
  .s-lg {
    width: 56px;
    height: 56px;
    font-size: 20px;
  }
  .s-xl {
    width: 80px;
    height: 80px;
    font-size: 28px;
  }
</style>
