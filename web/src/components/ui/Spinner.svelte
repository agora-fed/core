<script lang="ts">
  interface Props {
    size?: number;
    label?: string;
  }
  let { size = 20, label = 'Carregando' }: Props = $props();
</script>

<span
  class="sp"
  style={`width:${size}px;height:${size}px`}
  role="status"
  aria-label={label}
></span>

<style>
  .sp {
    display: inline-block;
    border: 2px solid var(--border-subtle);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
  }
  @keyframes spin {
    to {
      transform: rotate(1turn);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .sp {
      animation-duration: 2s;
    }
  }
</style>
