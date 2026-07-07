<script lang="ts">
  interface Props {
    variant?: 'text' | 'block' | 'circle';
    width?: string;
    height?: string;
    lines?: number;
  }
  let {
    variant = 'text',
    width = '100%',
    height,
    lines = 1,
  }: Props = $props();

  const h = $derived(
    height ??
      (variant === 'text' ? '1em' : variant === 'circle' ? width : '6rem'),
  );
</script>

{#if variant === 'text' && lines > 1}
  <span class="stack" aria-hidden="true">
    {#each Array.from({ length: lines }) as _, i}
      <span
        class="s text"
        style={`width:${i === lines - 1 ? '65%' : width};height:${h}`}
      ></span>
    {/each}
  </span>
{:else}
  <span
    class={`s ${variant}`}
    style={`width:${width};height:${h}`}
    aria-hidden="true"
  ></span>
{/if}

<style>
  .s {
    display: inline-block;
    background: linear-gradient(
      90deg,
      var(--surface-2) 25%,
      var(--surface-3) 37%,
      var(--surface-2) 63%
    );
    background-size: 200% 100%;
    border-radius: var(--r-sm);
    animation: shimmer 1.4s ease-in-out infinite;
  }
  .circle {
    border-radius: 50%;
  }
  .text {
    border-radius: 4px;
  }
  .stack {
    display: flex;
    flex-direction: column;
    gap: var(--sp-2);
    width: 100%;
  }
  @keyframes shimmer {
    from {
      background-position: 200% 0;
    }
    to {
      background-position: -200% 0;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .s {
      animation: none;
      background: var(--surface-2);
    }
  }
</style>
